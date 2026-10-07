//! `ir capture` and `sweep run`: a sweep measurement run in the foreground
//! (`docs/design/sweep-distortion.md`, `docs/design/measurement-tree.md`); `ir metrics`: the
//! room parameters of a stored sweep (`docs/design/room-metrics.md`).
//!
//! `ir capture` takes the sweep's settings as flags and runs the sweep measurement with
//! exactly those settings, making one when there is none: the same command again is the
//! next run of the same measurement. `sweep run` runs a measurement made with `meas new
//! sweep`. Each run is stored under its measurement.
//!
//! Safety as `gen`: the level is a required typed dBFS value, checked against the daemon's
//! ceiling before the lease is acquired; the command acquires and *arms* with the sweep
//! (arming does not emit) and only Enter plays it. Esc, q, Ctrl-C or end of input before
//! Enter cancels; while it plays they stop the sweep (`gen.stop`), and the daemon discards
//! the run. If the process dies, the daemon's lease expiry stops the output and the run.

use ac2_client::{Client, LeaseLost, OnDrop, StimulusLease, expect_body};
use ac2_proto::model::{
    EssSpec, GeneratorDesired, GeneratorSettings, MeasConfig, MeasKind, Measurement, Signal, State,
    SweepConfig, SweepStatus, TraceData,
};
use ac2_proto::units::{Dbfs, Hz, Seconds, SweepId, TraceId};
use ac2_proto::{Command, ReplyBody};
use ac2_scene::distortion::{self, Reading};
use ac2_scene::room::{BandSet, room_table};
use ac2_scene::view::DistortionUnit;
use serde_json::json;
use tokio::sync::mpsc;

use super::gen_::{Input, input, say};
use super::{connect, find_meas, find_trace, state};
use crate::CliError;
use crate::args::{Cli, IrCaptureArgs, IrCmd, IrMetricsArgs, SweepCmd, SweepRunArgs};
use crate::output::{self, Out};
use crate::units::channels_text;
use crate::watch::{Key, RawTerm, quit_signal};

pub(crate) async fn run(cli: &Cli, cmd: &IrCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    match cmd {
        IrCmd::Capture(a) => capture(cli, a, out).await,
        IrCmd::Metrics(a) => metrics(cli, a, out).await,
    }
}

async fn metrics(cli: &Cli, a: &IrMetricsArgs, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let st = state(&c).await?;
    let t = find_trace(&st, &a.trace)?;
    let data = c.call(Command::TraceGet { trace: t.id }).await?;
    let data = expect_body!("trace.get", data, ReplyBody::TraceData(d) => d)?;
    let set = if a.third {
        BandSet::Third
    } else {
        BandSet::Octave
    };
    print_room(out, &data, set)
}

/// The room parameters of a sweep trace: the table, or `{trace, name, room}` as JSON (every
/// band of both sets, refusals tagged with their reason).
pub fn print_room(out: &mut Out<'_>, data: &TraceData, set: BandSet) -> Result<(), CliError> {
    let Some(s) = &data.sweep else {
        return Err(CliError::Usage(format!(
            "trace {} is not a sweep: room parameters come from a sweep's impulse response \
             (`ac2 ir capture`)",
            data.meta.id
        )));
    };
    let Some(r) = &s.room else {
        return Err(CliError::Refused(format!(
            "trace {} has no room parameters (imported from an export written without them)",
            data.meta.id
        )));
    };
    out.emit(
        &json!({ "trace": data.meta.id, "name": data.meta.edit.name, "room": r }),
        || room_table(r, set).text().trim_end().to_owned(),
    )?;
    Ok(())
}

/// A sweep's settings as typed (`meas new sweep`, `ir capture`).
pub struct SweepFlags {
    pub reference: u16,
    pub mic: u16,
    pub outputs: Vec<u16>,
    pub level: Dbfs,
    pub from: Hz,
    pub to: Hz,
    pub duration: Seconds,
    pub repeats: u8,
    pub gate: Option<Seconds>,
    pub tail: Option<Seconds>,
}

/// The sweep measurement settings of `f`, validated without a daemon.
pub fn sweep_config(f: &SweepFlags) -> Result<SweepConfig, CliError> {
    if f.from.0 >= f.to.0 {
        return Err(CliError::Usage("--from must be below --to".into()));
    }
    if f.duration.0 <= 0.0 {
        return Err(CliError::Usage("--duration must be positive".into()));
    }
    if f.outputs.is_empty() {
        return Err(CliError::Usage("--out needs at least one channel".into()));
    }
    if f.reference == f.mic {
        return Err(CliError::Usage(
            "the reference and the mic are the same input: the mic is the measurement".into(),
        ));
    }
    Ok(SweepConfig {
        reference_input: f.reference,
        measurement_input: f.mic,
        outputs: f.outputs.clone(),
        level: f.level,
        sweep: EssSpec::with_fades(f.from, f.to, f.duration),
        repeats: f.repeats,
        gate: f.gate,
        tail: f.tail,
    })
}

/// The settings `ir capture` runs with; `inputs` (reference, mic) are the typed ones or
/// those of the `--meas` transfer measurement, resolved by the caller.
pub fn request(a: &IrCaptureArgs, (reference, mic): (u16, u16)) -> Result<SweepConfig, CliError> {
    sweep_config(&SweepFlags {
        reference,
        mic,
        outputs: a.outputs.0.clone(),
        level: a.level.0,
        from: a.from.0,
        to: a.to.0,
        duration: a.duration.0,
        repeats: a.repeats,
        gate: a.gate.map(|g| g.0),
        tail: a.tail.map(|t| t.0),
    })
}

fn describe(r: &SweepConfig) -> String {
    format!(
        "sweep {} – {}, {} s × {} at {} on out {} · in {} re in {}",
        ac2_scene::format::freq_readout(r.sweep.start.0),
        ac2_scene::format::freq_readout(r.sweep.end.0),
        ac2_scene::format::fixed(r.sweep.duration.0, 1),
        r.repeats,
        output::dbfs(r.level.0),
        channels_text(&r.outputs),
        r.measurement_input + 1,
        r.reference_input + 1
    )
}

/// The sweep measurement `ir capture` runs: the one with exactly these settings (repeating
/// the command is a re-run of the same measurement, as Space is in the app's sweep view),
/// else a new one named `Sweep <n>`.
pub(crate) async fn sweep_meas_for(
    c: &Client,
    st: &State,
    config: &SweepConfig,
) -> Result<Measurement, CliError> {
    if let Some(m) = st
        .measurements
        .iter()
        .find(|m| matches!(&m.config.kind, MeasKind::Sweep { config: x } if x == config))
    {
        return Ok(m.clone());
    }
    let name = (1u32..)
        .map(|n| format!("Sweep {n}"))
        .find(|n| st.measurements.iter().all(|m| m.config.name != *n))
        .unwrap_or_else(|| "Sweep".into());
    super::basic::meas_call(
        c,
        Command::MeasCreate {
            config: MeasConfig {
                name,
                kind: MeasKind::Sweep {
                    config: config.clone(),
                },
            },
        },
    )
    .await
}

/// Checks before the lease is taken: the level against the daemon's ceiling, an open session.
fn check_runnable(st: &State, config: &SweepConfig) -> Result<(), CliError> {
    let ceiling = st.generator.ceiling;
    if config.level.0 > ceiling.0 {
        return Err(CliError::Refused(format!(
            "level {} is above the daemon's ceiling {}",
            output::dbfs(config.level.0),
            output::dbfs(ceiling.0)
        )));
    }
    if st.session.open.is_none() {
        return Err(CliError::Usage(
            "no open session; open one with `ac2 session open`".into(),
        ));
    }
    Ok(())
}

async fn capture(cli: &Cli, a: &IrCaptureArgs, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, true).await?;
    let st = state(&c).await?;
    let inputs = match (&a.meas, a.reference, a.mic) {
        (Some(m), _, _) => match &find_meas(&st, m)?.config.kind {
            MeasKind::Transfer { config } => (config.reference_input, config.measurement_input),
            _ => {
                return Err(CliError::Usage(format!(
                    "{} is not a transfer measurement",
                    m.0
                )));
            }
        },
        (None, Some(r), Some(m)) => (r.0, m.0),
        _ => return Err(CliError::Usage("give --ref and --mic, or --meas".into())),
    };
    let config = request(a, inputs)?;
    check_runnable(&st, &config)?;
    let m = sweep_meas_for(&c, &st, &config).await?;
    let lease = c.acquire_lease(a.force, OnDrop::StopAndRelease).await?;
    let keys = input()?;
    foreground(&c, lease, &m, a.name.clone(), out, keys).await
}

/// `sweep run`: a sweep measurement with its own settings.
async fn sweep_run(cli: &Cli, a: &SweepRunArgs, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, true).await?;
    let st = state(&c).await?;
    let m = find_meas(&st, &a.meas)?.clone();
    let MeasKind::Sweep { config } = &m.config.kind else {
        return Err(CliError::Usage(format!(
            "{} is not a sweep measurement (`ac2 meas new sweep …` makes one)",
            m.config.name
        )));
    };
    check_runnable(&st, config)?;
    let lease = c.acquire_lease(a.force, OnDrop::StopAndRelease).await?;
    let keys = input()?;
    foreground(&c, lease, &m, a.name.clone(), out, keys).await
}

/// `sweep …`.
pub(crate) async fn sweep(cli: &Cli, cmd: &SweepCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    match cmd {
        SweepCmd::Run(a) => sweep_run(cli, a, out).await,
    }
}

/// How a run ended.
enum End {
    /// Quit before Enter: nothing played.
    Cancelled,
    Stored(TraceId),
}

async fn foreground(
    c: &Client,
    mut lease: StimulusLease,
    meas: &Measurement,
    name: Option<String>,
    out: &mut Out<'_>,
    (term, mut rx): (Option<RawTerm>, mpsc::UnboundedReceiver<Input>),
) -> Result<(), CliError> {
    let MeasKind::Sweep { config: req } = &meas.config.kind else {
        return Err(CliError::Usage(format!(
            "{} is not a sweep measurement",
            meas.config.name
        )));
    };
    let raw = term.is_some();
    let settings = GeneratorSettings {
        signal: Signal::Ess { sweep: req.sweep },
        level: req.level,
        band: None,
        outputs: req.outputs.clone(),
    };
    lease
        .set(GeneratorDesired {
            settings,
            armed: true,
            firing: false,
        })
        .await?;
    say(
        out,
        raw,
        "armed",
        &format!(
            "ARMED  {} · {}\nEnter: play the sweep   Esc, q, Ctrl-C: cancel",
            meas.config.name,
            describe(req)
        ),
        json!({ "meas": meas.id, "settings": req, "client_id": c.client_id() }),
    );
    let mut watch = c.watch();
    let quit = quit_signal();
    tokio::pin!(quit);
    let mut run: Option<(SweepId, u8)> = None;
    let mut last: Option<SweepStatus> = None;
    let outcome: Result<End, CliError> = loop {
        tokio::select! {
            _ = &mut quit => break stop(c, run.is_some(), out, raw).await,
            lost = lease.wait_lost() => {
                let LeaseLost::Refused { msg, .. } = &lost;
                say(out, raw, "lost", &format!("LEASE LOST: {msg}"), json!({ "reason": msg }));
                break Err(CliError::Refused(format!("stimulus lease lost: {msg}")));
            }
            i = rx.recv() => match i {
                None | Some(Input::Eof) | Some(Input::Key(Key::Quit)) => {
                    break stop(c, run.is_some(), out, raw).await;
                }
                Some(Input::Key(Key::Enter)) if run.is_none() => {
                    let r = c
                        .call(Command::SweepRun {
                            lease_token: lease.token(),
                            meas: meas.id,
                            name: name.clone(),
                        })
                        .await
                        .and_then(|r| expect_body!("sweep.run", r, ReplyBody::Sweep(s) => s));
                    match r {
                        Ok(s) => {
                            run = Some((s.id, s.repeats));
                            say(
                                out,
                                raw,
                                "playing",
                                &format!(
                                    "PLAYING sweep 1/{} ({:.1} s) · Esc stops",
                                    s.repeats,
                                    s.total().0
                                ),
                                json!({ "run": s.id, "repeat": 1, "repeats": s.repeats }),
                            );
                        }
                        Err(e) => break Err(e.into()),
                    }
                }
                Some(Input::Key(Key::Enter)) => {}
            },
            changed = watch.changed(), if run.is_some() => {
                if changed.is_err() {
                    break Err(CliError::Refused("the connection to the daemon closed".into()));
                }
                let view = watch.borrow_and_update().clone();
                let Some(r) = view.state.as_ref().and_then(|s| s.sweep.clone()) else {
                    continue;
                };
                let Some((id, repeats)) = run else { continue };
                if r.id != id || last.as_ref() == Some(&r.status) {
                    continue;
                }
                last = Some(r.status.clone());
                match r.status {
                    SweepStatus::Playing { repeat } if repeat > 1 => say(
                        out,
                        raw,
                        "playing",
                        &format!("PLAYING sweep {repeat}/{repeats}"),
                        json!({ "run": id, "repeat": repeat, "repeats": repeats }),
                    ),
                    SweepStatus::Playing { .. } => {}
                    SweepStatus::Analysing => say(
                        out,
                        raw,
                        "analysing",
                        "ANALYSING",
                        json!({ "run": id }),
                    ),
                    SweepStatus::Done { trace } => break Ok(End::Stored(trace)),
                    SweepStatus::Failed { reason, msg } => {
                        say(
                            out,
                            raw,
                            "failed",
                            &format!("FAILED: {msg}"),
                            json!({ "run": id, "reason": reason, "msg": msg }),
                        );
                        break Err(CliError::Refused(format!("sweep failed: {msg}")));
                    }
                }
            }
        }
    };
    let end = lease.end().await;
    drop(term);
    let trace = match outcome? {
        End::Cancelled => {
            say(
                out,
                false,
                "cancelled",
                "cancelled: nothing played, lease released",
                json!({}),
            );
            return end.map_err(CliError::from);
        }
        End::Stored(t) => t,
    };
    end.map_err(CliError::from)?;
    let data = c.call(Command::TraceGet { trace }).await?;
    let data = expect_body!("trace.get", data, ReplyBody::TraceData(d) => d)?;
    let grid = c.grid(data.meta.grid_id).await?;
    let freqs = ac2_scene::grid::column_frequencies(&grid);
    print_summary(out, &data, &freqs);
    Ok(())
}

/// Stops a playing run (the daemon discards it); before Enter there is nothing to stop.
async fn stop(c: &Client, playing: bool, out: &mut Out<'_>, raw: bool) -> Result<End, CliError> {
    if !playing {
        return Ok(End::Cancelled);
    }
    let _ = c.call(Command::GenStop).await;
    say(
        out,
        raw,
        "stopped",
        "STOPPED: the sweep was discarded",
        json!({}),
    );
    Err(CliError::Refused("sweep stopped; nothing stored".into()))
}

fn reading_json(r: Reading) -> serde_json::Value {
    match r {
        Reading::NotMeasured => json!({ "measured": false }),
        Reading::BelowFloor(f) => json!({ "below_floor": true, "floor_db": f }),
        Reading::Level(v) => json!({ "db": v, "percent": distortion::percent(v) }),
    }
}

/// The summary of a stored sweep: THD at 100 Hz / 1 kHz / 10 kHz, each order's highest
/// valid point, the arrival and the reference level.
pub fn print_summary(out: &mut Out<'_>, data: &TraceData, freqs: &[f64]) {
    let Some(s) = &data.sweep else {
        return;
    };
    let sum = distortion::summary(s, freqs);
    let unit = DistortionUnit::Db;
    if out.json {
        let j = json!({
            "event": "done",
            "trace": data.meta.id,
            "name": data.meta.edit.name,
            "arrival_ms": s.info.arrival.0 * 1000.0,
            "reference_db": s.info.reference_level.0,
            "repeats": s.info.repeats,
            "clipped": s.info.clipped,
            "thd": sum.thd.iter().map(|(f, r)| {
                let mut v = reading_json(*r);
                v["hz"] = json!(f);
                v
            }).collect::<Vec<_>>(),
            "max": sum.peaks.iter().map(|(k, p)| json!({
                "order": k,
                "hz": p.map(|x| x.0),
                "db": p.map(|x| x.1),
            })).collect::<Vec<_>>(),
            "room": s.room,
        });
        let _ = out.json_line(&j);
        return;
    }
    let mut t = format!(
        "DONE   trace {} {:?}: arrival {} · reference {} · {} × {} s{}\n",
        data.meta.id,
        data.meta.edit.name,
        ac2_scene::format::ms(s.info.arrival.0, 2),
        ac2_scene::format::db_readout(s.info.reference_level.0),
        s.info.repeats,
        ac2_scene::format::fixed(s.info.duration.0, 2),
        if s.info.clipped {
            " · CLIPPED: lower the level"
        } else {
            ""
        }
    );
    for (f, r) in &sum.thd {
        let _ = std::fmt::Write::write_fmt(
            &mut t,
            format_args!(
                "  THD at {:>9}  {:>12}  {}\n",
                ac2_scene::format::freq_readout(*f),
                distortion::reading_text(*r, unit),
                match r {
                    Reading::Level(v) => distortion::percent_text(*v),
                    _ => String::new(),
                }
            ),
        );
    }
    for (k, p) in &sum.peaks {
        let line = match p {
            Some((f, v)) => format!(
                "  max {:<3}         {:>12}  {} at {}\n",
                distortion::order_name(*k),
                distortion::db_text(*v),
                distortion::percent_text(*v),
                ac2_scene::format::freq_readout(*f)
            ),
            None => format!(
                "  max {:<3}         below the noise floor everywhere\n",
                distortion::order_name(*k)
            ),
        };
        t.push_str(&line);
    }
    if let Some(r) = &s.room {
        for l in room_table(r, BandSet::Octave).text().lines() {
            t.push_str(&format!("  {l}\n"));
        }
    }
    let _ = std::fmt::Write::write_fmt(
        &mut t,
        format_args!(
            "  every curve: ac2 trace export {} --csv FILE\n  \
             one-third octaves: ac2 ir metrics {} --third",
            data.meta.id, data.meta.id
        ),
    );
    let _ = writeln!(out.w, "{t}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::Out;
    use crate::units::{Channel, Channels, Freq, LevelDbfs, Time};

    fn args() -> IrCaptureArgs {
        IrCaptureArgs {
            reference: Some(Channel(1)),
            mic: Some(Channel(0)),
            meas: None,
            outputs: Channels(vec![0, 1]),
            level: LevelDbfs(Dbfs(-50.0)),
            from: Freq(Hz(20.0)),
            to: Freq(Hz(20_000.0)),
            duration: Time(Seconds(3.0)),
            repeats: 1,
            gate: None,
            tail: None,
            name: None,
            force: false,
        }
    }

    #[test]
    fn the_request_carries_the_typed_values() {
        let r = request(&args(), (1, 0)).expect("request");
        assert_eq!((r.reference_input, r.measurement_input), (1, 0));
        assert_eq!(r.level, Dbfs(-50.0));
        assert_eq!(r.outputs, vec![0, 1]);
        assert_eq!(
            r.sweep,
            EssSpec::with_fades(Hz(20.0), Hz(20_000.0), Seconds(3.0))
        );
        assert_eq!(
            describe(&r),
            "sweep 20.0 Hz – 20.0 kHz, 3.0 s × 1 at −50.0 dBFS on out 1,2 · in 1 re in 2"
        );
        let mut a = args();
        a.from = Freq(Hz(30_000.0));
        assert!(request(&a, (1, 0)).is_err());
        assert!(request(&args(), (1, 1)).is_err());
    }

    type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

    async fn ac2(ep: &ac2_client::Endpoints, args: &[&str]) -> R<(u8, String)> {
        use clap::Parser;
        let argv: Vec<&str> = [
            "ac2",
            "--ctrl-endpoint",
            &ep.ctrl,
            "--data-endpoint",
            &ep.data,
        ]
        .into_iter()
        .chain(args.iter().copied())
        .collect();
        let cli = Cli::try_parse_from(argv)?;
        let (mut so, mut se) = (Vec::new(), Vec::new());
        let code = {
            let mut out = Out::new(cli.json, &mut so);
            crate::run_reporting(&cli, &mut out, &mut se).await
        };
        Ok((code, String::from_utf8(so)? + &String::from_utf8_lossy(&se)))
    }

    /// From an empty daemon on the simulated rig: open the session with the CLI, capture a
    /// sweep (Enter plays it), read the summary, export every curve.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ir_capture_on_the_simulated_rig() -> R {
        use ac2_client::{ClientConfig, Endpoints};
        use ac2d::{BackendChoice, Daemon, DaemonConfig, Listen};
        let backend = ac2d::backend(BackendChoice::Fake)?;
        let listen = Listen::Local {
            ctrl: "tcp://127.0.0.1:0".into(),
            data: "tcp://127.0.0.1:0".into(),
        };
        let h = Daemon::start(DaemonConfig::new(backend, listen, -10.0))?;
        let ep = Endpoints {
            ctrl: h.ctrl_endpoint().to_owned(),
            data: h.data_endpoint().to_owned(),
        };
        let (code, text) = ac2(
            &ep,
            &[
                "session",
                "open",
                "--backend",
                "fake",
                "--in",
                "1-3",
                "--loopback-out",
                "1",
                "--loopback-in",
                "1",
            ],
        )
        .await?;
        assert_eq!(code, 0, "{text}");

        let mut cfg = ClientConfig::new(ep.clone(), "ir-capture-test");
        cfg.mirror = true;
        let c = Client::connect(cfg).await?;
        c.wait_synced(std::time::Duration::from_secs(5)).await?;
        // The rig: out 1 → in 1 loopback, out 1 → in 2 through the distorting "speaker".
        let mut a = args();
        a.reference = Some(Channel(0));
        a.mic = Some(Channel(1));
        a.outputs = Channels(vec![0]);
        a.level = LevelDbfs(Dbfs(-20.0));
        a.from = Freq(Hz(100.0));
        a.to = Freq(Hz(5000.0));
        a.duration = Time(Seconds(1.0));
        a.name = Some("rig sweep".into());
        let req = request(&a, (0, 1))?;
        let st = c.snapshot().await?.state;
        let meas = sweep_meas_for(&c, &st, &req).await?;
        assert_eq!(meas.config.name, "Sweep 1");
        let lease = c.acquire_lease(false, OnDrop::StopAndRelease).await?;
        let (tx, rx) = mpsc::unbounded_channel();
        let mut buf = Vec::new();
        let mut out = Out::new(true, &mut buf);
        tx.send(Input::Key(Key::Enter))?;
        foreground(&c, lease, &meas, a.name.clone(), &mut out, (None, rx)).await?;
        let lines: Vec<serde_json::Value> = String::from_utf8(buf)?
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        let events: Vec<&str> = lines.iter().filter_map(|v| v["event"].as_str()).collect();
        assert_eq!(events.first(), Some(&"armed"));
        assert!(events.contains(&"playing") && events.contains(&"analysing"));
        let done = lines.last().ok_or("no output")?;
        assert_eq!(done["event"], "done", "{lines:?}");
        assert_eq!(done["name"], "rig sweep");
        // The fake rig's driver: H2 −40 dB, H3 −50 dB at −20 dBFS.
        let max = |k: u64| {
            done["max"]
                .as_array()
                .and_then(|m| m.iter().find(|x| x["order"] == k))
                .and_then(|x| x["db"].as_f64())
                .unwrap_or(f64::NAN)
        };
        assert!((max(2) + 40.0).abs() < 1.0, "H2 {}", max(2));
        // H3 sits near the rig's noise (about −76 dBFS against −80 dBFS of noise), so its
        // per-frequency estimate scatters and the maximum over the sweep is biased up (−49.0
        // on Linux, −48.9 on macOS); the level itself is checked as the median below.
        assert!((max(3) + 50.0).abs() < 2.0, "H3 {}", max(3));
        let thd_1k = &done["thd"][1];
        assert_eq!(thd_1k["hz"], 1000.0);
        assert!((thd_1k["db"].as_f64().unwrap_or(f64::NAN) + 39.6).abs() < 1.0);
        // The human summary of the same trace.
        let id: u32 = done["trace"].as_u64().ok_or("trace id")?.try_into()?;
        let data = c.call(Command::TraceGet { trace: TraceId(id) }).await?;
        let ReplyBody::TraceData(data) = data else {
            return Err("trace data".into());
        };
        let freqs = ac2_scene::grid::column_frequencies(&*c.grid(data.meta.grid_id).await?);
        // The rig's H3 is c3·A²/4 re the fundamental (A = 0.1 at −20 dBFS, the fundamental
        // itself gaining 3/4·c3·A²): −50.08 dB at every frequency. Its median over the band
        // where H3 lies inside the sweep (3·f ≤ 5 kHz) reads it without the noise's bias.
        let sweep = data.sweep.as_ref().ok_or("sweep data")?;
        let h3 = &sweep
            .harmonics
            .iter()
            .find(|h| h.order == 3)
            .ok_or("H3 curve")?
            .curve
            .level_db;
        let mut band: Vec<f64> = freqs
            .iter()
            .zip(h3)
            .filter(|(f, l)| (200.0..=1500.0).contains(*f) && l.is_finite())
            .map(|(_, l)| f64::from(*l))
            .collect();
        band.sort_by(f64::total_cmp);
        assert!(band.len() > 20, "{} H3 columns", band.len());
        let median = band[band.len() / 2];
        assert!((median + 50.08).abs() < 0.5, "H3 median {median}");
        let mut human = Vec::new();
        print_summary(&mut Out::new(false, &mut human), &data, &freqs);
        let human = String::from_utf8(human)?;
        eprintln!("{human}");
        assert!(human.starts_with("DONE   trace "), "{human}");
        assert!(human.contains("THD at  1.00 kHz"), "{human}");
        assert!(human.contains("THD at  10.0 kHz             —"), "{human}");
        assert!(human.contains("max H2 "), "{human}");
        // Released and disarmed.
        let st = c.snapshot().await?.state;
        assert!(st.generator.owner.is_none() && !st.generator.armed);

        let file = tempfile::NamedTempFile::new()?;
        let path = file.path().to_string_lossy().into_owned();
        let trace = done["trace"].to_string();
        let (code, text) = ac2(&ep, &["trace", "export", &trace, "--csv", &path]).await?;
        assert_eq!(code, 0, "{text}");
        let csv = std::fs::read_to_string(&path)?;
        assert!(csv.contains("# kind: sweep"));
        assert!(csv.contains(",h2_db,h2_floor_db,h3_db,h3_floor_db,h4_db,h4_floor_db,h5_db,h5_floor_db,thd_db,thd_floor_db"));
        assert!(csv.contains("\n# room_metrics: {"));

        // The rig's hall (in 3, T60 0.8 s): a sweep with 2 s of silence after it, then its
        // room parameters by name, as a table and as JSON.
        let mut a = args();
        a.reference = Some(Channel(0));
        a.mic = Some(Channel(2));
        a.outputs = Channels(vec![0]);
        a.level = LevelDbfs(Dbfs(-20.0));
        a.from = Freq(Hz(100.0));
        a.to = Freq(Hz(10_000.0));
        a.duration = Time(Seconds(1.0));
        a.tail = Some(Time(Seconds(2.0)));
        a.name = Some("hall".into());
        let req = request(&a, (0, 2))?;
        assert_eq!(req.tail, Some(Seconds(2.0)));
        let st = c.snapshot().await?.state;
        let hall = sweep_meas_for(&c, &st, &req).await?;
        assert_eq!(hall.config.name, "Sweep 2");
        let lease = c.acquire_lease(false, OnDrop::StopAndRelease).await?;
        let (tx, rx) = mpsc::unbounded_channel();
        let mut buf = Vec::new();
        let mut out = Out::new(true, &mut buf);
        tx.send(Input::Key(Key::Enter))?;
        foreground(&c, lease, &hall, a.name.clone(), &mut out, (None, rx)).await?;
        let (code, text) = ac2(&ep, &["ir", "metrics", "hall"]).await?;
        assert_eq!(code, 0, "{text}");
        eprintln!("{text}");
        assert!(
            text.starts_with("Room (ISO 3382-1) · octave bands · decay to "),
            "{text}"
        );
        assert!(text.contains("\nT30 (s)"), "{text}");
        let (code, text) = ac2(&ep, &["--json", "ir", "metrics", "hall"]).await?;
        assert_eq!(code, 0, "{text}");
        let j: serde_json::Value = serde_json::from_str(&text)?;
        let t30_1k = j["room"]["octave"]
            .as_array()
            .and_then(|b| b.iter().find(|b| b["centre"] == 1000.0))
            .map(|b| b["t30"].clone())
            .ok_or("no 1 kHz band")?;
        assert_eq!(t30_1k["type"], "value", "{t30_1k}");
        let t30 = t30_1k["value"].as_f64().unwrap_or(f64::NAN);
        assert!((t30 / 0.8 - 1.0).abs() < 0.1, "T30 at 1 kHz {t30}");
        let (code, text) = ac2(&ep, &["ir", "metrics", "rig sweep", "--third"]).await?;
        assert_eq!(code, 0, "{text}");
        assert!(text.contains("⅓-octave bands"), "{text}");
        drop(c);
        h.shutdown();
        Ok(())
    }
}
