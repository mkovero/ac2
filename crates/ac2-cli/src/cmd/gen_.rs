//! `gen <signal>` (foreground, holds the lease) and `gen stop` (universal).
//!
//! Safety: the level is a required typed dBFS value, checked against the daemon's ceiling
//! before the lease is even acquired. The command acquires and *arms* (arming does not
//! emit); only Enter fires. Esc, q, Ctrl-C, end of input, losing the lease or any error
//! stops output and releases the lease; if the process dies instead, the daemon's 1.5 s
//! lease expiry fades the output out.

use std::io::IsTerminal;

use ac2_client::{Client, LeaseLost, OnDrop, StimulusLease, expect_body};
use ac2_proto::model::{BandLimit, FilterOrder, GeneratorDesired, GeneratorSettings, Signal};
use ac2_proto::units::Samples;
use ac2_proto::{Command, ReplyBody};
use serde_json::json;
use tokio::sync::mpsc;

use super::connect;
use crate::CliError;
use crate::args::{Cli, GenCmd, GenOpts, SlopeArg};
use crate::output::{self, Out};
use crate::units::channels_text;
use crate::watch::{Key, RawTerm};

/// The settings `gen` would send, validated without a daemon.
pub fn settings(cmd: &GenCmd) -> Result<Option<(GeneratorSettings, bool)>, CliError> {
    let (signal, opts): (Signal, &GenOpts) = match cmd {
        GenCmd::Stop => return Ok(None),
        GenCmd::Pink(o) => (Signal::Pink, o),
        GenCmd::White(o) => (Signal::White, o),
        GenCmd::Sine { freq, opts } => (Signal::Sine { freq: freq.0 }, opts),
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

fn signal_text(s: &GeneratorSettings) -> String {
    let sig = match s.signal {
        Signal::White => "white noise".to_owned(),
        Signal::Pink => "pink noise".to_owned(),
        Signal::PeriodicPink { period } => format!("periodic pink ({} samples)", period.0),
        Signal::Sine { freq } => format!("sine {}", ac2_scene::format::freq_readout(freq.0)),
        Signal::Ess { .. } => "sweep".to_owned(),
    };
    format!(
        "{sig} at {} on out {}",
        output::dbfs(s.level.0),
        channels_text(&s.outputs)
    )
}

#[derive(Debug)]
enum Input {
    Key(Key),
    Eof,
}

/// Enter / quit from the terminal (raw mode) or, when stdin is not a terminal, from lines:
/// an empty line is Enter, `q` or end of input is quit.
fn input() -> Result<(Option<RawTerm>, mpsc::UnboundedReceiver<Input>), CliError> {
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

fn say(out: &mut Out<'_>, raw: bool, event: &str, human: &str, extra: serde_json::Value) {
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

pub(crate) async fn run(cli: &Cli, cmd: &GenCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    let Some((settings, force)) = settings(cmd)? else {
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
            output::dbfs(settings.level.0),
            output::dbfs(ceiling.0)
        )));
    }
    if st.session.open.is_none() {
        return Err(CliError::Usage(
            "no open session; open one with `ac2 session open`".into(),
        ));
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
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    let mut firing = false;
    let outcome: Result<(), CliError> = loop {
        tokio::select! {
            _ = &mut ctrl_c => break Ok(()),
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
    use std::time::{Duration, Instant};

    use ac2_client::ClientConfig;
    use ac2_client::fake::{FakeDaemon, FakeOptions};
    use ac2_proto::units::Dbfs;

    use super::*;
    use crate::units::{Channels, LevelDbfs};

    type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

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

    #[tokio::test(flavor = "multi_thread")]
    async fn arm_enter_fires_quit_stops_and_releases() -> R {
        let f = FakeDaemon::start(FakeOptions::default())?;
        let c = Client::connect(ClientConfig::new(f.endpoints(), "gen-test")).await?;
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
            until("armed", || f.lock().state.generator.armed).await?;
            // Arming alone never emits.
            assert!(!f.lock().state.generator.firing);
            tx.send(Input::Key(Key::Enter))?;
            until("firing", || f.lock().state.generator.firing).await?;
            tx.send(Input::Key(Key::Quit))?;
            R::Ok(())
        };
        let (ran, drove) =
            tokio::join!(foreground(&c, lease, settings, &mut out, (None, rx)), drive);
        drove?;
        ran?;
        let g = f.lock().state.generator.clone();
        assert!(!g.firing && !g.armed && g.owner.is_none());
        assert_eq!(f.executions("gen.release"), 1);
        assert_eq!(f.executions("gen.stop"), 1);
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
