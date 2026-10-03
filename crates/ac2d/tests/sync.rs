//! Ctrl protocol and state sync (Q5): hello and version refusal, snapshot + events, a missed
//! final patch, expired replay, daemon restart, request dedup and `expect_rev`.
#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use ac2_proto::event::Change;
use ac2_proto::frame::FrameData;
use ac2_proto::model::State;
use ac2_proto::units::{MeasId, Rev};
use ac2_proto::{
    Command, DataMessage, ErrorCode, ErrorDetail, Event, PROTO_VERSION, ReplyBody, Request,
};
use ac2d::{Daemon, ReplayLimits};
use common::*;

const T: Duration = Duration::from_secs(5);

fn snapshot(c: &mut Client) -> (State, Rev, u64) {
    match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => (s.state, s.rev, s.daemon_incarnation.0),
        other => panic!("{other:?}"),
    }
}

fn apply(state: &mut State, e: &Event) {
    // Mirror application is plain assignment of the entity's new value.
    match &e.change {
        Change::Measurement(ac2_proto::Patch::Set(m)) => {
            match state.measurements.iter().position(|x| x.id == m.id) {
                Some(i) => state.measurements[i] = m.clone(),
                None => state.measurements.push(m.clone()),
            }
        }
        Change::Measurement(ac2_proto::Patch::Deleted(id)) => {
            state.measurements.retain(|x| x.id != *id);
        }
        Change::Generator(g) => state.generator = g.clone(),
        Change::Session(s) => state.session = s.clone(),
        Change::Timing(t) => state.timing = *t,
        Change::SplLog(ac2_proto::Patch::Set(l)) => {
            match state.spl_logs.iter().position(|x| x.meas == l.meas) {
                Some(i) => state.spl_logs[i] = l.clone(),
                None => state.spl_logs.push(l.clone()),
            }
        }
        Change::SplLog(ac2_proto::Patch::Deleted(id)) => {
            state.spl_logs.retain(|x| x.meas != *id);
        }
        other => panic!("unexpected change in this test: {other:?}"),
    }
}

#[test]
fn hello_and_version_refusal() {
    init_log();
    let h = Daemon::start(config(manual_rig(), inproc("hello"))).unwrap();
    let (mut c, _s) = connect(&h, &[]);
    match c.ok(Command::Hello {
        client: "test".into(),
    }) {
        ReplyBody::Welcome(w) => {
            assert_eq!(w.daemon_incarnation.0, h.incarnation());
            assert!(w.server.contains("(build "), "{}", w.server);
            assert!(w.client_id.0.starts_with("local-"));
            assert_eq!(w.rev, Rev(0));
        }
        other => panic!("{other:?}"),
    }

    // Another version: refused with a typed error at the daemon's version.
    let mut req = Request::new(c.next_id(), Command::SessionStatus);
    req.v = PROTO_VERSION + 1;
    let r = c.call_req(req);
    let e = r.result.unwrap_err();
    assert_eq!(e.code, ErrorCode::VersionMismatch);
    assert_eq!(
        e.detail,
        Some(ErrorDetail::Version {
            daemon: PROTO_VERSION,
            client: PROTO_VERSION + 1
        })
    );

    // No version at all: refused, never assumed.
    #[derive(serde::Serialize)]
    struct NoVersion {
        id: u64,
        cmd: Command,
    }
    let raw = rmp_serde::to_vec_named(&NoVersion {
        id: 77,
        cmd: Command::SessionStatus,
    })
    .unwrap();
    c.sock.send(&[raw]).unwrap();
    let r = c.recv(T).unwrap();
    assert_eq!(r.id.0, 77);
    let e = r.result.unwrap_err();
    assert_eq!(e.code, ErrorCode::VersionMismatch);
    assert_eq!(
        e.detail,
        Some(ErrorDetail::Version {
            daemon: PROTO_VERSION,
            client: 0
        })
    );

    // Malformed body at the right version: `invalid`, and the daemon keeps serving.
    #[derive(serde::Serialize)]
    struct Bogus {
        v: u16,
        id: u64,
        cmd: &'static str,
    }
    let raw = rmp_serde::to_vec_named(&Bogus {
        v: PROTO_VERSION,
        id: 78,
        cmd: "nope",
    })
    .unwrap();
    c.sock.send(&[raw]).unwrap();
    let r = c.recv(T).unwrap();
    assert_eq!(r.result.unwrap_err().code, ErrorCode::Invalid);
    assert!(matches!(
        c.ok(Command::SessionStatus),
        ReplyBody::Session(_)
    ));
    h.shutdown();
}

#[test]
fn snapshot_events_missed_final_patch_and_expired_replay() {
    init_log();
    let mut cfg = config(manual_rig(), inproc("sync"));
    cfg.replay = ReplayLimits {
        events: 8,
        age: Duration::from_secs(60),
    };
    let h = Daemon::start(cfg).unwrap();
    // Observer A follows the Q5 procedure; B mutates.
    let (mut a, sub) = connect(&h, &[b"evt", b"ka"]);
    let mut b = Client::connect(h.context(), h.ctrl_endpoint());
    sub.ka(T).expect("ka proves the subscription is live");
    let (mut mirror, r0, inc) = snapshot(&mut a);
    assert_eq!(inc, h.incarnation());

    for i in 0..3 {
        b.ok(Command::MeasCreate {
            config: transfer(&format!("m{i}")),
        });
    }
    b.ok(Command::MeasUpdate {
        meas: MeasId(2),
        config: spl("now an spl", 0),
    });
    b.ok(Command::MeasDelete { meas: MeasId(1) });
    let target = snapshot(&mut b);
    // Three creates, the update (the meter's `spl_log` entity, then the measurement), the
    // delete.
    assert_eq!(target.1, Rev(r0.0 + 6));

    // Apply live events, but lose the final one.
    let mut last = r0;
    let mut events = Vec::new();
    while events.len() < 6 {
        if let Some(DataMessage::Event(e)) = sub.next(T) {
            events.push(e);
        }
    }
    for e in &events[..5] {
        assert_eq!(e.rev, Rev(last.0 + 1), "events arrive in rev order");
        apply(&mut mirror, e);
        last = e.rev;
    }
    // A keepalive shows rev ahead of what was applied: fetch the gap.
    let ka = sub
        .frame(
            T,
            |f| matches!(&f.data, FrameData::Ka(m) if m.rev == target.1),
        )
        .expect("keepalive carries the current rev");
    let FrameData::Ka(meta) = &ka.data else {
        unreachable!()
    };
    assert_eq!(meta.rev, target.1);
    assert!(meta.rev > last, "missed final patch is visible on ka");
    match a.ok(Command::StateSince { rev: last }) {
        ReplyBody::Events(evs) => {
            assert_eq!(evs.len(), 1);
            for e in &evs {
                apply(&mut mirror, e);
                last = e.rev;
            }
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(last, target.1);
    assert_eq!(mirror, target.0, "mirror equals the daemon state");

    // Expired replay: more than 8 commits later, since(old) must resync.
    for _ in 0..10 {
        b.ok(Command::MeasFreeze {
            meas: MeasId(3),
            frozen: true,
        });
    }
    let e = a.call(Command::StateSince { rev: last }).unwrap_err();
    assert_eq!(e.code, ErrorCode::ResyncRequired);
    let Some(ErrorDetail::Resync { oldest }) = e.detail else {
        panic!("{e:?}")
    };
    assert!(oldest > Rev(last.0 + 1));
    // Recovery is a fresh snapshot.
    let (s2, r2, _) = snapshot(&mut a);
    assert_eq!(r2, Rev(last.0 + 10));
    assert!(s2.measurements.iter().all(|m| m.id != MeasId(1)));
    h.shutdown();
}

#[test]
fn request_dedup_and_expect_rev() {
    init_log();
    let h = Daemon::start(config(manual_rig(), inproc("dedup"))).unwrap();
    let (mut c, _s) = connect(&h, &[]);

    // The same id twice: one execution, the stored reply returned verbatim.
    let id = c.next_id();
    let req = Request::new(
        id,
        Command::MeasCreate {
            config: transfer("once"),
        },
    );
    c.send(&req);
    let first = c.recv_raw(T).unwrap();
    c.send(&req);
    let second = c.recv_raw(T).unwrap();
    assert_eq!(first, second);
    let (s, rev, _) = snapshot(&mut c);
    assert_eq!(s.measurements.len(), 1);
    assert_eq!(rev, Rev(1));

    // expect_rev: stale → conflict with the current rev; current → executes.
    let mut stale = Request::new(c.next_id(), Command::MeasStart { meas: MeasId(1) });
    stale.expect_rev = Some(Rev(0));
    let e = c.call_req(stale).result.unwrap_err();
    assert_eq!(e.code, ErrorCode::Conflict);
    assert_eq!(e.detail, Some(ErrorDetail::Conflict { rev: Rev(1) }));
    let mut fresh = Request::new(
        c.next_id(),
        Command::MeasFreeze {
            meas: MeasId(1),
            frozen: true,
        },
    );
    fresh.expect_rev = Some(Rev(1));
    assert!(c.call_req(fresh).result.is_ok());
    // Reads ignore expect_rev.
    let mut read = Request::new(c.next_id(), Command::SessionStatus);
    read.expect_rev = Some(Rev(0));
    assert!(c.call_req(read).result.is_ok());
    h.shutdown();
}

#[test]
fn daemon_restart_is_a_new_incarnation() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let h1 = Daemon::start(config(manual_rig(), local(dir.path()))).unwrap();
    let ctrl = h1.ctrl_endpoint().to_string();
    let data = h1.data_endpoint().to_string();
    let ctx = ac2_zmq::Context::new().unwrap();
    let mut c = Client::connect(&ctx, &ctrl);
    let sub = Sub::connect(&ctx, &data, &[b"evt", b"ka"]);
    sub.ka(T).unwrap();
    c.ok(Command::MeasCreate {
        config: transfer("before"),
    });
    let (_, rev1, inc1) = snapshot(&mut c);
    assert_eq!(inc1, h1.incarnation());
    h1.shutdown();

    // Same endpoints, new process state.
    let listen = ac2d::Listen::Local {
        ctrl: ctrl.clone(),
        data: data.clone(),
    };
    let h2 = Daemon::start(config(manual_rig(), listen)).unwrap();
    assert_ne!(h2.incarnation(), inc1);
    // The client's sockets reconnect by themselves; a keepalive reveals the restart.
    let ka = sub
        .frame(Duration::from_secs(10), |f| {
            matches!(f.data, FrameData::Ka(_)) && f.stamp.daemon_incarnation.0 != inc1
        })
        .expect("ka from the new incarnation");
    assert_eq!(ka.stamp.daemon_incarnation.0, h2.incarnation());
    // Old revs mean nothing to the new incarnation.
    let e = c.call(Command::StateSince { rev: rev1 }).unwrap_err();
    assert_eq!(e.code, ErrorCode::ResyncRequired);
    let (s, rev, inc2) = snapshot(&mut c);
    assert_eq!(inc2, h2.incarnation());
    assert_eq!(rev, Rev(0));
    assert!(s.measurements.is_empty());
    assert!(
        s.generator.owner.is_none() && !s.generator.armed,
        "comes up disarmed"
    );
    h2.shutdown();
}
