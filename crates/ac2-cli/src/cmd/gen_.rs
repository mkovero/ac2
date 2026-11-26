//! `gen <signal>` (foreground, holds the lease), `gen stop` (universal) and `gen ceiling`
//! (the system max level).
//!
//! Safety: the level is a required typed dBFS value, checked against the daemon's ceiling
//! before the lease is even acquired. The command acquires and *arms* (arming does not
//! emit); only Enter fires. Esc, q, Ctrl-C, end of input, losing the lease or any error
//! stops output and releases the lease; if the process dies instead, the daemon's 1.5 s
//! lease expiry fades the output out.

use std::io::IsTerminal;

use ac2_client::{Client, LeaseLost, OnDrop, StimulusLease, expect_body};
use ac2_proto::model::{
    BandLimit, FilterOrder, GeneratorDesired, GeneratorSettings, MeasKind, Signal, State,
};
use ac2_proto::units::{Hz, Samples};
use ac2_proto::{Command, ReplyBody};
use serde_json::json;
use tokio::sync::mpsc;

use super::{connect, find_meas, rate};
use crate::CliError;
use crate::args::{Cli, GenCmd, GenOpts, MeasRef, SlopeArg};
use crate::output::Out;
use crate::watch::{Key, RawTerm, quit_signal};
use ac2_scene::rig;
use ac2_scene::unit_text::LevelDbfs;
use ac2_scene::unit_text::channels_text;

/// The settings `gen` would send, validated without a daemon.
pub fn settings(cmd: &GenCmd) -> Result<Option<(GeneratorSettings, bool)>, CliError> {
    let (signal, opts): (Signal, &GenOpts) = match cmd {
        GenCmd::Stop | GenCmd::Ceiling { .. } => return Ok(None),
        GenCmd::Pink(o) => (Signal::Pink, o),
        GenCmd::White(o) => (Signal::White, o),
        GenCmd::Sine { freq, opts, .. } => (Signal::Sine { freq: freq.0 }, opts),
        GenCmd::PeriodicPink { period, opts } => {
            let p = period.0;
            if !(1024..=(1 << 22)).contains(&p) || (p & (p - 1)) != 0 {
                return Err(CliError::Usage(
                    "--period must be a power of two, 1024 … 4194304 samples".into(),
                ));
            }
            (Signal::PeriodicPink { period: Samples(p) }, opts)
        }
    };
    let noise = !matches!(signal, Signal::Sine { .. });
    let band = match (opts.hp, opts.lp) {
        (None, None) => None,
        _ if !noise => {
            return Err(CliError::Usage(
                "--hp/--lp apply to noise signals only".into(),
            ));
        }
        (hp, lp) => {
            if let (Some(h), Some(l)) = (hp, lp)
                && h.0.0 >= l.0.0
            {
                return Err(CliError::Usage("--hp must be below --lp".into()));
            }
            Some(BandLimit {
                highpass: hp.map(|f| f.0),
                lowpass: lp.map(|f| f.0),
                order: match opts.slope {
                    SlopeArg::Db12 => FilterOrder::Second,
                    SlopeArg::Db24 => FilterOrder::Fourth,
                },
            })
        }
    };
    if opts.outputs.0.is_empty() {
        return Err(CliError::Usage("--out needs at least one channel".into()));
    }
    Ok(Some((
        GeneratorSettings {
            signal,
            level: opts.level.0,
            band,
            outputs: opts.outputs.0.clone(),
        },
        opts.force,
    )))
}

/// A sine moved onto an FFT bin of a spectrum measurement.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Lock {
    /// The measurement's name.
    pub meas: String,
    /// The bin's centre frequency, Hz.
    pub freq: f64,
    /// The FFT length the bin belongs to.
    pub fft_len: u32,
    /// The stream's sample rate, Hz.
    pub sample_rate: u32,
}

/// `freq` on the nearest bin of spectrum measurement `meas`: of its one FFT, or of a
/// multi-resolution spectrum's longest, the one the daemon reads the sine from.
pub(crate) fn lock(st: &State, meas: &MeasRef, freq: f64) -> Result<Lock, CliError> {
    let m = find_meas(st, meas)?;
    let MeasKind::Spectrum { config } = &m.config.kind else {
        return Err(CliError::Usage(format!(
            "--lock needs a spectrum measurement; \"{}\" is not one",
            m.config.name
        )));
    };
    let sample_rate = rate(st).ok_or_else(|| {
        CliError::Usage("no audio I/O open; open it with `ac2 stream open`".into())
    })?;
    let fs = f64::from(sample_rate);
    let n = config.resolution.sine_fft_len(fs);
    Ok(Lock {
        meas: m.config.name.clone(),
        freq: config.resolution.nearest_bin(freq, fs),
        fft_len: u32::try_from(n).unwrap_or(u32::MAX),
        sample_rate,
    })
}

fn signal_text(s: &GeneratorSettings) -> String {
    let sig = match s.signal {
        Signal::White => "white noise".to_owned(),
        Signal::Pink => "pink noise".to_owned(),
        Signal::PeriodicPink { period } => format!("periodic pink ({} samples)", period.0),
        // To the hundredth of a hertz, as the app's top bar and the harmonics plate print
        // it: a sine on an FFT bin is told from the round number beside it.
        Signal::Sine { freq } => format!("sine {}", ac2_scene::format::freq_exact(freq.0)),
        Signal::Ess { .. } => "sweep".to_owned(),
    };
    format!(
        "{sig} at {} on out {}",
        ac2_scene::format::dbfs(s.level.0),
        channels_text(&s.outputs)
    )
}

#[derive(Debug)]
pub(crate) enum Input {
    Key(Key),
    Eof,
}

/// Enter / quit from the terminal (raw mode) or, when stdin is not a terminal, from lines:
/// an empty line is Enter, `q` or end of input is quit.
pub(crate) fn input() -> Result<(Option<RawTerm>, mpsc::UnboundedReceiver<Input>), CliError> {
    let (tx, rx) = mpsc::unbounded_channel();
    if std::io::stdin().is_terminal() {
        let (term, mut keys) = RawTerm::enter(false)?;
        tokio::spawn(async move {
            while let Some(k) = keys.recv().await {
                if tx.send(Input::Key(k)).is_err() {
                    return;
                }
            }
        });
        Ok((Some(term), rx))
    } else {
        std::thread::spawn(move || {
            let mut line = String::new();
            loop {
                line.clear();
                match std::io::stdin().read_line(&mut line) {
                    Ok(0) | Err(_) => {
                        let _ = tx.send(Input::Eof);
                        return;
                    }
                    Ok(_) => {
                        let k = match line.trim() {
                            "q" | "quit" | "stop" => Key::Quit,
                            _ => Key::Enter,
                        };
                        if tx.send(Input::Key(k)).is_err() {
                            return;
                        }
                    }
                }
            }
        });
        Ok((None, rx))
    }
}

pub(crate) fn say(
    out: &mut Out<'_>,
    raw: bool,
    event: &str,
    human: &str,
    extra: serde_json::Value,
) {
    if out.json {
        let mut j = json!({ "event": event });
        if let (Some(o), serde_json::Value::Object(e)) = (j.as_object_mut(), extra) {
            o.extend(e);
        }
        let _ = out.json_line(&j);
    } else {
        // Raw mode needs explicit carriage returns.
        let nl = if raw { "\r\n" } else { "\n" };
        let _ = write!(out.w, "{}{nl}", human.replace('\n', nl));
        let _ = out.w.flush();
    }
}

/// `gen ceiling [LEVEL] [--yes]`.
async fn ceiling(
    cli: &Cli,
    level: Option<LevelDbfs>,
    yes: bool,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let g = c.snapshot().await?.state.generator;
    let Some(LevelDbfs(to)) = level else {
        let change = rig::ceiling_change(&g);
        out.emit(
            &json!({
                "ceiling": g.ceiling.0,
                "bound": g.ceiling_bound.0,
                "changed": change,
            }),
            || {
                let mut s = format!("system max level {}", rig::ceiling_line(&g));
                if let Some(ch) = &change {
                    s.push_str(&format!(" ({ch})"));
                }
                s
            },
        )?;
        return Ok(());
    };
    if to.0 > g.ceiling.0 && !yes {
        return Err(CliError::Refused(format!(
            "raising the system max level from {} to {} needs --yes",
            ac2_scene::format::dbfs(g.ceiling.0),
            ac2_scene::format::dbfs(to.0)
        )));
    }
    let r = c
        .call(Command::GenCeiling {
            ceiling: to,
            confirm_raise: yes,
        })
        .await?;
    let now = expect_body!("gen.ceiling", r, ReplyBody::Generator(g) => g)?;
    out.emit(
        &json!({
            "ceiling": now.ceiling.0,
            "bound": now.ceiling_bound.0,
            "previous": g.ceiling.0,
            "stopped": (g.armed || g.firing) && !(now.armed || now.firing),
        }),
        || {
            let mut s = rig::ceiling_changed(g.ceiling, now.ceiling);
            if (g.armed || g.firing) && !(now.armed || now.firing) {
                s.push_str("; the stimulus above it was stopped");
            }
            s
        },
    )?;
    Ok(())
}

pub(crate) async fn run(cli: &Cli, cmd: &GenCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    if let GenCmd::Ceiling { level, yes } = cmd {
        return ceiling(cli, *level, *yes, out).await;
    }
    let Some((mut settings, force)) = settings(cmd)? else {
        let c = connect(cli, false).await?;
        let r = c.call(Command::GenStop).await?;
        let rev = expect_body!("gen.stop", r, ReplyBody::Ack { rev } => rev)?;
        out.emit(&json!({ "stopped": true, "rev": rev }), || {
            "stopped: output faded out and disarmed".to_owned()
        })?;
        return Ok(());
    };
    let c = connect(cli, false).await?;
    let st = c.snapshot().await?.state;
    let ceiling = st.generator.ceiling;
    if settings.level.0 > ceiling.0 {
        return Err(CliError::Refused(format!(
            "level {} is above the daemon's ceiling {}",
            ac2_scene::format::dbfs(settings.level.0),
            ac2_scene::format::dbfs(ceiling.0)
        )));
    }
    if st.stream.open.is_none() {
        return Err(CliError::Usage(
            "no audio I/O open; open it with `ac2 stream open`".into(),
        ));
    }
    if let GenCmd::Sine {
        freq,
        lock: Some(meas),
        ..
    } = cmd
    {
        let l = lock(&st, meas, freq.0.0)?;
        settings.signal = Signal::Sine { freq: Hz(l.freq) };
        // The readout rounds; the bin's frequency is stated in full.
        say(
            out,
            false,
            "locked",
            &format!(
                "sine locked to a bin of \"{}\": {} Hz ({}-point FFT at {} Hz)",
                l.meas, l.freq, l.fft_len, l.sample_rate
            ),
            json!({
                "meas": l.meas,
                "freq": l.freq,
                "requested": freq.0.0,
                "fft_len": l.fft_len,
                "sample_rate": l.sample_rate,
            }),
        );
    }
    let lease = c.acquire_lease(force, OnDrop::StopAndRelease).await?;
    let keys = input()?;
    foreground(&c, lease, settings, out, keys).await
}

async fn foreground(
    c: &Client,
    mut lease: StimulusLease,
    settings: GeneratorSettings,
    out: &mut Out<'_>,
    (term, mut rx): (Option<RawTerm>, mpsc::UnboundedReceiver<Input>),
) -> Result<(), CliError> {
    let desired = |armed, firing| GeneratorDesired {
        settings: settings.clone(),
        armed,
        firing,
    };
    lease.set(desired(true, false)).await?;
    let what = signal_text(&settings);
    let raw = term.is_some();
    say(
        out,
        raw,
        "armed",
        &format!("ARMED  {what}\nEnter: fire / hold   Esc, q, Ctrl-C: stop"),
        json!({ "settings": settings, "client_id": c.client_id() }),
    );
    let quit = quit_signal();
    tokio::pin!(quit);
    let mut firing = false;
    let outcome: Result<(), CliError> = loop {
        tokio::select! {
            _ = &mut quit => break Ok(()),
            lost = lease.wait_lost() => {
                let LeaseLost::Refused { msg, .. } = &lost;
                say(out, raw, "lost", &format!("LEASE LOST: {msg}"), json!({ "reason": msg }));
                break Err(CliError::Refused(format!("stimulus lease lost: {msg}")));
            }
            i = rx.recv() => match i {
                None | Some(Input::Eof) | Some(Input::Key(Key::Quit)) => break Ok(()),
                Some(Input::Key(Key::Enter)) => {
                    firing = !firing;
                    match lease.set(desired(true, firing)).await {
                        Ok(_) if firing => say(out, raw, "firing", &format!("FIRING {what}"), json!({})),
                        Ok(_) => say(out, raw, "holding", "HOLD   armed, not emitting", json!({})),
                        Err(e) => break Err(e.into()),
                    }
                }
            },
        }
    };
    let lost = lease.lost().is_some();
    let end = lease.end().await;
    drop(term);
    if !lost {
        say(
            out,
            false,
            "stopped",
            "stopped: output faded out, disarmed, lease released",
            json!({}),
        );
    }
    outcome?;
    end.map_err(CliError::from)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use ac2_client::{ClientConfig, Endpoints};
    use ac2_proto::model::{DeviceSelector, Generator, SpecResolution, StreamConfig};
    use ac2_proto::units::Dbfs;

    use super::*;
    use ac2_scene::unit_text::{Channels, LevelDbfs};

    type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

    fn generator(c: &Client) -> Option<Generator> {
        c.watch()
            .borrow()
            .state
            .as_ref()
            .map(|s| s.generator.clone())
    }

    async fn until(what: &str, mut f: impl FnMut() -> bool) -> R {
        let end = Instant::now() + Duration::from_secs(10);
        while !f() {
            if Instant::now() > end {
                return Err(format!("timed out waiting for {what}").into());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(())
    }

    /// The daemon on its simulated rig (no audio leaves the process), a stream open on
    /// both outputs.
    async fn daemon() -> R<(ac2d::Handle, Client)> {
        let listen = ac2d::Listen::apart();
        let backend = Arc::new(ac2d::fake_rig()?);
        let mut config = ac2d::DaemonConfig::new(backend, listen, -10.0);
        // A rig set up before: its max level stored at the bound.
        config.memory_max_level = Some(-10.0);
        let h = ac2d::Daemon::start(config)?;
        let ep = Endpoints {
            ctrl: h.ctrl_endpoint().to_owned(),
            data: h.data_endpoint().to_owned(),
        };
        let c = Client::connect(ClientConfig::new(ep, "gen-test")).await?;
        c.wait_synced(Duration::from_secs(5)).await?;
        c.call(Command::StreamOpen {
            config: StreamConfig {
                backend: None,
                input_device: DeviceSelector::Default,
                output_device: DeviceSelector::Default,
                input_channels: vec![0],
                spare_inputs: 0,
                output_channels: 2,
                sample_rate_hz: None,
                buffer_frames: None,
                loopback: None,
                remote_inputs: Vec::new(),
                link_offset: None,
            },
        })
        .await?;
        Ok((h, c))
    }

    /// The bin the protocol crate names for a spectrum's resolution (what the app and
    /// `--lock` snap to) is the one the analyzer reads the sine on: the same FFT length as
    /// the multi-resolution ladder's longest window and the same bin as its `snap_to_bin`.
    #[test]
    fn the_shared_bin_snap_agrees_with_the_analyzer() {
        use ac2_core::harmonics::snap_to_bin;
        use ac2_core::multires::MultiResLadder;
        let freqs = [
            f64::NAN,
            -5.0,
            0.0,
            0.1,
            19.9,
            997.0,
            1000.0,
            12_345.678,
            23_999.0,
            95_999.0,
            1e6,
        ];
        for fs in [8000.0, 44_100.0, 48_000.0, 88_200.0, 96_000.0, 192_000.0] {
            let multi = SpecResolution::MULTI;
            assert_eq!(
                multi.sine_fft_len(fs),
                MultiResLadder::longest_window(fs),
                "{fs}"
            );
            let fixed = [3u32, 4, 5, 1024, 4096, 65_536, 1 << 20]
                .map(|fft_len| SpecResolution::Fixed { fft_len });
            for r in fixed.into_iter().chain([multi]) {
                let n = r.sine_fft_len(fs);
                if let SpecResolution::Fixed { fft_len } = r {
                    assert_eq!(n, fft_len as usize);
                }
                for f in freqs {
                    assert_eq!(
                        r.nearest_bin(f, fs),
                        snap_to_bin(f, fs, n),
                        "{r:?} {f} {fs}"
                    );
                }
            }
        }
    }

    /// A bin of the shortest FFT among several spectra is a bin of each of them, as the
    /// analyzer judges it: every length the daemon takes and the multi-resolution
    /// spectrum's longest window are powers of two, so the shorter's bins are among the
    /// longer's.
    #[test]
    fn a_bin_of_the_shortest_fft_is_a_bin_of_every_longer_one() {
        use ac2_core::harmonics::is_on_bin;
        use ac2_proto::model::{SpectrumConfig, nearest_bin};
        let lens: Vec<u32> = (0..32)
            .map(|k| 1u32 << k)
            .filter(|n| SpectrumConfig::fft_len_ok(*n))
            .collect();
        assert_eq!(lens.first(), Some(&SpectrumConfig::MIN_FFT_LEN));
        assert_eq!(lens.last(), Some(&SpectrumConfig::MAX_FFT_LEN));
        for fs in [8000.0, 44_100.0, 48_000.0, 88_200.0, 96_000.0, 192_000.0] {
            let all: Vec<SpecResolution> = lens
                .iter()
                .map(|&fft_len| SpecResolution::Fixed { fft_len })
                .chain([SpecResolution::MULTI])
                .collect();
            assert!(SpecResolution::MULTI.sine_fft_len(fs).is_power_of_two());
            assert_eq!(SpecResolution::shared_sine_fft_len([], fs), None);
            for shortest in &all {
                // This one with every one at least as long.
                let n = shortest.sine_fft_len(fs);
                let with: Vec<SpecResolution> = all
                    .iter()
                    .copied()
                    .filter(|r| r.sine_fft_len(fs) >= n)
                    .collect();
                assert_eq!(
                    SpecResolution::shared_sine_fft_len(with.iter().rev().copied(), fs),
                    Some(n)
                );
                for f in [19.9, 997.0, 1000.0, 12_345.678, 0.45 * fs] {
                    let bin = nearest_bin(f, fs, n);
                    assert_eq!(bin, shortest.nearest_bin(f, fs));
                    for r in &with {
                        assert!(
                            is_on_bin(bin, fs, r.sine_fft_len(fs)),
                            "{f} Hz at {fs}: bin {bin} of {n} points is off the bins of {r:?}"
                        );
                    }
                }
            }
        }
    }

    /// `--lock` puts the sine on a bin of the named spectrum (its one FFT, or the longest
    /// of a multi-resolution spectrum) at the stream's rate, and refuses anything else.
    #[tokio::test(flavor = "multi_thread")]
    async fn lock_moves_the_sine_onto_a_bin_of_a_spectrum() -> R {
        use ac2_core::harmonics::is_on_bin;
        use ac2_core::multires::MultiResLadder;
        use ac2_proto::model::{BandFraction, MeasConfig, MeasScope, RtaConfig, SpectrumConfig};

        let (_h, c) = daemon().await?;
        let spectrum = |name: &str, resolution| MeasConfig {
            scope: MeasScope::Session,
            name: name.into(),
            kind: MeasKind::Spectrum {
                config: SpectrumConfig {
                    resolution,
                    ..SpectrumConfig::on_input(0)
                },
            },
        };
        for config in [
            spectrum("fft", SpecResolution::Fixed { fft_len: 4096 }),
            spectrum("multi", SpecResolution::MULTI),
            MeasConfig {
                scope: MeasScope::Session,
                name: "rta".into(),
                kind: MeasKind::Rta {
                    config: RtaConfig::on_input(0, BandFraction::Third),
                },
            },
        ] {
            c.call(Command::MeasCreate { config }).await?;
        }
        let st = c.snapshot().await?.state;
        let fs = rate(&st).ok_or("no stream")?;
        let bin = f64::from(fs) / 4096.0;

        let l = lock(&st, &MeasRef("fft".into()), 1000.0)?;
        assert_eq!(
            (l.meas.as_str(), l.fft_len, l.sample_rate),
            ("fft", 4096, fs)
        );
        assert!(is_on_bin(l.freq, f64::from(fs), 4096), "{l:?}");
        assert!((l.freq - 1000.0).abs() <= bin / 2.0, "{l:?}");
        // Never DC: the lowest bin there is.
        assert_eq!(lock(&st, &MeasRef("fft".into()), 0.1)?.freq, bin);

        let l = lock(&st, &MeasRef("multi".into()), 1000.0)?;
        let longest = MultiResLadder::longest_window(f64::from(fs));
        assert_eq!(l.fft_len as usize, longest);
        assert!(
            longest > 4096 && is_on_bin(l.freq, f64::from(fs), longest),
            "{l:?}"
        );

        let e = lock(&st, &MeasRef("rta".into()), 1000.0).expect_err("an RTA is no spectrum");
        assert!(
            matches!(&e, CliError::Usage(m) if m.contains("needs a spectrum measurement")),
            "{e:?}"
        );
        assert!(lock(&st, &MeasRef("nothing".into()), 1000.0).is_err());
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn arm_enter_fires_quit_stops_and_releases() -> R {
        let (_h, c) = daemon().await?;
        let opts = GenOpts {
            outputs: Channels(vec![0, 1]),
            level: LevelDbfs(Dbfs(-30.0)),
            hp: None,
            lp: None,
            slope: SlopeArg::Db24,
            force: false,
        };
        let (settings, _) = settings(&GenCmd::Pink(opts))?.ok_or("no settings")?;
        let lease = c.acquire_lease(false, OnDrop::StopAndRelease).await?;
        let (tx, rx) = mpsc::unbounded_channel();
        let mut buf = Vec::new();
        let mut out = Out::new(true, &mut buf);
        let drive = async {
            until("armed", || generator(&c).is_some_and(|g| g.armed)).await?;
            // Arming alone never emits.
            assert!(generator(&c).is_some_and(|g| !g.firing));
            tx.send(Input::Key(Key::Enter))?;
            until("firing", || generator(&c).is_some_and(|g| g.firing)).await?;
            tx.send(Input::Key(Key::Quit))?;
            R::Ok(())
        };
        let (ran, drove) =
            tokio::join!(foreground(&c, lease, settings, &mut out, (None, rx)), drive);
        drove?;
        ran?;
        // Stopped, disarmed and released.
        until("released", || {
            generator(&c).is_some_and(|g| !g.firing && !g.armed && g.owner.is_none())
        })
        .await?;
        let text = String::from_utf8(buf)?;
        let events: Vec<String> = text
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter_map(|v| v["event"].as_str().map(str::to_owned))
            .collect();
        assert_eq!(events, ["armed", "firing", "stopped"]);
        Ok(())
    }
}
