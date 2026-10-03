//! Sweep measurement (`ir.capture`) end to end on a fake rig whose acoustic path distorts
//! with known harmonics: from an empty daemon through session, lease, arm and capture to
//! a stored sweep trace with H2/H3 at their analytic levels; aborts discard the run.
#![allow(clippy::unwrap_used)]

mod common;

use std::time::{Duration, Instant};

use ac2_audio::fake::{FakeDrive, FakePath};
use ac2_audio::{FakeBackend, FakeConfig, FakeDriver};
use ac2_proto::event::Change;
use ac2_proto::model::{
    EssSpec, GeneratorDesired, GeneratorSettings, Signal, SweepFailure, SweepInputs, SweepRequest,
    SweepRun, SweepStatus, TraceKind, TraceSource,
};
use ac2_proto::units::{Dbfs, Hz, LeaseToken, Seconds, TraceId};
use ac2_proto::{Command, ErrorCode, ReplyBody};
use ac2d::{Daemon, FAKE_RIG_DISTORTION};
use common::*;

const T: Duration = Duration::from_secs(10);
const LEVEL: f64 = -20.0;

fn distorting_rig() -> FakeBackend {
    FakeBackend::new(FakeConfig {
        sample_rate: FS,
        block_frames: BLOCK,
        inputs: 4,
        outputs: 2,
        drive: FakeDrive::Manual,
        seed: 11,
        paths: vec![
            FakePath::loopback(0, 0, LOOP_DELAY),
            FakePath::acoustic(0, 1, LOOP_DELAY + ACOUSTIC_DELAY, vec![ACOUSTIC_GAIN], 1e-5)
                .distorting(FAKE_RIG_DISTORTION.to_vec()),
        ],
        ..FakeConfig::default()
    })
    .unwrap()
}

fn request(level: Option<f64>) -> SweepRequest {
    SweepRequest {
        inputs: SweepInputs::Channels {
            reference: 0,
            measurement: 1,
        },
        outputs: vec![0],
        level: level.map(Dbfs),
        sweep: EssSpec::with_fades(Hz(100.0), Hz(5000.0), Seconds(1.0)),
        repeats: 1,
        gate: None,
    }
}

fn arm(c: &mut Client, token: LeaseToken) {
    c.ok(Command::GenSet {
        lease_token: token,
        desired: GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Ess {
                    sweep: request(None).sweep,
                },
                level: Dbfs(LEVEL),
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: false,
        },
    });
}

/// Runs device time in 0.25 s steps, refreshing the lease, until a sweep event matches.
fn run_until(
    d: &mut FakeDriver,
    c: &mut Client,
    sub: &Sub,
    token: LeaseToken,
    mut pred: impl FnMut(&SweepRun) -> bool,
) -> SweepRun {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        assert!(Instant::now() < deadline, "no matching sweep event");
        run(d, 0.25);
        let _ = c.call(Command::GenRefresh { lease_token: token });
        while let Some(ac2_proto::DataMessage::Event(e)) = sub.next(Duration::from_millis(5)) {
            if let Change::Sweep(r) = e.change
                && pred(&r)
            {
                return r;
            }
        }
    }
}

fn setup() -> (
    ac2d::Handle,
    FakeBackend,
    Client,
    Sub,
    FakeDriver,
    LeaseToken,
) {
    init_log();
    let backend = distorting_rig();
    let h = Daemon::start(config(backend.clone(), inproc("sweep"))).unwrap();
    let (mut c, sub) = connect(&h, &[b"evt"]);
    c.ok(Command::Hello {
        client: "sweep test".into(),
    });
    c.ok(Command::SessionOpen {
        config: session(true),
    });
    let d = driver(&backend);
    let token = match c.ok(Command::GenAcquire { force: false }) {
        ReplyBody::Lease(l) => l.lease_token,
        other => panic!("{other:?}"),
    };
    (h, backend, c, sub, d, token)
}

#[test]
fn sweep_measures_the_rigs_harmonics_from_an_empty_daemon() {
    let (_h, _b, mut c, sub, mut d, token) = setup();

    // Refused before it is armed, without a level, above the ceiling.
    let capture = |c: &mut Client, level: Option<f64>| {
        c.call(Command::IrCapture {
            lease_token: token,
            request: request(level),
            name: "sweep 1".into(),
        })
    };
    let e = capture(&mut c, Some(LEVEL)).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{}", e.msg);
    assert!(e.msg.contains("arm"), "{}", e.msg);
    arm(&mut c, token);
    let e = capture(&mut c, None).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused);
    assert!(e.msg.contains("level"), "{}", e.msg);
    let e = capture(&mut c, Some(-5.0)).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused);
    assert!(e.msg.contains("maximum"), "{}", e.msg);

    let ReplyBody::Sweep(run0) = capture(&mut c, Some(LEVEL)).unwrap() else {
        panic!("not a sweep");
    };
    assert_eq!(run0.status, SweepStatus::Playing { repeat: 1 });
    // While it plays, the generator fires the sweep and other stimulus commands wait.
    let st = match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state,
        other => panic!("{other:?}"),
    };
    assert!(st.generator.firing && st.generator.armed);
    assert_eq!(st.sweep.as_ref().map(|s| s.id), Some(run0.id));
    assert_eq!(
        c.call(Command::GenSet {
            lease_token: token,
            desired: GeneratorDesired {
                settings: st.generator.settings.clone().unwrap(),
                armed: true,
                firing: true,
            },
        })
        .unwrap_err()
        .code,
        ErrorCode::Refused
    );

    let done = run_until(&mut d, &mut c, &sub, token, |r| {
        matches!(
            r.status,
            SweepStatus::Done { .. } | SweepStatus::Failed { .. }
        )
    });
    let SweepStatus::Done { trace } = done.status else {
        panic!("sweep failed: {:?}", done.status);
    };
    // Armed again, not firing.
    let st = match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state,
        other => panic!("{other:?}"),
    };
    assert!(st.generator.armed && !st.generator.firing);

    let data = match c.ok(Command::TraceGet { trace }) {
        ReplyBody::TraceData(t) => t,
        other => panic!("{other:?}"),
    };
    assert_eq!(data.meta.kind, TraceKind::Sweep);
    assert!(matches!(data.meta.source, TraceSource::IrCapture { .. }));
    // The arrival is the acoustic path's delay re the loopback.
    let want = f64::from(ACOUSTIC_DELAY) / f64::from(FS);
    assert!(
        (data.meta.delay.0 - want).abs() < 1.5 / f64::from(FS),
        "{}",
        data.meta.delay.0
    );
    let s = data.sweep.expect("sweep data");
    let a = 10f64.powf(LEVEL / 20.0);
    let (c2, c3) = (FAKE_RIG_DISTORTION[0], FAKE_RIG_DISTORTION[1]);
    let fund = a + 0.75 * c3 * a.powi(3);
    let h2 = 20.0 * (0.5 * c2 * a * a / fund).log10();
    let h3 = 20.0 * (0.25 * c3 * a.powi(3) / fund).log10();
    let freqs = ac2_traces::frequencies(&match c.ok(Command::GridGet {
        grid_id: data.meta.grid_id,
    }) {
        ReplyBody::Grid(g) => g,
        other => panic!("{other:?}"),
    });
    let mut worst = (0.0f64, 0.0f64);
    for (i, f) in freqs.iter().enumerate() {
        if (200.0..=5000.0 / 3.0 / 1.2).contains(f) {
            let m = s.info.floor_margin;
            assert!(s.harmonics[0].curve.valid(i, m), "H2 at {f}");
            assert!(s.harmonics[1].curve.valid(i, m), "H3 at {f}");
            worst.0 = worst
                .0
                .max((f64::from(s.harmonics[0].curve.level_db[i]) - h2).abs());
            worst.1 = worst
                .1
                .max((f64::from(s.harmonics[1].curve.level_db[i]) - h3).abs());
        }
    }
    eprintln!(
        "daemon on the fake rig: H2 max error {:.3} dB, H3 max error {:.3} dB",
        worst.0, worst.1
    );
    assert!(worst.0 < 0.5 && worst.1 < 0.5, "H2 / H3 error {worst:?}");
    // The CSV export carries every curve.
    let csv = match c.ok(Command::TraceExport {
        trace,
        format: ac2_proto::model::ExportFormat::Ac2Csv,
    }) {
        ReplyBody::Export { content, .. } => String::from_utf8(content.0).unwrap(),
        other => panic!("{other:?}"),
    };
    assert!(
        csv.contains(",h2_db,h2_floor_db,h3_db,h3_floor_db,"),
        "{}",
        &csv[..600]
    );
    assert_eq!(trace, TraceId(1));
}

#[test]
fn stopping_or_losing_the_lease_discards_the_run() {
    let (_h, _b, mut c, sub, mut d, token) = setup();
    arm(&mut c, token);
    let start = |c: &mut Client| match c.ok(Command::IrCapture {
        lease_token: token,
        request: request(Some(LEVEL)),
        name: "sweep".into(),
    }) {
        ReplyBody::Sweep(r) => r,
        other => panic!("{other:?}"),
    };
    let r = start(&mut c);
    run(&mut d, 0.5);
    c.ok(Command::GenStop);
    let failed = run_until(&mut d, &mut c, &sub, token, |x| x.id == r.id && !x.active());
    assert!(matches!(
        failed.status,
        SweepStatus::Failed {
            reason: SweepFailure::Stopped,
            ..
        }
    ));
    let st = match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state,
        other => panic!("{other:?}"),
    };
    assert!(!st.generator.firing && !st.generator.armed);
    assert!(st.traces.is_empty(), "nothing stored");

    // Lease expiry: no refresh while the device runs on.
    arm(&mut c, token);
    let r = start(&mut c);
    let deadline = Instant::now() + T;
    let ended = loop {
        assert!(Instant::now() < deadline, "the run outlived its lease");
        run(&mut d, 0.1);
        std::thread::sleep(Duration::from_millis(100));
        if let Some(ac2_proto::DataMessage::Event(e)) = sub.next(Duration::from_millis(5))
            && let Change::Sweep(x) = e.change
            && x.id == r.id
            && !x.active()
        {
            break x;
        }
    };
    assert!(matches!(
        ended.status,
        SweepStatus::Failed {
            reason: SweepFailure::LeaseExpired,
            ..
        }
    ));
}
