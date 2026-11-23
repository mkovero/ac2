//! Dynamics measurements (`MeasKind::Dynamics`, `dynamics.run`) end to end on the fake rig
//! against the real daemon: a short programme through a linear path (no gain reduction, so
//! every time reads "no gain reduction"), stored as one trace the measurement owns, exported
//! as CSV and imported back as the same trace; a replayed run plays the same rests; a run
//! that ends, failed, stopped or done, leaves its measurement free to run again or go.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::{Duration, Instant};

use ac2_audio::fake::{FakeDrive, FakePath};
use ac2_audio::{FakeBackend, FakeConfig, FakeDriver};
use ac2_proto::event::Change;
use ac2_proto::model::{
    DynamicsConfig, DynamicsEdge, DynamicsFailure, DynamicsOnset, DynamicsRun, DynamicsStatus,
    DynamicsTime, ExportFormat, GeneratorDesired, GeneratorSettings, ImportFormat, ImportRole,
    MeasConfig, MeasKind, OwnedTraces, Signal, TraceKind, TraceOwner, TraceSource,
};
use ac2_proto::units::{Blob, Dbfs, Hz, LeaseToken, MeasId, Seconds, TraceId};
use ac2_proto::{Command, ErrorCode};
use ac2d::Daemon;
use common::*;

fn rig() -> FakeBackend {
    FakeBackend::new(FakeConfig {
        sample_rate: FS,
        block_frames: BLOCK,
        inputs: 4,
        outputs: 2,
        drive: FakeDrive::Manual,
        seed: 13,
        paths: vec![
            FakePath::loopback(0, 0, LOOP_DELAY),
            FakePath::acoustic(0, 1, LOOP_DELAY + ACOUSTIC_DELAY, vec![ACOUSTIC_GAIN], 1e-6),
        ],
        ..FakeConfig::default()
    })
    .unwrap()
}

fn config() -> DynamicsConfig {
    DynamicsConfig {
        reference_input: 0,
        measurement_inputs: vec![1],
        outputs: vec![0],
        carrier: Hz(1000.0),
        onset: DynamicsOnset::Zero,
        pilot: Dbfs(-33.0),
        bursts: vec![Dbfs(-20.0)],
        durations: vec![Seconds(0.01), Seconds(0.1)],
        rest_max: Seconds(4.0),
    }
}

fn meas_config(config: DynamicsConfig) -> MeasConfig {
    MeasConfig {
        scope: ac2_proto::model::MeasScope::Session,
        name: "VCA".into(),
        kind: MeasKind::Dynamics { config },
    }
}

fn create(c: &mut Client, config: DynamicsConfig) -> MeasId {
    let m = c.measurement(Command::MeasCreate {
        config: meas_config(config),
    });
    assert!(!m.running, "a dynamics measurement waits for dynamics.run");
    m.id
}

fn arm(c: &mut Client, token: LeaseToken) {
    arm_at(c, token, Dbfs(-20.0));
}

fn arm_at(c: &mut Client, token: LeaseToken, level: Dbfs) {
    c.ok(Command::GenSet {
        lease_token: token,
        desired: GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Sine { freq: Hz(1000.0) },
                level,
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: false,
        },
    });
}

/// Runs device time in 0.25 s steps, refreshing the lease, until the run has ended.
fn run_to_end(
    d: &mut FakeDriver,
    c: &mut Client,
    sub: &Sub,
    ka: &Sub,
    token: LeaseToken,
) -> DynamicsRun {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        assert!(Instant::now() < deadline, "the dynamics run never ended");
        run(d, 0.25);
        // Stepped faster than real time: wait for the daemon to take the audio in, or a slow
        // host overflows the capture queue and the run reports a dropout the test caused.
        handed_on(ka, end_sample(d));
        let _ = c.call(Command::GenRefresh { lease_token: token });
        while let Some(ac2_proto::DataMessage::Event(e)) = sub.next(Duration::from_millis(5)) {
            if let Change::Dynamics(r) = e.change
                && !r.active()
            {
                return r;
            }
        }
    }
}

fn play(
    d: &mut FakeDriver,
    c: &mut Client,
    sub: &Sub,
    ka: &Sub,
    token: LeaseToken,
    meas: MeasId,
    rests_from: Option<TraceId>,
) -> TraceId {
    arm(c, token);
    let r = c
        .call(Command::DynamicsRun(ac2_proto::DynamicsRunArgs {
            lease_token: token,
            meas,
            name: None,
            rests_from,
        }))
        .unwrap()
        .dynamics();
    assert!(r.active(), "{:?}", r.status);
    let done = run_to_end(d, c, sub, ka, token);
    let DynamicsStatus::Done { trace } = done.status else {
        panic!("dynamics run failed: {:?}", done.status);
    };
    trace
}

#[test]
fn slow_dynamics_run_stores_one_trace_that_exports_and_imports() {
    init_log();
    let backend = rig();
    let h = Daemon::start(common::config(backend.clone(), inproc("dynamics"))).unwrap();
    let (mut c, sub) = connect(&h, &[b"evt"]);
    let ka = Sub::connect(h.context(), h.data_endpoint(), &[b"ka"]);
    c.ok(Command::Hello {
        client: "dynamics test".into(),
    });
    c.ok(Command::SessionOpen {
        config: session(true),
    });
    let mut d = driver(&backend);
    let token = c.acquire();

    // A reference is required; the run waits for dynamics.run and refuses unarmed.
    let e = c
        .call(Command::MeasCreate {
            config: MeasConfig {
                scope: ac2_proto::model::MeasScope::Session,
                name: "bad".into(),
                kind: MeasKind::Dynamics {
                    config: DynamicsConfig {
                        measurement_inputs: vec![0],
                        ..config()
                    },
                },
            },
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{}", e.msg);
    let meas = create(&mut c, config());
    let e = c
        .call(Command::DynamicsRun(ac2_proto::DynamicsRunArgs {
            lease_token: token,
            meas,
            name: None,
            rests_from: None,
        }))
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{}", e.msg);
    assert!(e.msg.contains("arm"), "{}", e.msg);

    arm(&mut c, token);
    let r = c
        .call(Command::DynamicsRun(ac2_proto::DynamicsRunArgs {
            lease_token: token,
            meas,
            name: None,
            rests_from: None,
        }))
        .unwrap()
        .dynamics();
    // 1 kHz is a 48-sample period at 48 kHz: played as typed.
    assert_eq!(r.carrier, Hz(1000.0));
    assert_eq!(r.name, "Run 1");
    // While it plays, its measurement stays and the level holds.
    let e = c
        .call(Command::MeasDelete {
            meas,
            traces: OwnedTraces::Keep,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{}", e.msg);
    // Its settings hold too: the analysis reads the programme it was started with.
    let e = c.update(meas, meas_config(config())).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{}", e.msg);
    assert!(e.msg.contains("playing"), "{}", e.msg);
    let st = c.state();
    assert!(st.generator.firing);
    let e = c
        .call(Command::GenSet {
            lease_token: token,
            desired: GeneratorDesired {
                settings: st.generator.settings.clone().unwrap(),
                armed: true,
                firing: true,
            },
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{}", e.msg);

    let done = run_to_end(&mut d, &mut c, &sub, &ka, token);
    let DynamicsStatus::Done { trace } = done.status else {
        panic!("dynamics run failed: {:?}", done.status);
    };
    let st = c.state();
    assert!(
        !st.generator.armed && !st.generator.firing,
        "one shot, disarmed"
    );

    let data = c.trace_data(Command::TraceGet { trace });
    assert_eq!(data.meta.kind, TraceKind::Dynamics);
    assert_eq!(data.meta.edit.owner, TraceOwner::Meas { meas });
    assert!(matches!(
        &data.meta.source,
        TraceSource::Dynamics { meas: m, number: 1, .. } if *m == meas
    ));
    let dyn_data = data.dynamics.clone().expect("dynamics data");
    assert_eq!(dyn_data.info.steps.len(), 2);
    assert_eq!(dyn_data.info.period, 48);
    assert_eq!(dyn_data.inputs.len(), 1);
    let input = &dyn_data.inputs[0];
    assert_eq!(input.input, 1);
    // The acoustic path re the loopback: its gain and delay.
    let g0 = 20.0 * f64::from(ACOUSTIC_GAIN).log10();
    assert!((input.g0.0 - g0).abs() < 0.1, "g0 {}", input.g0.0);
    let delay = f64::from(ACOUSTIC_DELAY) / f64::from(FS);
    assert!(
        (input.delay.0 - delay).abs() < 2.0 / f64::from(FS),
        "delay {}",
        input.delay.0
    );
    for row in &input.rows {
        // A linear path: nothing to time.
        assert!(row.gr_end.0.abs() < 0.5, "{row:?}");
        assert_eq!(row.attack_tau, DynamicsTime::NoGainReduction, "{row:?}");
        assert_eq!(row.release_tau, DynamicsTime::NoGainReduction, "{row:?}");
    }
    // A linear path is flat at once: every rest ends at the first flat reading after `rest_min`.
    for s in &dyn_data.info.steps {
        assert!(s.recovered && s.rest.0 < 3.0, "{s:?}");
    }
    // The step table alone is what `trace.get` holds without the envelopes.
    assert_eq!(
        c.dynamics_table(Command::DynamicsTable { trace }),
        dyn_data.table()
    );
    // One step's envelopes of one input are its points split by edge, in order.
    let env = |input, step| {
        Command::DynamicsEnvelope(ac2_proto::DynamicsEnvelopeArgs { trace, input, step })
    };
    let e1 = c.dynamics_envelope(env(1, 1));
    let points = &dyn_data.envelopes[0][1];
    let starts: Vec<f32> = points
        .iter()
        .filter(|p| p.edge == DynamicsEdge::Start)
        .map(|p| p.t)
        .collect();
    assert_eq!(e1.start.t, starts);
    assert_eq!(e1.start.t.len() + e1.end.t.len(), points.len());
    assert_eq!(e1.end.gr_db.len(), e1.end.t.len());
    assert!(e1.start.h_db.iter().all(|h| h.len() == starts.len()));
    for bad in [env(0, 0), env(1, 2)] {
        let e = c.call(bad).unwrap_err();
        assert_eq!(e.code, ErrorCode::Invalid, "{}", e.msg);
    }
    // Ended, the measurement takes new settings again.
    c.update_ok(meas, meas_config(config()));

    // Exported and imported back, it is the same trace.
    let csv = c.export_text(Command::TraceExport {
        trace,
        format: ExportFormat::Ac2Csv,
    });
    assert!(csv.contains("# dynamics_info: "), "{}", &csv[..400]);
    let back = c.import_trace(Import {
        file_name: "dynamics.csv".into(),
        format: ImportFormat::Auto,
        role: ImportRole::Trace,
        ir: None,
        content: Blob(csv.clone().into_bytes()),
        owner: TraceOwner::Imported,
    });
    assert_eq!(back.kind, TraceKind::Dynamics);
    let again = c.trace_data(Command::TraceGet { trace: back.id });
    assert_eq!(
        again.dynamics.as_ref().map(|d| &d.info),
        data.dynamics.as_ref().map(|d| &d.info)
    );
    // Harmonics under the analysis floor are NaN points, so the data is compared as the
    // text it exports to; the CSV carries its own header lines, which name the trace.
    let csv_again = c.export_text(Command::TraceExport {
        trace: back.id,
        format: ExportFormat::Ac2Csv,
    });
    let body = |t: &str| {
        t.lines()
            .skip_while(|l| !l.starts_with("# dynamics_info: "))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    assert_eq!(body(&csv_again), body(&csv));

    // A replayed run plays the first run's rests and is numbered after it.
    let replay = play(&mut d, &mut c, &sub, &ka, token, meas, Some(back.id));
    let second = c.trace_data(Command::TraceGet { trace: replay });
    assert!(matches!(
        &second.meta.source,
        TraceSource::Dynamics { number: 2, .. }
    ));
    let s2 = second.dynamics.unwrap();
    assert!(s2.info.replayed);
    for (a, b) in s2.info.steps.iter().zip(&dyn_data.info.steps) {
        assert!((a.rest.0 - b.rest.0).abs() < 1e-3, "{a:?} vs {b:?}");
    }
}

#[test]
fn slow_an_ended_run_leaves_its_measurement_free() {
    init_log();
    let backend = rig();
    let h = Daemon::start(common::config(backend.clone(), inproc("dynamics-ended"))).unwrap();
    let (mut c, sub) = connect(&h, &[b"evt"]);
    let ka = Sub::connect(h.context(), h.data_endpoint(), &[b"ka"]);
    c.ok(Command::Hello {
        client: "dynamics test".into(),
    });
    c.ok(Command::SessionOpen {
        config: ac2_proto::model::SessionConfig {
            input_channels: vec![0, 1, 2],
            ..session(true)
        },
    });
    let mut d = driver(&backend);
    let token = c.acquire();
    let start = |c: &mut Client, meas: MeasId| {
        arm(c, token);
        c.call(Command::DynamicsRun(ac2_proto::DynamicsRunArgs {
            lease_token: token,
            meas,
            name: None,
            rests_from: None,
        }))
        .map(|b| b.dynamics())
    };

    // Input 3 is captured but has no path from the outputs: the job finds no programme and fails the run.
    let meas = create(
        &mut c,
        DynamicsConfig {
            reference_input: 2,
            ..config()
        },
    );
    start(&mut c, meas).unwrap();
    let failed = run_to_end(&mut d, &mut c, &sub, &ka, token);
    assert!(
        matches!(
            failed.status,
            DynamicsStatus::Failed {
                reason: DynamicsFailure::NoReference,
                ..
            }
        ),
        "{:?}",
        failed.status
    );

    // A failed run holds nothing: the measurement runs again, and a stopped run likewise
    // lets it go.
    let again = start(&mut c, meas).unwrap();
    assert!(again.active(), "{:?}", again.status);
    c.ok(Command::GenStop);
    let stopped = run_to_end(&mut d, &mut c, &sub, &ka, token);
    assert!(
        matches!(
            stopped.status,
            DynamicsStatus::Failed {
                reason: DynamicsFailure::Stopped,
                ..
            }
        ),
        "{:?}",
        stopped.status
    );
    c.ok(Command::MeasDelete {
        meas,
        traces: OwnedTraces::Delete,
    });

    // A run that ends with its trace likewise.
    let meas = create(&mut c, config());
    play(&mut d, &mut c, &sub, &ka, token, meas, None);
    c.ok(Command::MeasDelete {
        meas,
        traces: OwnedTraces::Keep,
    });
}

/// A programme louder than the global maximum is made, changed and kept like any other; it is
/// refused when it would play, against the maximum in force at that moment.
#[test]
fn a_programme_above_the_global_maximum_is_kept_and_refused_when_run() {
    init_log();
    let backend = rig();
    let h = Daemon::start(common::config(backend.clone(), inproc("dynamics-ceiling"))).unwrap();
    let (mut c, _sub) = connect(&h, &[b"evt"]);
    c.ok(Command::Hello {
        client: "dynamics test".into(),
    });
    c.ok(Command::SessionOpen {
        config: session(true),
    });
    let token = c.acquire();
    let run = |c: &mut Client, meas| {
        c.call(Command::DynamicsRun(ac2_proto::DynamicsRunArgs {
            lease_token: token,
            meas,
            name: None,
            rests_from: None,
        }))
    };
    // The daemon's maximum is −10 dBFS.
    let loud = DynamicsConfig {
        bursts: vec![Dbfs(-20.0), Dbfs(-6.0)],
        ..config()
    };
    let meas = create(&mut c, loud.clone());
    let quiet = create(&mut c, config());
    c.update_ok(quiet, meas_config(loud));
    c.update_ok(quiet, meas_config(config()));

    arm(&mut c, token);
    let e = run(&mut c, meas).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{}", e.msg);
    assert!(e.msg.contains("-6.0 dBFS"), "{}", e.msg);

    // Brought down below the quiet one's −20 dBFS: it is refused too.
    c.ok(Command::GenSet {
        lease_token: token,
        desired: GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Sine { freq: Hz(1000.0) },
                level: Dbfs(-40.0),
                band: None,
                outputs: vec![0],
            },
            armed: false,
            firing: false,
        },
    });
    c.ok(Command::GenCeiling {
        ceiling: Dbfs(-30.0),
        confirm_raise: false,
    });
    arm_at(&mut c, token, Dbfs(-40.0));
    let e = run(&mut c, quiet).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{}", e.msg);
    assert!(e.msg.contains("-20.0 dBFS"), "{}", e.msg);
    assert!(e.msg.contains("-30.0 dBFS"), "{}", e.msg);
}

/// The largest programme a dynamics measurement may hold: every measured input, every level,
/// every length. Its step table and one step's envelopes each fit one ctrl message, at the
/// highest sample rate and shortest carrier period, the longest burst and rest.
#[test]
fn the_largest_programmes_table_and_envelope_fit_a_ctrl_message() {
    use ac2_core::dynamics::{
        MAX_BURST_SECONDS, MAX_DURATIONS, MAX_LEVELS, MIN_PERIOD, REST_MAX_SECONDS,
        max_envelope_points,
    };
    use ac2_proto::model::{DynamicsEdgeCurve, DynamicsEnvelope, DynamicsTable};
    use ac2_proto::units::RequestId;
    use ac2_proto::{MAX_CTRL_BYTES, Reply, ReplyBody, encode_reply};

    const INPUTS: usize = 8;
    let sample = ac2_proto::samples::dynamics_data();
    let mut table: DynamicsTable = sample.table();
    let step = table.info.steps[0];
    table.info.bursts = vec![Dbfs(-8.0); MAX_LEVELS];
    table.info.durations = vec![Seconds(0.001); MAX_DURATIONS];
    table.info.steps = vec![step; MAX_LEVELS * MAX_DURATIONS];
    let input = table.inputs[0].clone();
    table.inputs = (0..INPUTS as u16)
        .map(|i| ac2_proto::model::DynamicsInput {
            input: i,
            name: "x".repeat(64),
            rows: vec![input.rows[0].clone(); MAX_LEVELS * MAX_DURATIONS],
            ..input.clone()
        })
        .collect();
    let size = |body| {
        encode_reply(&Reply::new(RequestId(1), Ok(body)))
            .unwrap()
            .len()
    };
    let t = size(ReplyBody::DynamicsTable(Box::new(table)));
    assert!(t <= MAX_CTRL_BYTES, "table {t} B");

    let fs = 192_000.0;
    let n = max_envelope_points(fs, MIN_PERIOD, MAX_BURST_SECONDS, REST_MAX_SECONDS);
    // Not one value repeats or is NaN, so no encoding is shorter than the worst.
    let curve = |k: usize| {
        let v = |j: usize| {
            (0..k)
                .map(|i| (i * 7 + j) as f32 * 1.000_1)
                .collect::<Vec<_>>()
        };
        DynamicsEdgeCurve {
            t: v(0),
            gr_db: v(1),
            h_db: [v(2), v(3), v(4), v(5)],
        }
    };
    let e = size(ReplyBody::DynamicsEnvelope(Box::new(DynamicsEnvelope {
        input: 7,
        step: 127,
        start: curve(n),
        end: curve(n),
    })));
    assert!(
        e <= MAX_CTRL_BYTES,
        "envelope of {n} points per edge: {e} B"
    );
}

/// An 8 kHz run's trace, as the rig made one: 4 levels × 9 lengths at 96 kHz, 2 inputs with
/// their envelopes; its CSV is several ctrl messages long.
fn big_dynamics_csv() -> Vec<u8> {
    use ac2_proto::model::{
        CalState, DynamicsData, DynamicsInfo, DynamicsPoint, DynamicsStep, TraceMeta,
    };
    use ac2_proto::units::{DynamicsId, SessionEpoch, WallNs};
    use ac2_traces::StoredTrace;

    const POINTS: usize = 1500;
    let sample = ac2_proto::samples::dynamics_data();
    let bursts = [-14.0, -10.0, -6.0, -1.0].map(Dbfs).to_vec();
    let durations = [0.001, 0.003, 0.01, 0.03, 0.1, 0.3, 1.0, 3.0, 10.0]
        .map(Seconds)
        .to_vec();
    let steps: Vec<DynamicsStep> = bursts
        .iter()
        .flat_map(|&level| {
            durations.iter().map(move |&burst| DynamicsStep {
                level,
                burst,
                rest: Seconds(4.5),
                recovered: true,
            })
        })
        .collect();
    let n = steps.len();
    let envelope = |seed: usize| -> Vec<DynamicsPoint> {
        (0..POINTS)
            .map(|j| {
                let start = j < POINTS * 2 / 3;
                let k = if start { j } else { j - POINTS * 2 / 3 };
                DynamicsPoint {
                    edge: if start {
                        DynamicsEdge::Start
                    } else {
                        DynamicsEdge::End
                    },
                    t: -0.05 + k as f32 * 0.000_125,
                    gr_db: ((seed * 31 + j) % 977) as f32 * 0.010_3,
                    h_db: [-40.5 - (j % 13) as f32, -55.25, -70.0, -81.5],
                }
            })
            .collect()
    };
    let input = |i: u16| ac2_proto::model::DynamicsInput {
        input: i,
        name: format!("Studer {i}"),
        rows: vec![sample.inputs[0].rows[0].clone(); n],
        ..sample.inputs[0].clone()
    };
    let d = DynamicsData {
        info: DynamicsInfo {
            sample_rate: Hz(96_000.0),
            carrier: Hz(8000.0),
            period: 12,
            bursts,
            durations,
            steps,
            ..sample.info.clone()
        },
        inputs: vec![input(4), input(5)],
        envelopes: (0..2)
            .map(|i| (0..n).map(|s| envelope(i * n + s)).collect())
            .collect(),
    };
    let grid = ac2_traces::text::dynamics::grid(d.info.carrier);
    let id = TraceId(1);
    let t = StoredTrace {
        meta: TraceMeta {
            id,
            edit: ac2_traces::meta::new_edit(id, "8k-1617".into(), None, TraceOwner::Imported),
            kind: TraceKind::Dynamics,
            source: TraceSource::Dynamics {
                meas: MeasId(2),
                meas_name: "Studer dyn 8k".into(),
                run: DynamicsId(1),
                number: 1,
                epoch: SessionEpoch(3),
            },
            grid_id: grid.id(),
            delay: Seconds(0.0),
            depth: None,
            lf_window: None,
            cal: CalState::Uncalibrated,
            mic: None,
            mic_curve: None,
            created_at: WallNs(1_790_000_000_000_000_000),
            eq_in: Vec::new(),
            position: None,
            captured_unsettled: false,
            superseded: false,
        },
        grid,
        columns: ac2_traces::text::dynamics::columns(),
        sweep: None,
        dynamics: Some(Box::new(d)),
        ir: None,
        mic_curve: None,
    };
    ac2_traces::text::export_csv(&t).into_bytes()
}

/// A trace file bigger than one ctrl message goes up in chunks and comes down in chunks, the
/// same trace both ways; the file sent whole in one message is answered `too_large`.
#[test]
fn a_trace_bigger_than_a_ctrl_message_imports_and_exports() {
    use ac2_proto::MAX_CTRL_BYTES;
    use ac2_proto::model::DynamicsTable;
    init_log();
    let backend = rig();
    let h = Daemon::start(common::config(backend.clone(), inproc("dynamics-big"))).unwrap();
    let (mut c, _sub) = connect(&h, &[b"evt"]);
    c.ok(Command::Hello {
        client: "dynamics test".into(),
    });
    let csv = big_dynamics_csv();
    assert!(csv.len() > MAX_CTRL_BYTES, "{} B", csv.len());

    let e = c
        .call(Command::FileUploadChunk {
            transfer: ac2_proto::units::TransferId(1),
            offset: 0,
            data: Blob(csv.clone()),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::TooLarge, "{}", e.msg);

    let import = |c: &mut Client, file_name: &str, content: Vec<u8>| {
        c.import_trace(Import {
            file_name: file_name.into(),
            format: ImportFormat::Ac2Csv,
            role: ImportRole::Trace,
            ir: None,
            content: Blob(content),
            owner: TraceOwner::Imported,
        })
    };
    let a = import(&mut c, "8k-1617.csv", csv);
    assert_eq!(a.kind, TraceKind::Dynamics);
    let (_, out) = c.export(Command::TraceExport {
        trace: a.id,
        format: ExportFormat::Ac2Csv,
    });
    assert!(out.0.len() > MAX_CTRL_BYTES, "{} B", out.0.len());
    let b = import(&mut c, "again.csv", out.0);

    let table = |c: &mut Client, trace| -> DynamicsTable {
        c.dynamics_table(Command::DynamicsTable { trace })
    };
    let (ta, tb) = (table(&mut c, a.id), table(&mut c, b.id));
    assert_eq!(ta.info, tb.info);
    assert_eq!(ta.info.steps.len(), 36);
    assert_eq!(ta.inputs, tb.inputs);
    for (input, step) in [(4, 0), (5, 35)] {
        let env = |c: &mut Client, trace| {
            c.dynamics_envelope(Command::DynamicsEnvelope(ac2_proto::DynamicsEnvelopeArgs {
                trace,
                input,
                step,
            }))
        };
        let (ea, eb) = (env(&mut c, a.id), env(&mut c, b.id));
        assert_eq!(ea, eb);
        assert_eq!(ea.start.t.len() + ea.end.t.len(), 1500);
    }
}
