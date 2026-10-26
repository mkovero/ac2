//! The programme meters (`docs/design/programme-meter.md` §3–§5, §7): `levels.set` is a rig
//! edit kept in the rig settings file, the session meters carry the chosen ballistic, the
//! true peak, the max holds, the overs and each pair's correlation, a sustained over counts
//! once, `levels.reset` clears the holds, and the goniometer frame carries 256 points.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::path::Path;
use std::time::Duration;

use ac2_proto::frame::{FrameData, GONIO_POINTS, SessionLevelsFrame};
use ac2_proto::model::{
    Ballistic, CorrelationWindow, GeneratorDesired, GeneratorSettings, InputKind, InputSetup,
    LevelsSettings, Pair, PairSide, Signal, State,
};
use ac2_proto::units::{Dbfs, Dbtp, Hz, Lufs, Rev};
use ac2_proto::{Command, ErrorCode, ReplyBody, Subscription};
use ac2d::{Daemon, Handle};
use common::*;

const T: Duration = Duration::from_secs(5);

fn start_with_file(dir: &Path) -> Handle {
    init_log();
    let mut cfg = config(manual_rig(), inproc(&unique("levels")));
    cfg.session_dir = dir.join("sessions");
    cfg.rig_settings = Some(dir.join("rig.json"));
    cfg.rig_logs = Some(dir.join("rig-logs"));
    Daemon::start(cfg).unwrap()
}

fn state(c: &mut Client) -> State {
    match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state,
        other => panic!("{other:?}"),
    }
}

fn set(c: &mut Client, base: Rev, f: impl FnOnce(&mut Command)) -> Result<ReplyBody, ErrorCode> {
    let mut cmd = Command::LevelsSet {
        base,
        ballistic: None,
        correlation_window: None,
        over_threshold: None,
        correlation_gate: None,
        loudness_target: None,
    };
    f(&mut cmd);
    c.call(cmd).map_err(|e| e.code)
}

fn line(channel: u16) -> InputSetup {
    InputSetup {
        channel,
        kind: InputKind::Line,
        pair: None,
    }
}

#[test]
fn set_is_a_rig_edit_checked_and_kept_across_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let h = start_with_file(dir.path());
    let (mut c, _s) = connect(&h, &[]);
    let rig = state(&mut c).rig;
    assert_eq!(rig.levels, LevelsSettings::default());
    // Another rev: conflict, nothing changes.
    let stale = Rev(rig.rev.0 + 7);
    let e = set(&mut c, stale, |cmd| {
        if let Command::LevelsSet { ballistic, .. } = cmd {
            *ballistic = Some(Ballistic::Type2);
        }
    })
    .unwrap_err();
    assert_eq!(e, ErrorCode::Conflict);
    // Outside the ranges or not a number: invalid.
    for (thr, gate) in [
        (Some(6.5), None),
        (Some(-20.5), None),
        (Some(f64::NAN), None),
        (None, Some(0.5)),
        (None, Some(-121.0)),
        (None, Some(f64::INFINITY)),
    ] {
        let base = state(&mut c).rig.rev;
        let e = set(&mut c, base, |cmd| {
            if let Command::LevelsSet {
                over_threshold,
                correlation_gate,
                ..
            } = cmd
            {
                *over_threshold = thr.map(Dbtp);
                *correlation_gate = gate.map(Dbfs);
            }
        })
        .unwrap_err();
        assert_eq!(e, ErrorCode::Invalid, "{thr:?} {gate:?}");
    }
    assert_eq!(state(&mut c).rig.levels, LevelsSettings::default());
    // Applied: the fields given change, the rest stay, the rig rev moves.
    let base = state(&mut c).rig.rev;
    let ReplyBody::Ack { rev } = set(&mut c, base, |cmd| {
        if let Command::LevelsSet {
            ballistic,
            correlation_window,
            over_threshold,
            ..
        } = cmd
        {
            *ballistic = Some(Ballistic::Type1);
            *correlation_window = Some(CorrelationWindow::Short);
            *over_threshold = Some(Dbtp(-1.0));
        }
    })
    .unwrap() else {
        panic!("ack");
    };
    let want = LevelsSettings {
        ballistic: Ballistic::Type1,
        correlation_window: CorrelationWindow::Short,
        over_threshold: Dbtp(-1.0),
        correlation_gate: Dbfs(-60.0),
        loudness_target: Lufs(-23.0),
    };
    let rig = state(&mut c).rig;
    assert_eq!(rig.levels, want);
    assert_eq!(rig.rev, rev);
    assert!(rig.rev > base);
    drop(c);
    h.shutdown();
    // The next daemon reads them back from the rig settings file.
    let h = start_with_file(dir.path());
    let (mut c, _s) = connect(&h, &[]);
    assert_eq!(state(&mut c).rig.levels, want);
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("rig.json")).unwrap()).unwrap();
    assert_eq!(json["levels"]["ballistic"], "type1");
    assert_eq!(json["levels"]["correlation_window"], "short");
}

fn levels_frame(f: &ac2_proto::Frame) -> Option<&SessionLevelsFrame> {
    match &f.data {
        FrameData::SessionLevels(l) => Some(l),
        _ => None,
    }
}

/// The first session meter frame satisfying `pred`.
fn meters(
    s: &Sub,
    what: &str,
    mut pred: impl FnMut(&SessionLevelsFrame) -> bool,
) -> SessionLevelsFrame {
    let f = s
        .frame(T, |f| levels_frame(f).is_some_and(&mut pred))
        .unwrap_or_else(|| panic!("no session meter frame: {what}"));
    levels_frame(&f).unwrap().clone()
}

#[test]
fn slow_session_meters_carry_the_programme_columns_and_count_an_over_once() {
    init_log();
    let h = Daemon::start(steady_config(realtime_rig(), inproc("levels-meters"))).unwrap();
    let (mut c, s) = connect(&h, &[&Subscription::InputMeters.prefix()]);
    let mut cfg = session(false);
    cfg.input_channels = vec![0, 1, 2, 3];
    c.ok(Command::SessionOpen { config: cfg });
    // Input 1 hears output 0 through the fake rig's acoustic path, input 2 nothing: a pair
    // of the two has a quiet right side.
    c.ok(Command::SessionInputs {
        inputs: vec![line(0), line(1), line(2)],
        pairs: vec![Pair {
            left: 1,
            right: 2,
            name: None,
        }],
    });
    let base = state(&mut c).rig.rev;
    set(&mut c, base, |cmd| {
        if let Command::LevelsSet { over_threshold, .. } = cmd {
            *over_threshold = Some(Dbtp(-20.0));
        }
    })
    .unwrap();
    // Silence first: every column present, nothing over, the pair quiet on the left (the
    // first side checked).
    let f = meters(&s, "silence", |l| l.meta.pair_levels.len() == 1);
    assert_eq!(f.meta.channels, [0, 1, 2, 3]);
    assert_eq!(f.meta.ballistic, Ballistic::TruePeak);
    assert_eq!(f.ppm.len(), 4);
    assert_eq!(f.true_peak.len(), 4);
    assert_eq!(f.max.len(), 4);
    assert_eq!(f.overs, [0, 0, 0, 0]);
    let p = &f.meta.pair_levels[0];
    assert_eq!((p.left, p.right), (1, 2));
    assert_eq!(p.quiet, Some(PairSide::Left));
    assert_eq!(p.correlation, None);
    // A −12 dBFS tone out of output 0: input 0 (loopback) reads −12 dBTP, input 1
    // (ACOUSTIC_GAIN down) −18; both above −20 dBTP, so both go over, once.
    let acoustic = -12.0 + 20.0 * ACOUSTIC_GAIN.log10();
    let ReplyBody::Lease(lease) = c.ok(Command::GenAcquire { force: false }) else {
        panic!("lease");
    };
    c.ok(Command::GenSet {
        lease_token: lease.lease_token,
        desired: GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Sine { freq: Hz(1000.0) },
                level: Dbfs(-12.0),
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: true,
        },
    });
    let f = meters(&s, "tone", |l| l.overs[1] >= 1 && l.ppm[1] > acoustic - 1.0);
    assert!((f.true_peak[1] - acoustic).abs() < 0.5, "{:?}", f.true_peak);
    assert!((f.ppm[1] - acoustic).abs() < 0.5, "{:?}", f.ppm);
    assert!((f.max[1] - acoustic).abs() < 0.5, "{:?}", f.max);
    assert_eq!(f.overs[0], 1);
    assert_eq!(f.overs[1], 1);
    assert_eq!(f.overs[2], 0);
    let p = &f.meta.pair_levels[0];
    assert_eq!(p.quiet, Some(PairSide::Right));
    assert_eq!(p.correlation, None);
    // A second later the tone still sounds and the count is still one.
    std::thread::sleep(Duration::from_secs(1));
    let f = meters(&s, "sustained", |l| l.ppm[1] > acoustic - 1.0);
    assert_eq!(f.overs[1], 1, "{:?}", f.overs);
    assert!((f.max[1] - acoustic).abs() < 0.5);
    // The goniometer frame of the pair: 256 points, the left side's gain.
    let g = s
        .frame(T, |f| matches!(f.data, FrameData::SessionGonio(_)))
        .expect("session/gonio frame");
    let FrameData::SessionGonio(g) = g.data else {
        unreachable!()
    };
    assert_eq!(g.pairs.len(), 1);
    assert_eq!((g.pairs[0].left, g.pairs[0].right), (1, 2));
    assert_eq!(g.pairs[0].points.len(), GONIO_POINTS);
    assert!(g.pairs[0].points.iter().any(|p| p != &[0, 0]));
    // VU chosen but not offered on a line input without a line calibration: the frame names
    // the choice, the column carries the true peak.
    let base = state(&mut c).rig.rev;
    set(&mut c, base, |cmd| {
        if let Command::LevelsSet { ballistic, .. } = cmd {
            *ballistic = Some(Ballistic::Vu);
        }
    })
    .unwrap();
    let f = meters(&s, "vu", |l| l.meta.ballistic == Ballistic::Vu);
    assert!((f.ppm[1] - acoustic).abs() < 0.5, "{:?}", f.ppm);
    // The tone stopped (the acoustic path's noise floor stays): the holds stay until a
    // reset of those inputs clears them.
    c.ok(Command::GenStop);
    let f = meters(&s, "silence again", |l| l.ppm[1] < -60.0);
    assert_eq!(f.overs[1], 1);
    assert!((f.max[1] - acoustic).abs() < 0.5);
    c.ok(Command::LevelsReset { channels: vec![1] });
    let f = meters(&s, "reset", |l| l.overs[1] == 0);
    assert!(f.max[1] < -60.0, "{:?}", f.max);
    assert_eq!(f.overs[0], 1, "input 0 was not reset");
    c.ok(Command::LevelsReset {
        channels: Vec::new(),
    });
    let f = meters(&s, "reset all", |l| l.overs[0] == 0);
    assert_eq!(f.overs, [0, 0, 0, 0]);
}
