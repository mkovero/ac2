//! Standing evidence: a paired rig's always-on levels relate the two rigs' clocks with no
//! remote input, no audio between them and no app subscribed to anything.
#![allow(clippy::unwrap_used)]

use std::time::{Duration, Instant};

use ac2_audio::fake::{FakeDrive, Pace};
use ac2_audio::{FakeBackend, FakeConfig};
use ac2_proto::frame::FrameData;
use ac2_proto::model::{
    CantTellReason, NotLockedReason, PeerRelation, RemoteLink, RemoteState, RigPeer, SessionConfig,
    StandingRig,
};
use ac2_proto::units::{RigId, WallNs};
use ac2_proto::{Command, Topic};
use ac2_zmq::{Context, CurveClient, KeyPair};
use ac2d::Handle;

use crate::common::*;
use crate::remote_inputs::{Net, client, remote, rig, rig_with, wait_remote};

struct Rigs {
    _dir: tempfile::TempDir,
    app: KeyPair,
    a: Handle,
    b: Handle,
    ca: Client,
    cb: Client,
    a_id: RigId,
    b_id: RigId,
    a_keys: KeyPair,
}

/// Rig A paired with rig B, which admits it; both sessions open at `rate_b` on B, and no
/// remote input anywhere.
fn rigs(rate_b: u32) -> Rigs {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let app = KeyPair::generate().unwrap();
    let a_keys = KeyPair::generate().unwrap();
    let b = if rate_b == FS {
        rig(dir.path(), "rig-b", &app, None)
    } else {
        let backend = FakeBackend::new(FakeConfig {
            sample_rate: rate_b,
            block_frames: BLOCK,
            inputs: 2,
            outputs: 2,
            drive: FakeDrive::Thread(Pace::Realtime),
            ..FakeConfig::default()
        })
        .unwrap();
        rig_with(dir.path(), "rig-b", &app, None, None, Net::Any, backend)
    };
    let a = rig(dir.path(), "rig-a", &app, Some(&a_keys));
    let ctx = Context::new().unwrap();
    let (mut cb, b_id) = client(&ctx, &b, &app);
    let (mut ca, a_id) = client(&ctx, &a, &app);
    cb.ok(Command::SessionOpen { config: at(rate_b) });
    ca.ok(Command::SessionOpen { config: at(FS) });
    cb.ok(Command::AuthAdmitRig {
        key: a_keys.public.to_z85(),
        rig_id: a_id,
        name: "rig-a".into(),
    });
    ca.ok(Command::RigPair {
        peer: RigPeer {
            rig: b_id,
            name: "rig-b".into(),
            ctrl: net_endpoints(&b).0.as_str().to_owned(),
            data: net_endpoints(&b).1.as_str().to_owned(),
            key: b.server_public_key().unwrap().to_z85(),
        },
    });
    Rigs {
        _dir: dir,
        app,
        a,
        b,
        ca,
        cb,
        a_id,
        b_id,
        a_keys,
    }
}

fn at(rate: u32) -> SessionConfig {
    SessionConfig {
        sample_rate_hz: Some(rate),
        ..session(false)
    }
}

fn standing(r: &RemoteState) -> Option<&StandingRig> {
    r.standing.first()
}

fn marks_since(r: &RemoteState) -> Option<WallNs> {
    standing(r).and_then(|s| s.marks_since)
}

/// How long marks have been paired, s, by this host's wall clock (both rigs run on it).
fn marks_for(r: &RemoteState) -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    marks_since(r).map_or(0.0, |s| now.saturating_sub(s.0) as f64 * 1e-9)
}

fn measuring_for(r: &RemoteState, s: f64) -> bool {
    standing(r).is_some_and(|x| {
        x.link == RemoteLink::Up
            && x.clock.relation == PeerRelation::Measuring
            && x.session.as_ref().map(|s| s.channels.as_slice()) == Some(&[0, 1][..])
    }) && marks_for(r) >= s
}

fn cant_tell(r: &RemoteState, why: CantTellReason) -> bool {
    standing(r).is_some_and(|x| x.clock.relation == PeerRelation::CantTell { reason: why })
}

#[test]
fn the_clocks_are_measured_from_levels_alone_and_start_again_on_a_reopen() {
    let mut t = rigs(FS);
    wait_remote(&mut t.ca, "measuring from marks", |r| measuring_for(r, 2.0));
    let r = remote(&mut t.ca);
    // The audio-fed relation has nothing: no remote input, no audio.
    assert!(r.inputs.is_empty(), "{r:#?}");
    let s = standing(&r).unwrap();
    assert_eq!(s.rig, t.b_id);
    assert_eq!(s.name, "rig-b");
    assert_eq!(s.session.as_ref().unwrap().sample_rate_hz, FS);

    // B closes: its frames stop, A says whose session is missing.
    t.cb.ok(Command::SessionClose);
    wait_remote(&mut t.ca, "B has no session", |r| {
        cant_tell(r, CantTellReason::NoSession { rig: t.b_id })
            && standing(r).is_some_and(|s| s.session.is_none())
    });
    // Absent: no count shown as if it went on.
    assert_eq!(marks_for(&remote(&mut t.ca)), 0.0);
    // B again: a new run, the marks counted from its start.
    let opened = Instant::now();
    t.cb.ok(Command::SessionOpen { config: at(FS) });
    wait_remote(&mut t.ca, "measuring again", |r| measuring_for(r, 1.0));
    let since = opened.elapsed().as_secs_f64();
    assert!(marks_for(&remote(&mut t.ca)) <= since + 1.0);

    // A closes: B's frames still come, A has nothing to pair them with.
    t.ca.ok(Command::SessionClose);
    wait_remote(&mut t.ca, "A has no session", |r| {
        cant_tell(r, CantTellReason::NoSession { rig: t.a_id })
    });
    t.ca.ok(Command::SessionOpen { config: at(FS) });
    wait_remote(&mut t.ca, "measuring after A's reopen", |r| {
        measuring_for(r, 1.0) && marks_for(r) < 5.0
    });
    t.a.shutdown();
    t.b.shutdown();
}

#[test]
fn a_link_gap_leaves_a_hole_and_keeps_the_count() {
    let mut t = rigs(FS);
    wait_remote(&mut t.ca, "measuring", |r| measuring_for(r, 2.0));
    let since = marks_since(&remote(&mut t.ca)).unwrap();
    t.cb.ok(Command::AuthRevokeRig { rig_id: t.a_id });
    wait_remote(&mut t.ca, "link down", |r| {
        cant_tell(r, CantTellReason::LinkDown)
            && standing(r).is_some_and(|s| s.link == RemoteLink::NotAdmitted)
    });
    // Down: no figure shown as if it were current.
    let r = remote(&mut t.ca);
    let s = standing(&r).unwrap();
    assert!(s.marks_since.is_none() && s.clock.ppm.is_none(), "{s:#?}");
    // Longer than a hole: the windows break there, the count goes on.
    std::thread::sleep(Duration::from_secs(3));
    t.cb.ok(Command::AuthAdmitRig {
        key: t.a_keys.public.to_z85(),
        rig_id: t.a_id,
        name: "rig-a".into(),
    });
    wait_remote(&mut t.ca, "measuring after the gap", |r| {
        measuring_for(r, 1.0) && marks_since(r) == Some(since)
    });
    t.a.shutdown();
    t.b.shutdown();
}

#[test]
fn a_steady_pair_commits_no_state_second_after_second() {
    let mut t = rigs(FS);
    wait_remote(&mut t.ca, "measuring", |r| measuring_for(r, 2.0));
    let first = remote(&mut t.ca);
    for _ in 0..8 {
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(remote(&mut t.ca), first);
    }
    t.a.shutdown();
    t.b.shutdown();
}

#[test]
fn another_nominal_rate_is_not_locked() {
    let mut t = rigs(96_000);
    wait_remote(&mut t.ca, "another rate", |r| {
        standing(r).is_some_and(|s| {
            s.clock.relation
                == PeerRelation::NotLocked {
                    reason: NotLockedReason::OtherRate {
                        theirs: 96_000,
                        ours: FS,
                    },
                }
                && s.session.as_ref().map(|x| x.sample_rate_hz) == Some(96_000)
        })
    });
    // Nothing fitted across two rates.
    let r = remote(&mut t.ca);
    assert!(standing(&r).unwrap().clock.ppm.is_none(), "{r:#?}");
    t.a.shutdown();
    t.b.shutdown();
}

#[test]
fn source_levels_go_to_an_admitted_rig_only() {
    let mut t = rigs(FS);
    wait_remote(&mut t.ca, "measuring", |r| measuring_for(r, 0.5));
    let ctx = Context::new().unwrap();
    let curve = CurveClient {
        keys: t.app.clone(),
        server_key: t.b.server_public_key().unwrap(),
    };
    let levels = |f: &ac2_proto::frame::Frame| matches!(f.data, FrameData::SourceLevels(_));
    let session = |f: &ac2_proto::frame::Frame| matches!(f.data, FrameData::SessionLevels(_));
    // An app asks for both: its session's levels come, the source levels never do.
    let app = Sub::connect_with(
        &ctx,
        net_endpoints(&t.b).1.as_str(),
        &[
            Topic::SourceLevels.to_string().as_bytes(),
            Topic::SessionLevels.to_string().as_bytes(),
        ],
        Some(&curve),
        None,
    );
    assert!(app.frame(Duration::from_secs(5), session).is_some());
    assert!(app.frame(Duration::from_secs(2), levels).is_none());
    // The admitted rig had them all along: its marks kept counting.
    let r = remote(&mut t.ca);
    assert!(measuring_for(&r, 1.5), "{r:#?}");
    t.a.shutdown();
    t.b.shutdown();
}

/// A minute of marks between two fake rigs on one true clock: the first whole minute within
/// the rule is agreement.
#[test]
fn slow_marks_alone_agree_after_a_minute() {
    let mut t = rigs(FS);
    let t0 = Instant::now();
    loop {
        let r = remote(&mut t.ca);
        let s = standing(&r).filter(|s| s.clock.relation == PeerRelation::Agree);
        if let Some(s) = s {
            assert!(s.clock.ppm.unwrap().abs() < 2.0, "{s:#?}");
            assert!(marks_for(&r) >= 59.0, "{s:#?}");
            assert_eq!(s.counting, None);
            break;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(150),
            "no agreement: {r:#?}"
        );
        std::thread::sleep(Duration::from_millis(500));
    }
    t.a.shutdown();
    t.b.shutdown();
}
