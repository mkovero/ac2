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
    DynamicsConfig, DynamicsFailure, DynamicsOnset, DynamicsRun, DynamicsStatus, DynamicsTime,
    ExportFormat, GeneratorDesired, GeneratorSettings, ImportFormat, ImportRole, MeasConfig,
    MeasKind, OwnedTraces, Signal, TraceKind, TraceOwner, TraceSource,
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

fn create(c: &mut Client, config: DynamicsConfig) -> MeasId {
    let m = c.measurement(Command::MeasCreate {
        config: MeasConfig {
            scope: ac2_proto::model::MeasScope::Session,
            name: "VCA".into(),
            kind: MeasKind::Dynamics { config },
        },
    });
    assert!(!m.running, "a dynamics measurement waits for dynamics.run");
    m.id
}

fn arm(c: &mut Client, token: LeaseToken) {
    c.ok(Command::GenSet {
        lease_token: token,
        desired: GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Sine { freq: Hz(1000.0) },
                level: Dbfs(-20.0),
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

    // Exported and imported back, it is the same trace.
    let csv = c.export_text(Command::TraceExport {
        trace,
        format: ExportFormat::Ac2Csv,
    });
    assert!(csv.contains("# dynamics_info: "), "{}", &csv[..400]);
    let back = c.trace(Command::TraceImport {
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
