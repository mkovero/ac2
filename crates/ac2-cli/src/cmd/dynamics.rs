//! `ac2 dynamics run`: plays a dynamics measurement in the foreground under the stimulus
//! lease, follows its progress, and prints the stored trace's step table.

use std::fmt::Write as _;

use ac2_client::{Client, LeaseLost, OnDrop, StimulusLease, expect_body};
use ac2_core::dynamics::{Onset, Programme, ProgrammeConfig};
use ac2_proto::model::{
    DynamicsConfig, DynamicsData, DynamicsLevel, DynamicsOnset, DynamicsPhase, DynamicsStatus,
    DynamicsTime, GeneratorDesired, GeneratorSettings, MeasKind, Measurement, Signal, State,
};
use ac2_proto::units::{Dbfs, DynamicsId, Hz, TraceId};
use ac2_proto::{Command, ReplyBody};
use serde_json::json;
use tokio::sync::mpsc;

use super::gen_::{Input, input, say};
use super::{connect, find_meas, find_trace, state};
use crate::CliError;
use crate::args::{Cli, DynamicsCmd, DynamicsRunArgs};
use crate::output::Out;
use crate::watch::{Key, RawTerm, quit_signal};

/// `dynamics …`.
pub(crate) async fn run(cli: &Cli, cmd: &DynamicsCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    match cmd {
        DynamicsCmd::Run(a) => dynamics_run(cli, a, out).await,
    }
}

/// What the run will play at the session's rate: its highest level and its longest run
/// time, for the confirmation at arm.
struct Plan {
    carrier: f64,
    top: f64,
    longest: f64,
    steps: usize,
}

fn plan(st: &State, c: &DynamicsConfig, replay: Option<Vec<f64>>) -> Result<Plan, CliError> {
    let Some(open) = &st.session.open else {
        return Err(CliError::Usage(
            "no audio I/O open; open it with `ac2 session open`".into(),
        ));
    };
    let p = Programme::new(
        &ProgrammeConfig {
            carrier_hz: c.carrier.0,
            onset: match c.onset {
                DynamicsOnset::Zero => Onset::Zero,
                DynamicsOnset::Peak => Onset::Peak,
            },
            pilot_dbfs: c.pilot.0,
            bursts_dbfs: c.bursts.iter().map(|b| b.0).collect(),
            durations_s: c.durations.iter().map(|d| d.0).collect(),
            rest_max_s: c.rest_max.0,
            replay_rests_s: replay,
        },
        f64::from(open.sample_rate_hz),
    )
    .map_err(|e| CliError::Usage(e.to_string()))?;
    let top = p.highest_dbfs();
    let ceiling = st.generator.ceiling;
    if top > ceiling.0 {
        return Err(CliError::Refused(format!(
            "level {} is above the daemon's ceiling {}",
            ac2_scene::format::dbfs(top),
            ac2_scene::format::dbfs(ceiling.0)
        )));
    }
    Ok(Plan {
        carrier: p.carrier_hz(),
        top,
        longest: p.longest_seconds(),
        steps: p.steps(),
    })
}

async fn dynamics_run(cli: &Cli, a: &DynamicsRunArgs, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, true).await?;
    let st = state(&c).await?;
    let m = find_meas(&st, &a.meas)?.clone();
    let MeasKind::Dynamics { config } = &m.config.kind else {
        return Err(CliError::Usage(format!(
            "{} is not a dynamics measurement (`ac2 meas new dynamics …` makes one)",
            m.config.name
        )));
    };
    let (rests_from, replay) = match &a.rests_from {
        None => (None, None),
        Some(r) => {
            let t = find_trace(&st, r)?.id;
            let d = dynamics_data(&c, t).await?;
            (
                Some(t),
                Some(d.info.steps.iter().map(|s| s.rest.0).collect()),
            )
        }
    };
    let plan = plan(&st, config, replay)?;
    let lease = c.acquire_lease(a.force, OnDrop::StopAndRelease).await?;
    let keys = input()?;
    foreground(
        &c,
        lease,
        &m,
        config,
        &plan,
        a.name.clone(),
        rests_from,
        out,
        keys,
    )
    .await
}

async fn dynamics_data(c: &Client, trace: TraceId) -> Result<Box<DynamicsData>, CliError> {
    let data = c.call(Command::TraceGet { trace }).await?;
    let data = expect_body!("trace.get", data, ReplyBody::TraceData(d) => d)?;
    data.dynamics
        .ok_or_else(|| CliError::Usage(format!("trace {trace} is not a dynamics trace")))
}

#[expect(
    clippy::too_many_arguments,
    reason = "one call site, every argument distinct"
)]
async fn foreground(
    c: &Client,
    mut lease: StimulusLease,
    meas: &Measurement,
    config: &DynamicsConfig,
    plan: &Plan,
    name: Option<String>,
    rests_from: Option<TraceId>,
    out: &mut Out<'_>,
    (term, mut rx): (Option<RawTerm>, mpsc::UnboundedReceiver<Input>),
) -> Result<(), CliError> {
    let raw = term.is_some();
    // Armed as the carrier at the highest level: the loudest the programme plays.
    lease
        .set(GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Sine {
                    freq: Hz(plan.carrier),
                },
                level: Dbfs(plan.top),
                band: None,
                outputs: config.outputs.clone(),
            },
            armed: true,
            firing: false,
        })
        .await?;
    say(
        out,
        raw,
        "armed",
        &format!(
            "ARMED  {} · {} steps at {:.2} Hz, up to {} · at most {}\n\
             Enter: play the programme   Esc, q, Ctrl-C: cancel",
            meas.config.name,
            plan.steps,
            plan.carrier,
            ac2_scene::format::dbfs(plan.top),
            minutes(plan.longest),
        ),
        json!({
            "meas": meas.id,
            "settings": config,
            "carrier_hz": plan.carrier,
            "highest_dbfs": plan.top,
            "longest_s": plan.longest,
            "client_id": c.client_id(),
        }),
    );
    let mut watch = c.watch();
    let quit = quit_signal();
    tokio::pin!(quit);
    let mut run: Option<DynamicsId> = None;
    let mut last: Option<DynamicsStatus> = None;
    let outcome: Result<Option<TraceId>, CliError> = loop {
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
                        .call(Command::DynamicsRun(ac2_proto::DynamicsRunArgs {
                            lease_token: lease.token(),
                            meas: meas.id,
                            name: name.clone(),
                            rests_from,
                        }))
                        .await
                        .and_then(|r| expect_body!("dynamics.run", r, ReplyBody::Dynamics(s) => s));
                    match r {
                        Ok(s) => {
                            run = Some(s.id);
                            say(
                                out,
                                raw,
                                "playing",
                                &format!("PLAYING (at most {}) · Esc stops", minutes(s.longest.0)),
                                json!({ "run": s.id, "longest_s": s.longest.0 }),
                            );
                        }
                        Err(e) => break Err(e.into()),
                    }
                }
                Some(Input::Key(Key::Enter)) => {}
            },
            silent = c.await_dead() => break Err(crate::watch::dead(silent)),
            changed = watch.changed(), if run.is_some() => {
                if changed.is_err() {
                    break Err(CliError::Refused("the connection to the daemon closed".into()));
                }
                let view = watch.borrow_and_update().clone();
                let Some(r) = view.state.as_ref().and_then(|s| s.dynamics.clone()) else {
                    continue;
                };
                if Some(r.id) != run || last.as_ref() == Some(&r.status) {
                    continue;
                }
                last = Some(r.status.clone());
                match r.status {
                    DynamicsStatus::Playing { step, steps, phase } => {
                        let what = match phase {
                            DynamicsPhase::Waiting => "waiting for the reference".to_owned(),
                            DynamicsPhase::Check => "linearity check".to_owned(),
                            DynamicsPhase::Burst => format!("step {step}/{steps} burst"),
                            DynamicsPhase::Rest => format!("step {step}/{steps} rest"),
                        };
                        say(
                            out,
                            raw,
                            "progress",
                            &format!("PLAYING {what}"),
                            json!({ "run": r.id, "step": step, "steps": steps, "phase": phase }),
                        );
                    }
                    DynamicsStatus::Done { trace } => break Ok(Some(trace)),
                    DynamicsStatus::Failed { reason, msg } => {
                        say(
                            out,
                            raw,
                            "failed",
                            &format!("FAILED: {msg}"),
                            json!({ "run": r.id, "reason": reason, "msg": msg }),
                        );
                        break Err(CliError::Refused(format!("dynamics run failed: {msg}")));
                    }
                }
            }
        }
    };
    let end = lease.end().await;
    drop(term);
    let Some(trace) = outcome? else {
        say(
            out,
            false,
            "cancelled",
            "cancelled: nothing played, lease released",
            json!({}),
        );
        return end.map_err(CliError::from);
    };
    end.map_err(CliError::from)?;
    let d = dynamics_data(c, trace).await?;
    if out.json {
        let _ = out.json_line(&json!({ "event": "stored", "trace": trace, "dynamics": d }));
    } else {
        let _ = write!(out.w, "{}", step_table(trace, &d));
    }
    Ok(())
}

/// Stops a playing run (the daemon discards it); before Enter there is nothing to stop.
async fn stop(
    c: &Client,
    playing: bool,
    out: &mut Out<'_>,
    raw: bool,
) -> Result<Option<TraceId>, CliError> {
    if !playing {
        return Ok(None);
    }
    let _ = c.call(Command::GenStop).await;
    say(
        out,
        raw,
        "stopped",
        "STOPPED: the run was discarded",
        json!({}),
    );
    Err(CliError::Refused(
        "dynamics run stopped; nothing stored".into(),
    ))
}

fn minutes(s: f64) -> String {
    if s < 60.0 {
        format!("{s:.0} s")
    } else {
        format!("{} min {:02.0} s", (s / 60.0).floor(), s % 60.0)
    }
}

/// Columns of the printed step table, as their header reads.
pub(crate) const STEP_COLUMNS: [&str; 18] = [
    "in",
    "step",
    "dBFS",
    "burst",
    "rest",
    "GR end",
    "GR set",
    "attack τ",
    "attack 2dB",
    "settle 0.5",
    "overshoot",
    "release τ",
    "recov 2dB",
    "recov 0.5",
    "THD atk",
    "THD set",
    "thump end",
    "drift",
];

fn time(t: DynamicsTime) -> String {
    match t {
        DynamicsTime::Seconds { value } => duration(value.0),
        DynamicsTime::WithinPeriod => "≤1 period".into(),
        DynamicsTime::CantTell => "can't tell".into(),
        DynamicsTime::NoGainReduction => "no GR".into(),
    }
}

fn duration(s: f64) -> String {
    if s.abs() < 1.0 {
        format!("{:.2} ms", s * 1000.0)
    } else {
        format!("{s:.2} s")
    }
}

fn level(l: DynamicsLevel) -> String {
    match l {
        DynamicsLevel::Db { value } => format!("{:.1} dB", value.0),
        DynamicsLevel::BelowFloor => "≤ floor".into(),
        DynamicsLevel::CantTell => "can't tell".into(),
    }
}

/// The step table of a stored run: one row per measured input and step, the rest
/// marked `*` where it reached the longest rest without recovering.
pub(crate) fn step_table(trace: TraceId, d: &DynamicsData) -> String {
    let i = &d.info;
    let mut rows: Vec<Vec<String>> = vec![STEP_COLUMNS.iter().map(|s| (*s).to_owned()).collect()];
    for input in &d.inputs {
        for (k, r) in input.rows.iter().enumerate() {
            let Some(s) = i.steps.get(k) else { continue };
            rows.push(vec![
                format!("{}", input.input + 1),
                format!("{}", k + 1),
                format!("{:.1}", s.level.0),
                duration(s.burst.0),
                format!("{:.1} s{}", s.rest.0, if s.recovered { "" } else { "*" }),
                format!("{:.1} dB", r.gr_end.0),
                r.gr_settled
                    .map_or_else(|| "can't tell".into(), |g| format!("{:.1} dB", g.0)),
                time(r.attack_tau),
                time(r.attack_2db),
                time(r.settle_0_5db),
                r.overshoot.map_or_else(
                    || "can't tell".into(),
                    |o| format!("{:.1} dB @ {}", o.level.0, duration(o.at.0)),
                ),
                time(r.release_tau),
                time(r.recovery_2db),
                time(r.recovery_0_5db),
                level(r.harmonics.attack.thd),
                level(r.harmonics.settled.thd),
                level(r.thump_end.peak),
                format!("{:+.2} dB", r.drift.0),
            ]);
        }
    }
    let widths: Vec<usize> = (0..STEP_COLUMNS.len())
        .map(|c| rows.iter().map(|r| r[c].chars().count()).max().unwrap_or(0))
        .collect();
    let mut s = String::new();
    let _ = writeln!(
        s,
        "trace {trace}: {} steps at {:.2} Hz ({}-sample period), pilot {}{}",
        i.steps.len(),
        i.carrier.0,
        i.period,
        ac2_scene::format::dbfs(i.pilot.0),
        if i.replayed { ", rests replayed" } else { "" }
    );
    for input in &d.inputs {
        let _ = writeln!(
            s,
            "in {} {}: delay {}, gain at pilot {:+.2} dB",
            input.input + 1,
            input.name,
            duration(input.delay.0),
            input.g0.0
        );
    }
    for r in rows {
        let line: Vec<String> = r
            .iter()
            .zip(&widths)
            .map(|(v, w)| format!("{v:>w$}", w = *w))
            .collect();
        let _ = writeln!(s, "{}", line.join("  ").trim_end());
    }
    if i.steps.iter().any(|s| !s.recovered) {
        let _ = writeln!(s, "* rest reached its longest without recovering");
    }
    s
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use ac2_audio::fake::{FakeDrive, FakePath};
    use ac2_audio::{FakeBackend, FakeConfig};
    use ac2_client::{ClientConfig, Endpoints};
    use ac2_proto::model::{DeviceSelector, MeasConfig, SessionConfig};
    use clap::Parser;

    use super::*;
    use crate::args::{Cmd, MeasCmd};

    type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

    /// Steps the device a batch at a time whenever the daemon has read the last, so the
    /// programme's seconds of device time take a fraction of that in wall time and no
    /// audio is lost.
    fn drive(backend: &FakeBackend, stop: &AtomicBool) {
        let mut current = None;
        while !stop.load(Ordering::Relaxed) {
            if let Some(d) = backend.take_driver() {
                current = Some(d);
            }
            match current.as_mut() {
                Some(d) if d.capture_queued() == 0 => d.run_blocks(16),
                _ => std::thread::sleep(Duration::from_millis(1)),
            }
        }
    }

    /// `ac2 meas new dynamics …` parsed as the CLI parses it.
    fn meas_new(args: &[&str]) -> R<MeasConfig> {
        let cli = Cli::try_parse_from(
            ["ac2", "meas", "new", "dynamics", "--name", "VCA"]
                .iter()
                .chain(args),
        )?;
        let Cmd::Meas {
            cmd: MeasCmd::New(n),
        } = &cli.cmd
        else {
            return Err("not meas new".into());
        };
        Ok(super::super::meas_config::meas_config(n)?)
    }

    /// The CLI's own parse of `meas new dynamics` makes the measurement, and `dynamics run`
    /// plays it on the real daemon (a linear simulated path: out 1 to in 1 and in 2) and
    /// prints its step table.
    #[tokio::test(flavor = "multi_thread")]
    async fn slow_meas_new_then_run_prints_the_step_table() -> R {
        let backend = FakeBackend::new(FakeConfig {
            drive: FakeDrive::Manual,
            paths: vec![
                FakePath::loopback(0, 0, 32),
                FakePath::acoustic(0, 1, 32 + 240, vec![0.5], 1e-6),
            ],
            ..FakeConfig::default()
        })?;
        let stop = Arc::new(AtomicBool::new(false));
        let driver = std::thread::spawn({
            let (b, stop) = (backend.clone(), stop.clone());
            move || drive(&b, &stop)
        });
        let listen = ac2d::Listen::Local {
            ctrl: "tcp://127.0.0.1:0".into(),
            data: "tcp://127.0.0.1:0".into(),
        };
        let h = ac2d::Daemon::start(ac2d::DaemonConfig::new(Arc::new(backend), listen, -10.0))?;
        let ep = Endpoints {
            ctrl: h.ctrl_endpoint().to_owned(),
            data: h.data_endpoint().to_owned(),
        };
        let c = Client::connect(ClientConfig::new(ep, "dynamics-test")).await?;
        c.wait_synced(Duration::from_secs(5)).await?;
        c.call(Command::SessionOpen {
            config: SessionConfig {
                backend: None,
                input_device: DeviceSelector::Default,
                output_device: DeviceSelector::Default,
                input_channels: vec![0, 1],
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

        // Flags as typed: 1-based channels, units on every value.
        assert!(meas_new(&["--ref", "1", "--meas", "2", "--out", "1"]).is_err());
        let config = meas_new(&[
            "--ref",
            "1",
            "--meas",
            "2",
            "--out",
            "1",
            "--carrier",
            "1khz",
            "--pilot",
            "-33dbfs",
            "--bursts",
            "-20dbfs",
            "--durations",
            "10ms,100ms",
            "--rest-max",
            "2s",
        ])?;
        let MeasKind::Dynamics { config: d } = &config.kind else {
            return Err("not a dynamics measurement".into());
        };
        assert_eq!(
            (d.reference_input, &d.measurement_inputs, &d.outputs),
            (0, &vec![1], &vec![0])
        );
        let defaults = meas_new(&[
            "--ref",
            "1",
            "--meas",
            "2",
            "--out",
            "1",
            "--carrier",
            "1khz",
            "--pilot",
            "-33dbfs",
            "--bursts",
            "-8dbfs,-5dbfs,0dbfs",
        ])?;
        let MeasKind::Dynamics { config: x } = &defaults.kind else {
            return Err("not a dynamics measurement".into());
        };
        assert_eq!(x.durations.len(), 9);
        assert_eq!(x.rest_max.0, 30.0);
        assert_eq!(x.bursts.len(), 3);
        let m = expect_body!(
            "meas.create",
            c.call(Command::MeasCreate { config }).await?,
            ReplyBody::Measurement(m) => m
        )?;
        let MeasKind::Dynamics { config: cfg } = &m.config.kind else {
            return Err("not a dynamics measurement".into());
        };
        let st = c.snapshot().await?.state;
        let plan = plan(&st, cfg, None)?;
        assert!((plan.carrier - 1000.0).abs() < 1e-9 && plan.top == -20.0);

        let lease = c.acquire_lease(false, OnDrop::StopAndRelease).await?;
        let (tx, rx) = mpsc::unbounded_channel();
        let mut buf = Vec::new();
        let mut out = Out::new(false, &mut buf);
        let drive_keys = async {
            let end = Instant::now() + Duration::from_secs(10);
            while !c
                .watch()
                .borrow()
                .state
                .as_ref()
                .is_some_and(|s| s.generator.armed)
            {
                if Instant::now() > end {
                    return Err::<(), Box<dyn std::error::Error>>("never armed".into());
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            tx.send(Input::Key(Key::Enter))?;
            Ok(())
        };
        let run = foreground(&c, lease, &m, cfg, &plan, None, None, &mut out, (None, rx));
        let (keys, ran) = tokio::time::timeout(Duration::from_secs(120), async {
            tokio::join!(drive_keys, run)
        })
        .await?;
        keys?;
        ran?;
        stop.store(true, Ordering::Relaxed);
        drop(h);
        let _ = driver.join();
        let text = String::from_utf8(buf)?;
        assert!(
            text.contains("ARMED  VCA · 2 steps at 1000.00 Hz, up to −20.0 dBFS"),
            "{text}"
        );
        let header = STEP_COLUMNS.join("  ");
        let table = text
            .lines()
            .find(|l| {
                l.split_whitespace().collect::<Vec<_>>()
                    == header.split_whitespace().collect::<Vec<_>>()
            })
            .ok_or_else(|| format!("no step table header in\n{text}"))?;
        assert!(table.starts_with("in"));
        let rows: Vec<&str> = text
            .lines()
            .filter(|l| l.trim_start().starts_with("2  "))
            .collect();
        assert_eq!(rows.len(), 2, "{text}");
        assert!(rows.iter().all(|r| r.contains("no GR")), "{text}");
        Ok(())
    }
}
