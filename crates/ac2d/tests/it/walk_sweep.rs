//! Sweep walks on the fake rig: one run at a step records every standing mic at its point,
//! the walk goes on when the run is done, a run that fails leaves the points pending, the
//! average is the power average of the sweep traces, and the walk's room parameters are the
//! positions' means with their range.
#![allow(clippy::unwrap_used)]

use crate::common;
use crate::sweep::{LEVEL, arm, create, request, run_until, start};

use ac2_audio::fake::{FakeDrive, FakePath};
use ac2_audio::{FakeBackend, FakeConfig, FakeReverb};
use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Command, ErrorCode, ReplyBody};
use ac2d::Daemon;
use common::*;

/// Reverberation time at mic 1 and at mic 2: two seats of different rooms, so the walk's
/// mean lies between them and its range spans them.
const T60: [f32; 2] = [0.5, 0.9];

/// Out 1 → in 1 loopback; out 1 → in 2 and in 3, two mics in reverberant fields.
fn reverberant_rig() -> FakeBackend {
    let mic = |input: u16, t60_s: f32| {
        FakePath::acoustic(0, input, LOOP_DELAY + ACOUSTIC_DELAY, vec![0.25], 1e-5)
            .reverberant(FakeReverb { t60_s, level: 2.0 })
    };
    FakeBackend::new(FakeConfig {
        sample_rate: FS,
        block_frames: BLOCK,
        inputs: 4,
        outputs: 2,
        drive: FakeDrive::Manual,
        seed: 31,
        paths: vec![
            FakePath::loopback(0, 0, LOOP_DELAY),
            mic(1, T60[0]),
            mic(2, T60[1]),
        ],
        ..FakeConfig::default()
    })
    .unwrap()
}

fn state(c: &mut Client) -> State {
    match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state,
        other => panic!("{other:?}"),
    }
}

fn walk(c: &mut Client) -> WalkState {
    state(c).walks.into_iter().next().expect("the walk")
}

fn points(n: usize) -> Vec<WalkPointSpec> {
    (0..n)
        .map(|i| WalkPointSpec {
            label: format!("P{}", i + 1),
            at: [4.0 + 3.0 * i as f64, 12.0, 1.2],
        })
        .collect()
}

#[test]
fn slow_a_two_mic_sweep_walk_captures_each_step_with_one_run() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let backend = reverberant_rig();
    let mut cfg = config(backend.clone(), inproc("walk-sweep"));
    cfg.plan_dir = Some(dir.path().join("plans"));
    let h = Daemon::start(cfg).unwrap();
    let (mut c, sub) = connect(&h, &[b"evt"]);
    let ka = Sub::connect(h.context(), h.data_endpoint(), &[b"ka"]);
    c.ok(Command::Hello {
        client: "sweep walk test".into(),
    });
    c.ok(Command::SessionOpen {
        config: SessionConfig {
            input_channels: vec![0, 1, 2],
            ..session(true)
        },
    });
    let mut d = driver(&backend);
    let token = match c.ok(Command::GenAcquire { force: false }) {
        ReplyBody::Lease(l) => l.lease_token,
        other => panic!("{other:?}"),
    };
    c.ok(Command::PlanSave {
        plan: ac2_proto::samples::plan(),
    });
    let meas = create(
        &mut c,
        "Room sweep",
        SweepConfig {
            measurement_inputs: vec![1, 2],
            tail: Some(Seconds(2.0)),
            sweep: EssSpec::with_fades(Hz(100.0), Hz(5000.0), Seconds(1.0)),
            ..request(LEVEL)
        },
    );

    // A sweep walk takes one sweep measurement; its inputs are the mics.
    let w = match c.ok(Command::WalkNew {
        name: "Room".into(),
        plan: "Hall".into(),
        kind: WalkKind::Sweep,
        meas: vec![meas],
        points: points(4),
    }) {
        ReplyBody::Walk(w) => w,
        other => panic!("{other:?}"),
    };
    let mic = |input| WalkMic { meas, input };
    assert_eq!(w.mics, vec![mic(1), mic(2)]);
    assert_eq!(w.steps(), 2);
    let standing = |w: &WalkState| -> Vec<(String, WalkMic)> {
        w.standing()
            .into_iter()
            .map(|i| (w.points[i].label.clone(), w.points[i].mic))
            .collect()
    };
    let first = standing(&w);
    assert_eq!(first.len(), 2);
    // Its capture is the sweep's run, not walk.capture; it cannot change its mics.
    let e = c.call(Command::WalkCapture { walk: w.id }).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{}", e.msg);
    let m = state(&mut c)
        .measurements
        .into_iter()
        .find(|m| m.id == meas)
        .unwrap();
    let MeasKind::Sweep { config } = m.config.kind.clone() else {
        panic!("{:?}", m.config.kind)
    };
    let e = c
        .call(Command::MeasUpdate {
            meas,
            base: m.config_rev,
            config: MeasConfig {
                kind: MeasKind::Sweep {
                    config: SweepConfig {
                        measurement_inputs: vec![1],
                        ..config
                    },
                },
                ..m.config.clone()
            },
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{}", e.msg);

    // A run stopped while it plays leaves the points pending, and the walk where it was;
    // while it plays the walk does not move.
    arm(&mut c, token);
    let r = start(&mut c, token, meas);
    let e = c
        .call(Command::WalkGoto {
            walk: w.id,
            step: 1,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{}", e.msg);
    run(&mut d, 0.5);
    c.ok(Command::GenStop);
    let failed = run_until(&mut d, &mut c, &sub, &ka, token, |x| {
        x.id == r.id && !x.active()
    });
    assert!(
        matches!(failed.status, SweepStatus::Failed { .. }),
        "{:?}",
        failed.status
    );
    let after = walk(&mut c);
    assert_eq!(after.step, Some(0));
    assert_eq!(standing(&after), first);
    assert!(after.captured_traces().is_empty());

    // Each step: one run, a trace per standing mic, at its point.
    let mut runs = Vec::new();
    for step in 0..2u16 {
        let before = walk(&mut c);
        assert_eq!(before.step, Some(step));
        let at = standing(&before);
        arm(&mut c, token);
        let r = start(&mut c, token, meas);
        let inputs: Vec<u16> = at.iter().map(|(_, m)| m.input).collect();
        assert_eq!(r.measurement_inputs, inputs, "the standing mics");
        let done = run_until(&mut d, &mut c, &sub, &ka, token, |x| {
            x.id == r.id && !x.active()
        });
        let SweepStatus::Done { traces } = done.status else {
            panic!("sweep failed: {:?}", done.status);
        };
        assert_eq!(traces.len(), at.len());
        let st = state(&mut c);
        for ((label, m), t) in at.iter().zip(&traces) {
            let meta = st.traces.iter().find(|x| x.id == *t).unwrap();
            assert_eq!(meta.kind, TraceKind::Sweep);
            assert_eq!(meta.edit.name, format!("{label} · Input {}", m.input + 1));
            let p = meta.position.as_ref().expect("its position");
            assert_eq!((&p.label, p.walk, p.plan.as_str()), (label, w.id, "Hall"));
        }
        runs.push(traces);
    }

    let w = walk(&mut c);
    assert_eq!(w.step, None, "every point captured");
    let captured = w.captured_traces();
    assert_eq!(captured.len(), 4);
    for p in &w.points {
        assert!(
            matches!(p.state, PointState::Captured { .. }),
            "{}",
            p.label
        );
    }

    // The average: a power average of the four sweep traces, a transfer.
    let st = state(&mut c);
    let avg = st
        .measurements
        .iter()
        .find(|m| Some(m.id) == w.average)
        .expect("the average");
    let MeasKind::Math { config } = &avg.config.kind else {
        panic!("{:?}", avg.config.kind)
    };
    assert_eq!(config.domain, MathDomain::Transfer);
    let MathExpr::Average { of, .. } = &config.expr else {
        panic!("{:?}", config.expr)
    };
    let of: Vec<TraceId> = of
        .iter()
        .map(|o| match o {
            Operand::Trace { trace } => *trace,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(of, captured);

    // Room parameters over the four positions: two seats of each room, so each band's T20
    // mean lies between the two rooms' and its range spans them.
    let room = w.room.expect("room parameters over the walk");
    assert_eq!((room.positions, room.sources), (4, 1));
    let mut seen = 0;
    for b in room.octave.iter().chain(std::iter::once(&room.broadband)) {
        let Some(t20) = b.t20 else { continue };
        seen += 1;
        assert!(t20.min <= t20.mean && t20.mean <= t20.max, "{b:?}");
        assert!(t20.n <= 4);
        if t20.n == 4 {
            let lo = f64::from(T60[0]);
            let hi = f64::from(T60[1]);
            assert!(
                t20.min < 0.5 * (lo + hi) && t20.max > 0.5 * (lo + hi),
                "{:?}: {t20:?}",
                b.centre
            );
            assert!(t20.max - t20.min > 0.2, "{:?}: {t20:?}", b.centre);
        }
        if let Some(c80) = b.c80 {
            assert!(c80.min <= c80.mean && c80.mean <= c80.max);
        }
    }
    assert!(seen >= 3, "T20 in too few bands: {room:?}");

    // A skip takes the position out of the room parameters.
    let skipped = match c.ok(Command::WalkSkip {
        walk: w.id,
        point: 0,
    }) {
        ReplyBody::Walk(w) => w,
        other => panic!("{other:?}"),
    };
    assert_eq!(skipped.room.expect("three positions").positions, 3);
}
