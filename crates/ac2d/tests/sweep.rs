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
    EssSpec, GeneratorDesired, GeneratorSettings, ImportFormat, ImportRole, Signal, SweepFailure,
    SweepInputs, SweepRequest, SweepRun, SweepStatus, TraceKind, TraceSource,
};
use ac2_proto::units::{Blob, Dbfs, Hz, LeaseToken, Seconds, TraceId};
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
        tail: None,
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
            request: Box::new(request(level)),
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
    // A finished sweep leaves the generator disarmed, never re-armed with the state from
    // before it: the lease stays with its holder, and the next sweep needs an explicit arm.
    let st = match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state,
        other => panic!("{other:?}"),
    };
    assert!(!st.generator.armed && !st.generator.firing);
    assert!(st.generator.owner.is_some(), "the lease is still held");
    let last = st.generator.last_action.as_ref().unwrap();
    assert_eq!(last.action, ac2_proto::model::GenAction::Stop);
    assert!(last.client.is_none(), "disarmed by the daemon");
    let e = capture(&mut c, Some(LEVEL)).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{}", e.msg);
    assert!(e.msg.contains("arm"), "{}", e.msg);

    let data = match c.ok(Command::TraceGet { trace }) {
        ReplyBody::TraceData(t) => *t,
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
    // Re-imported, the export is the same sweep again: the distortion pane draws it, the
    // delay readout reads the arrival.
    let back = match c.ok(Command::TraceImport {
        file_name: "sweep.csv".into(),
        format: ImportFormat::Auto,
        role: ImportRole::Trace,
        content: Blob(csv.into_bytes()),
    }) {
        ReplyBody::Trace(t) => t,
        other => panic!("{other:?}"),
    };
    assert_eq!(back.kind, TraceKind::Sweep);
    assert!((back.delay.0 - data.meta.delay.0).abs() < 1e-12);
    assert!(matches!(&back.source, TraceSource::Imported { notes, .. } if notes.is_empty()));
    let again = match c.ok(Command::TraceGet { trace: back.id }) {
        ReplyBody::TraceData(t) => *t,
        other => panic!("{other:?}"),
    };
    let s2 = again.sweep.expect("sweep data after import");
    assert_eq!((s2.ir, s2.info), (s.ir, s.info));
    assert_eq!(s2.harmonics.len(), s.harmonics.len());
}

#[test]
fn stopping_or_losing_the_lease_discards_the_run() {
    let (_h, _b, mut c, sub, mut d, token) = setup();
    arm(&mut c, token);
    let start = |c: &mut Client| match c.ok(Command::IrCapture {
        lease_token: token,
        request: Box::new(request(Some(LEVEL))),
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

/// A sweep that fails its analysis (played on an output nothing records) disarms like a
/// finished one: nothing is left armed for the next Enter.
#[test]
fn a_failed_sweep_leaves_the_generator_disarmed() {
    let (_h, _b, mut c, sub, mut d, token) = setup();
    arm(&mut c, token);
    let ReplyBody::Sweep(r) = c.ok(Command::IrCapture {
        lease_token: token,
        request: Box::new(SweepRequest {
            outputs: vec![1],
            ..request(Some(LEVEL))
        }),
        name: "unheard".into(),
    }) else {
        panic!("not a sweep");
    };
    let ended = run_until(&mut d, &mut c, &sub, token, |x| x.id == r.id && !x.active());
    assert!(
        matches!(ended.status, SweepStatus::Failed { .. }),
        "{:?}",
        ended.status
    );
    let st = match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state,
        other => panic!("{other:?}"),
    };
    assert!(!st.generator.armed && !st.generator.firing);
    assert!(st.traces.is_empty(), "nothing stored");
}

/// Room parameters end to end: a rig whose third input hears the output through a hall
/// with a known reverberation time (`ac2_audio::FakeReverb`, 0.8 s at every frequency); a
/// sweep with 2 s of silence after it reads T20 / T30 within 8 % of it in the octave bands
/// (its EDT is longer: the reverberator's field takes tens of ms to build up)
/// it excites, the export carries the table and the import restores it.
#[test]
fn a_sweep_in_a_hall_reads_its_reverberation_time() {
    use ac2_audio::FakeReverb;
    init_log();
    let t60 = 0.8;
    let backend = FakeBackend::new(FakeConfig {
        sample_rate: FS,
        block_frames: BLOCK,
        inputs: 4,
        outputs: 2,
        drive: FakeDrive::Manual,
        seed: 12,
        paths: vec![
            FakePath::loopback(0, 0, LOOP_DELAY),
            FakePath::acoustic(0, 2, LOOP_DELAY + ACOUSTIC_DELAY, vec![0.25], 1e-5).reverberant(
                FakeReverb {
                    t60_s: t60 as f32,
                    level: 2.0,
                },
            ),
        ],
        ..FakeConfig::default()
    })
    .unwrap();
    let h = Daemon::start(config(backend.clone(), inproc("sweep-hall"))).unwrap();
    let (mut c, sub) = connect(&h, &[b"evt"]);
    c.ok(Command::Hello {
        client: "hall test".into(),
    });
    c.ok(Command::SessionOpen {
        config: ac2_proto::model::SessionConfig {
            input_channels: vec![0, 1, 2],
            ..session(true)
        },
    });
    let mut d = driver(&backend);
    let token = match c.ok(Command::GenAcquire { force: false }) {
        ReplyBody::Lease(l) => l.lease_token,
        other => panic!("{other:?}"),
    };
    arm(&mut c, token);
    let req = SweepRequest {
        inputs: SweepInputs::Channels {
            reference: 0,
            measurement: 2,
        },
        sweep: EssSpec::with_fades(Hz(100.0), Hz(10_000.0), Seconds(1.0)),
        tail: Some(Seconds(2.0)),
        ..request(Some(LEVEL))
    };
    let ReplyBody::Sweep(r) = c.ok(Command::IrCapture {
        lease_token: token,
        request: Box::new(req),
        name: "hall".into(),
    }) else {
        panic!("not a sweep");
    };
    assert!((r.post_roll.0 - 2.0).abs() < 1e-3, "{:?}", r.post_roll);
    let done = run_until(&mut d, &mut c, &sub, token, |x| x.id == r.id && !x.active());
    let SweepStatus::Done { trace } = done.status else {
        panic!("sweep failed: {:?}", done.status);
    };
    let data = match c.ok(Command::TraceGet { trace }) {
        ReplyBody::TraceData(t) => *t,
        other => panic!("{other:?}"),
    };
    let s = data.sweep.expect("sweep data");
    let room = s.room.clone().expect("room parameters");
    assert!(
        room.span_end.0 > 1.9 && room.span_end.0 < 2.0,
        "{:?}",
        room.span_end
    );
    let centres: Vec<f64> = room
        .octave
        .iter()
        .filter_map(|b| b.centre.map(|c| c.0))
        .collect();
    assert_eq!(centres.len(), 5, "250 Hz … 4 kHz: {centres:?}");
    let mut bad = false;
    for b in room.octave.iter().chain(std::iter::once(&room.broadband)) {
        for (name, v) in [("T20", b.t20), ("T30", b.t30)] {
            let t = v
                .value()
                .unwrap_or_else(|| panic!("{:?} {name}: {v:?}", b.centre));
            eprintln!(
                "hall {:?} {name} {t:.3} s (T60 {t60})",
                b.centre.map(|c| c.0.round())
            );
            bad |= (t / t60 - 1.0).abs() >= 0.08;
        }
        assert!(
            b.decay_range.is_some_and(|r| r.0 > 45.0),
            "{:?}",
            b.decay_range
        );
    }
    assert!(!bad, "reverberation time off by 8 % or more");
    // The export carries the parameters, the import restores them.
    let csv = match c.ok(Command::TraceExport {
        trace,
        format: ac2_proto::model::ExportFormat::Ac2Csv,
    }) {
        ReplyBody::Export { content, .. } => String::from_utf8(content.0).unwrap(),
        other => panic!("{other:?}"),
    };
    assert!(csv.contains("\n# room_metrics: {"), "no room_metrics line");
    assert!(
        csv.contains("\n# band_hz,edt_s,t20_s,t30_s,"),
        "no room table"
    );
    let back = match c.ok(Command::TraceImport {
        file_name: "hall.csv".into(),
        format: ImportFormat::Auto,
        role: ImportRole::Trace,
        content: Blob(csv.into_bytes()),
    }) {
        ReplyBody::Trace(t) => t,
        other => panic!("{other:?}"),
    };
    let again = match c.ok(Command::TraceGet { trace: back.id }) {
        ReplyBody::TraceData(t) => *t,
        other => panic!("{other:?}"),
    };
    assert_eq!(again.sweep.and_then(|s| s.room), Some(room));
}
