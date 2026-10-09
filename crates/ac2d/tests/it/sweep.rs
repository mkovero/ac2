//! Sweep measurements (`MeasKind::Sweep`, `sweep.run`) end to end on a fake rig whose
//! acoustic path distorts with known harmonics: from an empty daemon through a sweep
//! measurement that waits, session, lease, arm and run to stored runs it owns with H2/H3 at
//! their analytic levels; aborts discard the run.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::{Duration, Instant};

use ac2_audio::fake::{FakeDrive, FakePath};
use ac2_audio::{FakeBackend, FakeConfig, FakeDriver};
use ac2_proto::event::Change;
use ac2_proto::model::{
    EssSpec, GeneratorDesired, GeneratorSettings, ImportFormat, ImportRole, MeasConfig, MeasKind,
    OwnedTraces, Signal, SweepConfig, SweepFailure, SweepRun, SweepStatus, TraceKind, TraceOwner,
    TraceSource,
};
use ac2_proto::units::{Blob, Dbfs, Hz, LeaseToken, MeasId, Seconds, TraceId};
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

fn request(level: f64) -> SweepConfig {
    SweepConfig {
        reference_input: 0,
        measurement_input: 1,
        outputs: vec![0],
        level: Dbfs(level),
        sweep: EssSpec::with_fades(Hz(100.0), Hz(5000.0), Seconds(1.0)),
        repeats: 1,
        gate: None,
        tail: None,
        lf_harmonics: ac2_proto::model::LfHarmonics::Standard,
    }
}

/// A sweep measurement with `config`: created, never started.
fn create(c: &mut Client, name: &str, config: SweepConfig) -> MeasId {
    match c.ok(Command::MeasCreate {
        config: MeasConfig {
            name: name.into(),
            kind: MeasKind::Sweep { config },
        },
    }) {
        ReplyBody::Measurement(m) => {
            assert!(!m.running, "a sweep measurement waits for sweep.run");
            m.id
        }
        other => panic!("{other:?}"),
    }
}

fn start(c: &mut Client, token: LeaseToken, meas: MeasId) -> SweepRun {
    match c.ok(Command::SweepRun {
        lease_token: token,
        meas,
        name: None,
    }) {
        ReplyBody::Sweep(r) => r,
        other => panic!("{other:?}"),
    }
}

fn arm(c: &mut Client, token: LeaseToken) {
    c.ok(Command::GenSet {
        lease_token: token,
        desired: GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Ess {
                    sweep: request(LEVEL).sweep,
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
    ka: &Sub,
    token: LeaseToken,
    mut pred: impl FnMut(&SweepRun) -> bool,
) -> SweepRun {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        assert!(Instant::now() < deadline, "no matching sweep event");
        run(d, 0.25);
        // The fake device is stepped faster than real time; wait for the daemon to take the
        // audio in before the next step, or a slow host overflows the capture queue and the
        // sweep reports a dropout that the test, not the daemon, caused.
        handed_on(ka, end_sample(d));
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
    Sub,
    FakeDriver,
    LeaseToken,
) {
    init_log();
    let backend = distorting_rig();
    let h = Daemon::start(config(backend.clone(), inproc("sweep"))).unwrap();
    let (mut c, sub) = connect(&h, &[b"evt"]);
    let ka = Sub::connect(h.context(), h.data_endpoint(), &[b"ka"]);
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
    (h, backend, c, sub, ka, d, token)
}

#[test]
fn slow_sweep_measures_the_rigs_harmonics_from_an_empty_daemon() {
    let (_h, _b, mut c, sub, ka, mut d, token) = setup();

    // A sweep measurement is settings only: creating it plays nothing, and it has no job.
    let meas = create(&mut c, "Genelec 1 m", request(LEVEL));
    let loud = create(&mut c, "too loud", request(-5.0));
    let st = match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state,
        other => panic!("{other:?}"),
    };
    assert!(!st.generator.armed && !st.generator.firing && st.sweep.is_none());
    for cmd in [
        Command::MeasStart { meas },
        Command::MeasReset { meas },
        Command::TraceCapture {
            meas,
            name: "x".into(),
            slot: None,
        },
    ] {
        assert_eq!(c.call(cmd).unwrap_err().code, ErrorCode::Invalid);
    }
    let bad = MeasKind::Sweep {
        config: SweepConfig {
            level: Dbfs(f64::NAN),
            ..request(LEVEL)
        },
    };
    let e = c
        .call(Command::MeasCreate {
            config: MeasConfig {
                name: "no level".into(),
                kind: bad,
            },
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{}", e.msg);

    // Refused before it is armed, above the ceiling.
    let capture = |c: &mut Client, meas: MeasId| {
        c.call(Command::SweepRun {
            lease_token: token,
            meas,
            name: None,
        })
    };
    let e = capture(&mut c, meas).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{}", e.msg);
    assert!(e.msg.contains("arm"), "{}", e.msg);
    arm(&mut c, token);
    let e = capture(&mut c, loud).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused);
    assert!(e.msg.contains("maximum"), "{}", e.msg);

    let ReplyBody::Sweep(run0) = capture(&mut c, meas).unwrap() else {
        panic!("not a sweep");
    };
    assert_eq!((run0.meas, run0.name.as_str()), (meas, "Run 1"));
    // Its measurement cannot go while the run plays.
    let e = c
        .call(Command::MeasDelete {
            meas,
            traces: OwnedTraces::Keep,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{}", e.msg);
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

    let done = run_until(&mut d, &mut c, &sub, &ka, token, |r| {
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
    let e = capture(&mut c, meas).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{}", e.msg);
    assert!(e.msg.contains("arm"), "{}", e.msg);

    let data = match c.ok(Command::TraceGet { trace }) {
        ReplyBody::TraceData(t) => *t,
        other => panic!("{other:?}"),
    };
    assert_eq!(data.meta.kind, TraceKind::Sweep);
    assert!(matches!(
        &data.meta.source,
        TraceSource::Sweep { meas: m, number: 1, meas_name, .. } if *m == meas && meas_name == "Genelec 1 m"
    ));
    assert_eq!(data.meta.edit.owner, TraceOwner::Meas { meas });
    assert_eq!(data.meta.edit.name, "Run 1");
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
    assert_eq!(back.edit.owner, TraceOwner::Imported);
    assert!((back.delay.0 - data.meta.delay.0).abs() < 1e-12);
    assert!(matches!(&back.source, TraceSource::Imported { notes, .. } if notes.is_empty()));
    let again = match c.ok(Command::TraceGet { trace: back.id }) {
        ReplyBody::TraceData(t) => *t,
        other => panic!("{other:?}"),
    };
    let s2 = again.sweep.expect("sweep data after import");
    assert_eq!((s2.ir, s2.info), (s.ir, s.info));
    assert_eq!(s2.harmonics.len(), s.harmonics.len());

    // Run again: the measurement's settings, the next number, under the same measurement.
    arm(&mut c, token);
    let run1 = start(&mut c, token, meas);
    assert_eq!(run1.name, "Run 2");
    let done = run_until(&mut d, &mut c, &sub, &ka, token, |r| {
        r.id == run1.id && !r.active()
    });
    let SweepStatus::Done { trace: second } = done.status else {
        panic!("sweep failed: {:?}", done.status);
    };
    let st = match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state,
        other => panic!("{other:?}"),
    };
    let t2 = st.traces.iter().find(|t| t.id == second).unwrap();
    assert_eq!(t2.edit.owner, TraceOwner::Meas { meas });
    assert!(matches!(t2.source, TraceSource::Sweep { number: 2, .. }));

    // Deleted with its runs kept: they move to the imported group.
    c.ok(Command::MeasDelete {
        meas,
        traces: OwnedTraces::Keep,
    });
    let st = match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state,
        other => panic!("{other:?}"),
    };
    assert!(st.measurements.iter().all(|m| m.id != meas));
    for id in [trace, second] {
        let t = st.traces.iter().find(|t| t.id == id).unwrap();
        assert_eq!(t.edit.owner, TraceOwner::Imported, "{}", t.edit.name);
    }
}

#[test]
fn stopping_or_losing_the_lease_discards_the_run() {
    let (_h, _b, mut c, sub, ka, mut d, token) = setup();
    let meas = create(&mut c, "sweep", request(LEVEL));
    arm(&mut c, token);
    let r = start(&mut c, token, meas);
    run(&mut d, 0.5);
    c.ok(Command::GenStop);
    let failed = run_until(&mut d, &mut c, &sub, &ka, token, |x| {
        x.id == r.id && !x.active()
    });
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
    let r = start(&mut c, token, meas);
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
    let (_h, _b, mut c, sub, ka, mut d, token) = setup();
    let meas = create(
        &mut c,
        "unheard",
        SweepConfig {
            outputs: vec![1],
            ..request(LEVEL)
        },
    );
    arm(&mut c, token);
    let r = start(&mut c, token, meas);
    let ended = run_until(&mut d, &mut c, &sub, &ka, token, |x| {
        x.id == r.id && !x.active()
    });
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
    let ka = Sub::connect(h.context(), h.data_endpoint(), &[b"ka"]);
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
    let req = SweepConfig {
        reference_input: 0,
        measurement_input: 2,
        sweep: EssSpec::with_fades(Hz(100.0), Hz(10_000.0), Seconds(1.0)),
        tail: Some(Seconds(2.0)),
        lf_harmonics: ac2_proto::model::LfHarmonics::Standard,
        ..request(LEVEL)
    };
    let meas = create(&mut c, "hall", req);
    arm(&mut c, token);
    let r = start(&mut c, token, meas);
    assert!((r.post_roll.0 - 2.0).abs() < 1e-3, "{:?}", r.post_roll);
    let done = run_until(&mut d, &mut c, &sub, &ka, token, |x| {
        x.id == r.id && !x.active()
    });
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

#[test]
fn slow_fine_lf_harmonics_report_h2_below_20_hz() {
    use ac2_proto::model::LfHarmonics;
    let (_h, _b, mut c, sub, ka, mut d, token) = setup();
    // Lowest frequency with a valid H2 point, the run's silence after the sweep, and the
    // setting its trace records.
    let mut sweep = |lf_harmonics: LfHarmonics| {
        let config = SweepConfig {
            sweep: EssSpec::with_fades(Hz(10.0), Hz(2000.0), Seconds(5.5)),
            lf_harmonics,
            ..request(LEVEL)
        };
        let meas = create(&mut c, &format!("{lf_harmonics:?}"), config);
        arm(&mut c, token);
        let r = start(&mut c, token, meas);
        assert_eq!(r.lf_harmonics, lf_harmonics, "the run echoes it");
        let done = run_until(&mut d, &mut c, &sub, &ka, token, |x| {
            x.id == r.id && !x.active()
        });
        let SweepStatus::Done { trace } = done.status else {
            panic!("sweep failed: {:?}", done.status);
        };
        let data = match c.ok(Command::TraceGet { trace }) {
            ReplyBody::TraceData(t) => *t,
            other => panic!("{other:?}"),
        };
        let TraceSource::Sweep {
            lf_harmonics: stored,
            ..
        } = data.meta.source
        else {
            panic!("{:?}", data.meta.source);
        };
        let s = data.sweep.expect("sweep data");
        let freqs = ac2_traces::frequencies(&match c.ok(Command::GridGet {
            grid_id: data.meta.grid_id,
        }) {
            ReplyBody::Grid(g) => g,
            other => panic!("{other:?}"),
        });
        let h2 = s.harmonics.iter().find(|h| h.order == 2).expect("H2");
        let lowest = (0..freqs.len())
            .find(|&i| h2.curve.valid(i, s.info.floor_margin))
            .map_or(f64::INFINITY, |i| freqs[i]);
        (lowest, r.post_roll.0, stored)
    };
    let (low_s, roll_s, stored_s) = sweep(LfHarmonics::Standard);
    let (low_f, roll_f, stored_f) = sweep(LfHarmonics::Fine);
    eprintln!(
        "H2 from {low_s:.1} Hz (standard), {low_f:.1} Hz (fine); post-roll {roll_s:.2} → {roll_f:.2} s"
    );
    assert_eq!(
        (stored_s, stored_f),
        (LfHarmonics::Standard, LfHarmonics::Fine)
    );
    assert!(low_s >= 20.0, "standard reports H2 from {low_s:.1} Hz");
    assert!(low_f < 20.0, "fine reports H2 from {low_f:.1} Hz");
    assert!(
        roll_f > roll_s,
        "fine waits longer: {roll_s:.2} → {roll_f:.2} s"
    );
}

/// The ac2 CSV's table: column name → values, in grid order.
fn csv_columns(csv: &str) -> std::collections::BTreeMap<String, Vec<f64>> {
    let mut lines = csv.lines().skip_while(|l| !l.starts_with("freq_hz,"));
    let names: Vec<String> = lines
        .next()
        .expect("column header")
        .split(',')
        .map(str::to_owned)
        .collect();
    let mut cols: std::collections::BTreeMap<String, Vec<f64>> =
        names.iter().map(|n| (n.clone(), Vec::new())).collect();
    for l in lines.take_while(|l| !l.starts_with('#')) {
        for (n, v) in names.iter().zip(l.split(',')) {
            cols.get_mut(n)
                .unwrap()
                .push(v.parse::<f64>().unwrap_or(f64::NAN));
        }
    }
    cols
}

/// The rule "a correction put on an input is used wherever the input is": a sweep whose
/// measurement input has a mic curve in use stores its trace with that curve applied after
/// capture (its columns stay of the raw recordings), normalised as the live jobs normalise
/// it. The served magnitude is the measured one − c(f); each harmonic Hk, a ratio of what
/// the mic picked up at k·f to what it picked up at f, moves by −(c(k·f) − c(f)); phase is
/// untouched. Taking the curve off serves the measured columns again.
#[test]
fn a_sweep_through_a_corrected_mic_applies_its_curve() {
    use ac2_core::mic_curve::MicCurve;
    // Flat to 1 kHz (the normalisation point without a calibration), +6 dB from 4 kHz.
    const CURVE: &str = "* test mic\n20 0\n1000 0\n4000 6\n24000 6\n";
    let (_h, _b, mut c, sub, ka, mut d, token) = setup();
    c.ok(Command::CalCurveImport {
        mic: "M30".into(),
        label: None,
        file_name: "M30.frd".into(),
        content: Blob(CURVE.as_bytes().to_vec()),
        input: Some(1),
    });
    let k = MicCurve::from_points(&[(20.0, 0.0), (1000.0, 0.0), (4000.0, 6.0), (24000.0, 6.0)])
        .unwrap()
        .normalised(1000.0);
    let meas = create(&mut c, "sweep", request(LEVEL));
    arm(&mut c, token);
    let r = start(&mut c, token, meas);
    let done = run_until(&mut d, &mut c, &sub, &ka, token, |x| {
        x.id == r.id && !x.active()
    });
    let SweepStatus::Done { trace } = done.status else {
        panic!("sweep failed: {:?}", done.status);
    };
    let shown = match c.ok(Command::TraceGet { trace }) {
        ReplyBody::TraceData(t) => *t,
        other => panic!("{other:?}"),
    };
    let meta = &shown.meta;
    assert_eq!(
        meta.mic
            .as_ref()
            .map(|m| (m.name.as_str(), m.curve.is_none())),
        Some(("M30", true)),
        "the columns are of the raw recordings"
    );
    let mc = meta.mic_curve.as_ref().expect("the input's curve applied");
    assert_eq!((mc.mic.as_str(), mc.curve.label.as_str()), ("M30", "M30"));
    assert_eq!(mc.f_norm, Hz(1000.0));

    let csv = match c.ok(Command::TraceExport {
        trace,
        format: ac2_proto::model::ExportFormat::Ac2Csv,
    }) {
        ReplyBody::Export { content, .. } => String::from_utf8(content.0).unwrap(),
        other => panic!("{other:?}"),
    };
    assert!(
        csv.contains("# mic: M30 (curve: M30, applied after capture as a display edit"),
        "{}",
        &csv[..1500]
    );
    let raw = csv_columns(&csv);
    let f = &raw["freq_hz"];
    let s = shown.sweep.as_ref().expect("sweep data");
    let mut checked = (0, 0);
    for (i, fi) in f.iter().enumerate() {
        let (a, b) = (raw["mag_db"][i], f64::from(shown.mag_db[i]));
        if a.is_finite() {
            assert!((a - b - k.db(*fi)).abs() < 1e-3, "{fi} Hz: {a} → {b}");
            checked.0 += 1;
        }
        for h in &s.harmonics {
            let n = f64::from(h.order);
            let a = raw[&format!("h{}_db", h.order)][i];
            let b = f64::from(h.curve.level_db[i]);
            if a.is_finite() {
                let want = k.db(n * fi) - k.db(*fi);
                assert!(
                    (a - b - want).abs() < 1e-3,
                    "H{} at {fi} Hz: {a} → {b}",
                    h.order
                );
                if want.abs() > 1.0 {
                    checked.1 += 1;
                }
            }
        }
    }
    assert!(checked.0 > 100 && checked.1 > 10, "{checked:?}");
    // Phase is never corrected.
    let p = &raw["phase_deg"];
    let sp = shown.phase_deg.as_ref().expect("phase");
    assert!(
        p.iter()
            .zip(sp)
            .all(|(a, b)| !a.is_finite() || (a - f64::from(*b)).abs() < 1e-3)
    );

    // Taken off, the trace serves what was measured.
    c.ok(Command::TraceMicCurve { trace, curve: None });
    let bare = match c.ok(Command::TraceGet { trace }) {
        ReplyBody::TraceData(t) => *t,
        other => panic!("{other:?}"),
    };
    for (i, a) in raw["mag_db"].iter().enumerate() {
        if a.is_finite() {
            assert!((a - f64::from(bare.mag_db[i])).abs() < 1e-3);
        }
    }
}
