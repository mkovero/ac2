//! `ir capture`: a sweep measurement in the foreground (`docs/design/sweep-distortion.md`).
//!
//! Safety as `gen`: the level is a required typed dBFS value, checked against the daemon's
//! ceiling before the lease is acquired; the command acquires and *arms* with the sweep
//! (arming does not emit) and only Enter plays it. Esc, q, Ctrl-C or end of input before
//! Enter cancels; while it plays they stop the sweep (`gen.stop`), and the daemon discards
//! the run. If the process dies, the daemon's lease expiry stops the output and the run.

use ac2_client::{Client, LeaseLost, OnDrop, StimulusLease, expect_body};
use ac2_proto::model::{
    EssSpec, GeneratorDesired, GeneratorSettings, Signal, SweepInputs, SweepRequest, SweepStatus,
    TraceData,
};
use ac2_proto::units::{SweepId, TraceId};
use ac2_proto::{Command, ReplyBody};
use ac2_scene::distortion::{self, Reading};
use ac2_scene::view::DistortionUnit;
use serde_json::json;
use tokio::sync::mpsc;

use super::gen_::{Input, input, say};
use super::{connect, find_meas, state};
use crate::CliError;
use crate::args::{Cli, IrCaptureArgs, IrCmd};
use crate::output::{self, Out};
use crate::units::channels_text;
use crate::watch::{Key, RawTerm, quit_signal};

pub(crate) async fn run(cli: &Cli, cmd: &IrCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    match cmd {
        IrCmd::Capture(a) => capture(cli, a, out).await,
    }
}

/// The request `ir capture` sends, validated without a daemon; inputs from `--meas` are
/// resolved by the caller.
pub fn request(a: &IrCaptureArgs) -> Result<SweepRequest, CliError> {
    let (from, to) = (a.from.0, a.to.0);
    if from.0 >= to.0 {
        return Err(CliError::Usage("--from must be below --to".into()));
    }
    if a.duration.0.0 <= 0.0 {
        return Err(CliError::Usage("--duration must be positive".into()));
    }
    if a.outputs.0.is_empty() {
        return Err(CliError::Usage("--out needs at least one channel".into()));
    }
    let inputs = match (a.reference, a.mic) {
        (Some(r), Some(m)) if r == m => {
            return Err(CliError::Usage(
                "--ref and --mic are the same input: the mic is the measurement".into(),
            ));
        }
        (Some(r), Some(m)) => SweepInputs::Channels {
            reference: r.0,
            measurement: m.0,
        },
        // Resolved against the daemon's measurements.
        _ => SweepInputs::Channels {
            reference: 0,
            measurement: 0,
        },
    };
    Ok(SweepRequest {
        inputs,
        outputs: a.outputs.0.clone(),
        level: Some(a.level.0),
        sweep: EssSpec::with_fades(from, to, a.duration.0),
        repeats: a.repeats,
        gate: a.gate.map(|g| g.0),
    })
}

fn describe(r: &SweepRequest) -> String {
    let inputs = match r.inputs {
        SweepInputs::Channels {
            reference,
            measurement,
        } => format!("in {} re in {}", measurement + 1, reference + 1),
        SweepInputs::Measurement { meas } => format!("inputs of measurement {meas}"),
    };
    format!(
        "sweep {} – {}, {} s × {} at {} on out {} · {inputs}",
        ac2_scene::format::freq_readout(r.sweep.start.0),
        ac2_scene::format::freq_readout(r.sweep.end.0),
        ac2_scene::format::fixed(r.sweep.duration.0, 1),
        r.repeats,
        output::dbfs(r.level.map_or(f64::NAN, |l| l.0)),
        channels_text(&r.outputs)
    )
}

async fn capture(cli: &Cli, a: &IrCaptureArgs, out: &mut Out<'_>) -> Result<(), CliError> {
    let mut req = request(a)?;
    let c = connect(cli, true).await?;
    let st = state(&c).await?;
    if let Some(m) = &a.meas {
        req.inputs = SweepInputs::Measurement {
            meas: find_meas(&st, m)?.id,
        };
    }
    let ceiling = st.generator.ceiling;
    if a.level.0.0 > ceiling.0 {
        return Err(CliError::Refused(format!(
            "level {} is above the daemon's ceiling {}",
            output::dbfs(a.level.0.0),
            output::dbfs(ceiling.0)
        )));
    }
    if st.session.open.is_none() {
        return Err(CliError::Usage(
            "no open session; open one with `ac2 session open`".into(),
        ));
    }
    let lease = c.acquire_lease(a.force, OnDrop::StopAndRelease).await?;
    let keys = input()?;
    foreground(&c, lease, req, a.name.clone(), out, keys).await
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
    req: SweepRequest,
    name: String,
    out: &mut Out<'_>,
    (term, mut rx): (Option<RawTerm>, mpsc::UnboundedReceiver<Input>),
) -> Result<(), CliError> {
    let raw = term.is_some();
    let settings = GeneratorSettings {
        signal: Signal::Ess { sweep: req.sweep },
        level: req
            .level
            .ok_or_else(|| CliError::Usage("--level is required".into()))?,
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
            "ARMED  {}\nEnter: play the sweep   Esc, q, Ctrl-C: cancel",
            describe(&req)
        ),
        json!({ "request": req, "client_id": c.client_id() }),
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
                        .call(Command::IrCapture {
                            lease_token: lease.token(),
                            request: req.clone(),
                            name: name.clone(),
                        })
                        .await
                        .and_then(|r| expect_body!("ir.capture", r, ReplyBody::Sweep(s) => s));
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
    let _ = std::fmt::Write::write_fmt(
        &mut t,
        format_args!(
            "  every curve: ac2 trace export {} --csv FILE",
            data.meta.id
        ),
    );
    let _ = writeln!(out.w, "{t}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::Out;
    use crate::units::{Channel, Channels, Freq, LevelDbfs, Time};
    use ac2_proto::units::{Dbfs, Hz, Seconds};

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
            name: "sweep".into(),
            force: false,
        }
    }

    #[test]
    fn the_request_carries_the_typed_values() {
        let r = request(&args()).expect("request");
        assert_eq!(
            r.inputs,
            SweepInputs::Channels {
                reference: 1,
                measurement: 0
            }
        );
        assert_eq!(r.level, Some(Dbfs(-50.0)));
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
        assert!(request(&a).is_err());
        let mut a = args();
        a.mic = Some(Channel(1));
        assert!(request(&a).is_err());
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
                "1-2",
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
        a.name = "rig sweep".into();
        let req = request(&a)?;
        let lease = c.acquire_lease(false, OnDrop::StopAndRelease).await?;
        let (tx, rx) = mpsc::unbounded_channel();
        let mut buf = Vec::new();
        let mut out = Out::new(true, &mut buf);
        tx.send(Input::Key(Key::Enter))?;
        foreground(&c, lease, req, a.name.clone(), &mut out, (None, rx)).await?;
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
        assert!((max(3) + 50.0).abs() < 1.0, "H3 {}", max(3));
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
        drop(c);
        h.shutdown();
        Ok(())
    }
}
