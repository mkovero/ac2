//! Transfer job on a fake acoustic path: magnitude and delay come out right, `delay.set`
//! aligns the phase, the loopback timing monitor locks at the cable delay, and the live IR
//! view is published only while subscribed.
#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use ac2_audio::FakeDriver;
use ac2_proto::frame::{Frame, FrameData, ProtectionFlags, TfFrame, ValidityMask};
use ac2_proto::model::{
    DelayOutcome, DelayPick, DepthPolicy, FinderBand, GeneratorDesired, GeneratorSettings,
    MeasKind, NoEstimateReason, Signal, TimingState,
};
use ac2_proto::units::{Dbfs, Hz, MeasId, Samples, Seconds};
use ac2_proto::{Command, ErrorCode, ReplyBody};
use ac2d::Daemon;
use common::*;

const T: Duration = Duration::from_secs(10);

fn grid_freq(i: usize) -> f64 {
    // The test measurement's grid: 48 ppo, k from -240.
    1000.0 * 2f64.powf((i as f64 - 240.0) / 48.0)
}

/// Runs audio until a TF frame covering it arrives; returns the newest such frame.
fn run_tf(d: &mut FakeDriver, sub: &Sub, seconds: f64, min_rev: u64) -> Frame {
    run(d, seconds);
    let end = end_sample(d);
    sub.frame(T, |f| {
        matches!(f.data, FrameData::Tf(_))
            && f.stamp.audio_sample.0 + 1 >= end
            && f.stamp.config_rev.0 >= min_rev
    })
    .expect("tf frame covering the audio")
}

fn tf(f: &Frame) -> &TfFrame {
    match &f.data {
        FrameData::Tf(t) => t,
        _ => unreachable!(),
    }
}

/// Valid columns between 300 Hz and `hi`.
fn band(t: &TfFrame, hi: f64) -> Vec<usize> {
    (0..t.mag.len())
        .filter(|&i| (300.0..=hi).contains(&grid_freq(i)))
        .filter(|&i| t.validity[i] == ValidityMask::NONE)
        .collect()
}

fn find(band: FinderBand, observation: Option<f64>) -> Command {
    Command::DelayFind {
        meas: MeasId(1),
        band,
        observation: observation.map(Seconds),
    }
}

#[test]
fn depth_policy_is_validated_and_kept() {
    init_log();
    let h = Daemon::start(config(manual_rig(), inproc("depth"))).unwrap();
    let (mut c, _sub) = connect(&h, &[]);
    let with = |depth| {
        let mut m = transfer("lf");
        if let MeasKind::Transfer { config } = &mut m.kind {
            config.depth = depth;
        }
        m
    };
    let e = c
        .call(Command::MeasCreate {
            config: with(DepthPolicy::FastLf {
                max_settle_s: Seconds(0.0),
            }),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    let fast = DepthPolicy::FastLf {
        max_settle_s: Seconds(1.0),
    };
    match c.ok(Command::MeasCreate { config: with(fast) }) {
        ReplyBody::Measurement(m) => match m.config.kind {
            MeasKind::Transfer { config } => assert_eq!(config.depth, fast),
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }
    // The job starts with it.
    c.ok(Command::SessionOpen {
        config: session(false),
    });
    c.ok(Command::MeasStart { meas: MeasId(1) });
    h.shutdown();
}

#[test]
fn transfer_magnitude_delay_timing_and_ir() {
    init_log();
    let backend = manual_rig();
    let mut cfg = config(backend.clone(), inproc("tf"));
    // The lease is not under test here; debug builds run the DSP slower than real time.
    cfg.lease_expiry = Duration::from_secs(60);
    let h = Daemon::start(cfg).unwrap();
    let (mut c, sub) = connect(&h, &[b"d/1/tf"]);
    let timing = Sub::connect(h.context(), h.data_endpoint(), &[b"timing"]);
    c.ok(Command::SessionOpen {
        config: session(true),
    });
    c.ok(Command::MeasCreate {
        config: transfer("main"),
    });
    c.ok(Command::MeasStart { meas: MeasId(1) });
    // Nothing captured yet: the finder refuses rather than guessing, with a typed reason,
    // and the refusal is what the measurement shows.
    match c.ok(find(FinderBand::Auto, None)) {
        ReplyBody::DelayFinding(f) => assert_eq!(
            f.outcome,
            DelayOutcome::NoEstimate {
                reasons: vec![NoEstimateReason::ObservationTooShort]
            }
        ),
        other => panic!("{other:?}"),
    }
    let e = c
        .call(Command::DelayInsert {
            meas: MeasId(1),
            pick: DelayPick::FirstArrival,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    // Requests the finder cannot run as asked are errors, not refusals.
    for (band, obs) in [
        (FinderBand::Sub, Some(3.0)),
        (FinderBand::Full, Some(9.0)),
        (FinderBand::Mid, Some(-1.0)),
        (
            FinderBand::Custom {
                lo_hz: Hz(800.0),
                hi_hz: Hz(80.0),
            },
            None,
        ),
    ] {
        let e = c.call(find(band, obs)).unwrap_err();
        assert_eq!(e.code, ErrorCode::Invalid, "{band:?} {obs:?}: {e:?}");
    }
    let tok = match c.ok(Command::GenAcquire { force: false }) {
        ReplyBody::Lease(l) => l.lease_token,
        other => panic!("{other:?}"),
    };
    c.ok(Command::GenSet {
        lease_token: tok,
        desired: GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Pink,
                level: Dbfs(-20.0),
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: true,
        },
    });
    let mut d = driver(&backend);

    // Unaligned: |H| = gain, phase = −360°·f·D/fs.
    for _ in 0..4 {
        run_tf(&mut d, &sub, 0.5, 0);
        c.ok(Command::GenRefresh { lease_token: tok });
    }
    let f = run_tf(&mut d, &sub, 0.5, 0);
    let t = tf(&f);
    // Unaligned, the phase turns across each display column (0.9°/Hz here), so the column
    // sum of cross-spectra loses magnitude and coherence where columns are wide; check the
    // band where a column spans only a few degrees.
    let cols = band(t, 800.0);
    assert!(cols.len() > 40, "settled columns: {}", cols.len());
    let gain_db = 20.0 * f32::log10(ACOUSTIC_GAIN);
    if std::env::var_os("TF_DUMP").is_some() {
        for &i in &cols {
            eprintln!(
                "{:8.1} {:7.2} {:7.1} {:5.3}",
                grid_freq(i),
                t.mag[i],
                t.phase[i],
                t.coh[i]
            );
        }
    }
    for &i in &cols {
        assert!(
            (t.mag[i] - gain_db).abs() < 0.3,
            "mag at {:.0} Hz: {}",
            grid_freq(i),
            t.mag[i]
        );
        assert!(
            t.coh[i] > 0.98,
            "coherence at {:.0} Hz: {}",
            grid_freq(i),
            t.coh[i]
        );
    }
    let i500 = 240 - 48;
    let expect = -360.0 * 500.0 * f64::from(ACOUSTIC_DELAY) / f64::from(FS);
    let wrapped = (expect + 180.0).rem_euclid(360.0) - 180.0;
    assert!(
        (f64::from(t.phase[i500]) - wrapped).abs() < 5.0,
        "phase at 500 Hz {} vs {wrapped}",
        t.phase[i500]
    );
    assert_eq!(f.stamp.protection.0 & ProtectionFlags::CHECK_ROUTING.0, 0);

    // Insert the delay: phase flat, magnitude unchanged, frames show the new config.
    // The finder (on the raw, unaligned pair) sees the acoustic delay.
    let delay_s = f64::from(ACOUSTIC_DELAY) / f64::from(FS);
    for band in [FinderBand::Auto, FinderBand::Mid, FinderBand::Full] {
        match c.ok(find(band, None)) {
            ReplyBody::DelayFinding(f) => {
                let DelayOutcome::Accepted { first, strongest } = f.outcome else {
                    panic!("{band:?}: {f:?}");
                };
                assert!(
                    (first.delay.0 - delay_s).abs() < 0.5 / f64::from(FS),
                    "{f:?}"
                );
                assert!(
                    (strongest.delay.0 - delay_s).abs() < 0.5 / f64::from(FS),
                    "{f:?}"
                );
                assert!(f.confidence.psr_db.is_some_and(|p| p.0 > 10.0), "{f:?}");
                assert!(f.observation.0 > 0.0);
                if band == FinderBand::Mid {
                    assert_eq!(f.band, ac2_proto::model::DelayBand::Mid);
                }
            }
            other => panic!("{other:?}"),
        }
    }
    // The finder's estimate goes in exactly, fraction included.
    let (rev, inserted) = match c.ok(Command::DelayInsert {
        meas: MeasId(1),
        pick: ac2_proto::model::DelayPick::FirstArrival,
    }) {
        ReplyBody::Measurement(m) => {
            let d = m.delay.unwrap();
            assert!(
                (d.applied_samples - f64::from(ACOUSTIC_DELAY)).abs() < 0.5,
                "{}",
                d.applied_samples
            );
            assert!((d.applied.0 * f64::from(FS) - d.applied_samples).abs() < 1e-6);
            (m.config_rev.0, d.applied_samples)
        }
        other => panic!("{other:?}"),
    };
    for _ in 0..4 {
        run_tf(&mut d, &sub, 0.5, rev);
        c.ok(Command::GenRefresh { lease_token: tok });
    }
    let f = run_tf(&mut d, &sub, 0.5, rev);
    let t = tf(&f);
    assert!(f.stamp.config_applied_at.0 > 0);
    assert!((t.meta.delay.0 - inserted / f64::from(FS)).abs() < 1e-12);
    let cols = band(t, 10_000.0);
    assert!(cols.len() > 200, "settled columns: {}", cols.len());
    for &i in &cols {
        assert!(
            (t.mag[i] - gain_db).abs() < 0.3,
            "mag at {:.0} Hz: {}",
            grid_freq(i),
            t.mag[i]
        );
        assert!(
            t.phase[i].abs() < 3.0,
            "phase at {:.0} Hz: {}",
            grid_freq(i),
            t.phase[i]
        );
        assert!(
            t.coh[i] > 0.99,
            "aligned coherence at {:.0} Hz: {}",
            grid_freq(i),
            t.coh[i]
        );
    }

    // A one-sample nudge keeps the averages: the very next frame shows the phase of the
    // one-sample residual (−360°·f/fs) with no column settling, and a nudge back restores it.
    let nudge = |c: &mut Client, by: f64| match c.ok(Command::DelayNudge {
        meas: MeasId(1),
        by: Seconds(by / f64::from(FS)),
    }) {
        ReplyBody::Measurement(m) => (m.config_rev.0, m.delay.unwrap().applied_samples),
        other => panic!("{other:?}"),
    };
    let (rev, applied) = nudge(&mut c, 1.0);
    assert!((applied - (inserted + 1.0)).abs() < 1e-6, "{applied}");
    let f = run_tf(&mut d, &sub, 0.05, rev);
    let t = tf(&f);
    assert!((t.meta.delay.0 - applied / f64::from(FS)).abs() < 1e-12);
    let cols = band(t, 10_000.0);
    assert!(
        cols.len() > 200,
        "valid columns right after the nudge: {}",
        cols.len()
    );
    assert!(
        t.validity
            .iter()
            .all(|v| !v.contains(ValidityMask::SETTLING))
    );
    for &i in &cols {
        let f_hz = grid_freq(i);
        let want = (360.0 * f_hz / f64::from(FS) + 180.0).rem_euclid(360.0) - 180.0;
        assert!(
            (f64::from(t.phase[i]) - want).abs() < 3.0,
            "phase at {f_hz:.0} Hz: {} vs {want}",
            t.phase[i]
        );
    }
    let (rev, applied) = nudge(&mut c, -1.0);
    assert!((applied - inserted).abs() < 1e-6, "{applied}");
    let t = run_tf(&mut d, &sub, 0.05, rev);
    let t = tf(&t);
    assert!(band(t, 10_000.0).iter().all(|&i| t.phase[i].abs() < 3.0));

    // Tracking: two agreeing windows at the inserted delay leave it where it is.
    match c.ok(Command::DelayTrack {
        meas: MeasId(1),
        enabled: true,
    }) {
        ReplyBody::Measurement(m) => assert!(m.delay.unwrap().tracking),
        other => panic!("{other:?}"),
    }
    run_tf(&mut d, &sub, 1.0, rev);
    match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => assert_eq!(
            s.state.measurements[0]
                .delay
                .as_ref()
                .unwrap()
                .applied_samples,
            inserted
        ),
        other => panic!("{other:?}"),
    }

    // The loopback monitor locked at the cable delay and committed it.
    let locked = TimingState::Locked {
        offset: Samples(i64::from(LOOP_DELAY)),
    };
    let mut last = None;
    let tf_frame = timing
        .frame(Duration::from_secs(2), |f| match &f.data {
            FrameData::Timing(m) => {
                last = Some(m.status);
                m.status.state == locked
            }
            _ => false,
        })
        .unwrap_or_else(|| panic!("no locked timing frame; last {last:?}"));
    let FrameData::Timing(tm) = &tf_frame.data else {
        unreachable!()
    };
    assert_eq!(
        tm.status.state,
        TimingState::Locked {
            offset: Samples(i64::from(LOOP_DELAY))
        }
    );
    match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => assert_eq!(
            s.state.timing.state,
            TimingState::Locked {
                offset: Samples(i64::from(LOOP_DELAY))
            }
        ),
        other => panic!("{other:?}"),
    }

    // IR view: only computed for a subscriber; time zero at the inserted delay, so the
    // arrival sits at t = 0 with the path gain.
    let ir_sub = Sub::connect(h.context(), h.data_endpoint(), &[b"d/1/ir"]);
    std::thread::sleep(Duration::from_millis(50));
    run(&mut d, 0.1);
    let f = ir_sub
        .frame(T, |f| matches!(f.data, FrameData::Ir(_)))
        .expect("ir frame for a subscriber");
    let FrameData::Ir(ir) = &f.data else {
        unreachable!()
    };
    let (imax, vmax) = ir.linear.iter().enumerate().fold((0, 0.0f32), |a, (i, v)| {
        if v.abs() > a.1.abs() { (i, *v) } else { a }
    });
    let t_peak = ir.meta.t0.0 + imax as f64 * ir.meta.dt.0;
    assert!(
        t_peak.abs() < 0.5 / f64::from(FS),
        "arrival at t = {t_peak}"
    );
    assert!((vmax - ACOUSTIC_GAIN).abs() < 0.05, "arrival gain {vmax}");

    c.ok(Command::GenStop);
    let stop = std::thread::spawn(move || h.shutdown());
    while !stop.is_finished() {
        d.run_blocks(4);
        std::thread::sleep(Duration::from_millis(1));
    }
    stop.join().unwrap();
}

/// Two arrivals 200 samples apart, the earlier 12.3 dB under the later: within 2 dB of the
/// first-arrival threshold, so the finder leaves the pick to the operator (decision 1c).
fn two_arrival_rig() -> ac2_audio::FakeBackend {
    use ac2_audio::FakeConfig;
    use ac2_audio::fake::{FakeDrive, FakePath};
    let mut fir = vec![0.0; 201];
    fir[0] = ACOUSTIC_GAIN * 10f32.powf(-12.3 / 20.0);
    fir[200] = ACOUSTIC_GAIN;
    ac2_audio::FakeBackend::new(FakeConfig {
        sample_rate: FS,
        block_frames: BLOCK,
        inputs: 4,
        outputs: 2,
        drive: FakeDrive::Manual,
        seed: 7,
        paths: vec![
            FakePath::loopback(0, 0, LOOP_DELAY),
            FakePath::acoustic(0, 1, LOOP_DELAY + ACOUSTIC_DELAY, fir, 1e-4),
        ],
        ..FakeConfig::default()
    })
    .unwrap()
}

fn delay_state(r: ReplyBody) -> ac2_proto::model::DelayState {
    match r {
        ReplyBody::Measurement(m) => m.delay.unwrap(),
        other => panic!("{other:?}"),
    }
}

fn snapshot_delay(c: &mut Client) -> ac2_proto::model::DelayState {
    match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state.measurements[0].delay.clone().unwrap(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_ambiguous_finding_awaits_a_pick() {
    init_log();
    let backend = two_arrival_rig();
    let mut cfg = config(backend.clone(), inproc("tf-ambiguous"));
    cfg.lease_expiry = Duration::from_secs(60);
    let h = Daemon::start(cfg).unwrap();
    let (mut c, sub) = connect(&h, &[b"d/1/tf"]);
    c.ok(Command::SessionOpen {
        config: session(true),
    });
    c.ok(Command::MeasCreate {
        config: transfer("main"),
    });
    c.ok(Command::MeasStart { meas: MeasId(1) });
    let st = delay_state(c.ok(Command::DelayTrack {
        meas: MeasId(1),
        enabled: true,
    }));
    assert!(st.tracking && !st.awaiting_pick);
    let tok = match c.ok(Command::GenAcquire { force: false }) {
        ReplyBody::Lease(l) => l.lease_token,
        other => panic!("{other:?}"),
    };
    c.ok(Command::GenSet {
        lease_token: tok,
        desired: GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Pink,
                level: Dbfs(-20.0),
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: true,
        },
    });
    let mut d = driver(&backend);
    for _ in 0..6 {
        run_tf(&mut d, &sub, 0.5, 0);
        c.ok(Command::GenRefresh { lease_token: tok });
    }

    let early = f64::from(ACOUSTIC_DELAY) / f64::from(FS);
    match c.ok(find(FinderBand::Full, None)) {
        ReplyBody::DelayFinding(f) => {
            let DelayOutcome::Ambiguous { ranked, .. } = &f.outcome else {
                panic!("{f:?}");
            };
            assert!(
                ranked
                    .iter()
                    .any(|a| (a.delay.0 - early).abs() < 1.0 / f64::from(FS)),
                "{ranked:?}"
            );
        }
        other => panic!("{other:?}"),
    }
    let st = snapshot_delay(&mut c);
    assert!(st.awaiting_pick, "{st:?}");
    assert!(st.tracking, "the operator's switch stays on while paused");

    // Paused tracking moves nothing while more audio arrives.
    let before = st.applied_samples;
    for _ in 0..4 {
        run_tf(&mut d, &sub, 0.5, 0);
        c.ok(Command::GenRefresh { lease_token: tok });
    }
    assert_eq!(snapshot_delay(&mut c).applied_samples, before);

    // The operator picks a candidate: resolved, tracking resumes; the finding stays (the
    // other ranked picks remain insertable).
    let st = delay_state(c.ok(Command::DelayInsert {
        meas: MeasId(1),
        pick: DelayPick::Ranked { index: 0 },
    }));
    assert!(!st.awaiting_pick);
    assert!(st.last_finding.is_some());

    // A new ambiguous finding pauses again; a typed value resolves it too.
    c.ok(find(FinderBand::Full, None));
    assert!(snapshot_delay(&mut c).awaiting_pick);
    let st = delay_state(c.ok(Command::DelaySet {
        meas: MeasId(1),
        delay: Seconds(early),
    }));
    assert!(!st.awaiting_pick);
    assert!(st.last_finding.is_none());

    c.ok(Command::GenStop);
    let stop = std::thread::spawn(move || h.shutdown());
    while !stop.is_finished() {
        d.run_blocks(4);
        std::thread::sleep(Duration::from_millis(1));
    }
    stop.join().unwrap();
}
