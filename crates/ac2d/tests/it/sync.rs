//! Ctrl protocol and state sync (Q5): hello and version refusal, snapshot + events, a missed
//! final patch, expired replay, daemon restart, request dedup.
#![allow(clippy::unwrap_used)]

use crate::common;

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
    let s = c.snapshot(Command::StateSnapshot);
    (s.state, s.rev, s.daemon_incarnation.0)
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
        Change::Stream(s) => state.stream = s.clone(),
        Change::Timing(t) => state.timing = *t,
        Change::Working(w) => state.working = w.clone(),
        Change::SplLog(ac2_proto::Patch::Set(l)) => {
            match state.spl_logs.iter().position(|x| x.meas == l.meas) {
                Some(i) => state.spl_logs[i] = l.clone(),
                None => state.spl_logs.push(l.clone()),
            }
        }
        Change::SplLog(ac2_proto::Patch::Deleted(id)) => {
            state.spl_logs.retain(|x| x.meas != *id);
        }
        Change::Delay(ac2_proto::Patch::Set(d)) => {
            match state.delays.binary_search_by_key(&d.meas, |x| x.meas) {
                Ok(i) => state.delays[i] = d.clone(),
                Err(i) => state.delays.insert(i, d.clone()),
            }
        }
        Change::Delay(ac2_proto::Patch::Deleted(id)) => {
            state.delays.retain(|x| x.meas != *id);
        }
        other => panic!("unexpected change in this test: {other:?}"),
    }
}

#[test]
fn hello_and_version_refusal() {
    init_log();
    let h = Daemon::start(config(manual_rig(), inproc("hello"))).unwrap();
    let (mut c, _s) = connect(&h, &[]);
    let w = c.welcome(Command::Hello {
        client: "test".into(),
    });
    assert_eq!(w.daemon_incarnation.0, h.incarnation());
    assert!(w.server.contains("(build "), "{}", w.server);
    assert!(w.client_id.0.starts_with("local-"));
    assert_eq!(w.rev, Rev(0));

    // Another version: refused with a typed error at the daemon's version.
    let mut req = Request::new(c.next_id(), Command::StreamStatus);
    req.v = PROTO_VERSION + 1;
    let r = c.call_req(req);
    assert_eq!(r.v, PROTO_VERSION);
    let e = r.result.unwrap_err();
    assert_eq!(e.code, ErrorCode::VersionMismatch);
    assert_eq!(e.detail, None);

    // No version at all: refused, never assumed.
    #[derive(serde::Serialize)]
    struct NoVersion {
        id: u64,
        cmd: Command,
    }
    let raw = rmp_serde::to_vec_named(&NoVersion {
        id: 77,
        cmd: Command::StreamStatus,
    })
    .unwrap();
    c.sock.send(&[raw]).unwrap();
    let r = c.recv(T).unwrap();
    assert_eq!(r.id.0, 77);
    assert_eq!(r.v, PROTO_VERSION);
    let e = r.result.unwrap_err();
    assert_eq!(e.code, ErrorCode::VersionMismatch);
    assert_eq!(e.detail, None);

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
    assert!(matches!(c.ok(Command::StreamStatus), ReplyBody::Stream(_)));
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
    b.update_ok(MeasId(2), spl("now an spl", 0));
    b.ok(Command::MeasDelete {
        meas: MeasId(1),
        traces: ac2_proto::model::OwnedTraces::Keep,
    });
    let target = snapshot(&mut b);
    // Three creates, each the measurement then its delay (the first also makes the working
    // state unsaved: `working`); the update (the delay's delete, the meter's `spl_log`
    // entity, then the measurement); the delete (the delay, then the measurement).
    assert_eq!(target.1, Rev(r0.0 + 12));

    // Apply live events, but lose the final one.
    let mut last = r0;
    let mut events = Vec::new();
    while events.len() < 12 {
        if let Some(DataMessage::Event(e)) = sub.next(T) {
            events.push(e);
        }
    }
    for e in &events[..11] {
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
    let evs = a.events(Command::StateSince { rev: last });
    assert_eq!(evs.len(), 1);
    for e in &evs {
        apply(&mut mirror, e);
        last = e.rev;
    }
    assert_eq!(last, target.1);
    assert_eq!(mirror, target.0, "mirror equals the daemon state");

    // Expired replay: more than 8 commits later, since(old) must resync.
    for _ in 0..10 {
        b.ok(Command::MeasStop { meas: MeasId(3) });
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
fn request_dedup() {
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
    // The create, its delay, then the working state unsaved.
    assert_eq!(rev, Rev(3));

    // A refusal is an answer like any other: retried after the state it was refused on
    // has gone, the id returns the stored refusal and does not execute.
    let m = s.measurements[0].clone();
    let stale = Request::new(
        c.next_id(),
        Command::MeasUpdate {
            meas: m.id,
            base: Rev(m.edit_rev.0 - 1),
            config: transfer("renamed"),
        },
    );
    c.send(&stale);
    let conflict = c.recv_raw(T).unwrap();
    c.send(&stale);
    assert_eq!(c.recv_raw(T).unwrap(), conflict);
    assert_eq!(snapshot(&mut c).0.measurements[0].config.name, "once");
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

/// Keepalives carry the daemon's host and, with a stream open, its callback load and the
/// xruns the backend reported; nothing of the audio without a stream.
#[test]
fn keepalives_carry_the_host_the_callback_load_and_the_xruns() {
    use ac2_audio::fake::{FakeConfig, FakeFault};
    init_log();
    let health = |sub: &Sub| match sub.ka(T).unwrap().data {
        FrameData::Ka(k) => k.health,
        _ => unreachable!(),
    };
    let backend = ac2_audio::FakeBackend::new(FakeConfig {
        faults: vec![FakeFault::XrunFlag { at_block: 2 }],
        ..manual_rig().config().clone()
    })
    .unwrap();
    let h = Daemon::start(config(backend.clone(), inproc("health"))).unwrap();
    let (mut c, sub) = connect(&h, &[b"ka"]);
    let first = health(&sub);
    assert!(!first.host.is_empty());
    assert_eq!(first.audio, None, "no stream");

    c.ok(Command::StreamOpen {
        config: stream(false),
    });
    let mut d = driver(&backend);
    d.run_blocks(50);
    // The first window ends with the first keepalive after the open; the next one a second
    // later holds the blocks just run.
    let deadline = std::time::Instant::now() + 3 * T;
    // A keepalive sent before the open was handled can still be queued: it has no audio
    // figures yet.
    let audio = loop {
        let a = health(&sub).audio;
        if let Some(a) = a.filter(|a| a.load.is_some() && a.xruns == 1) {
            break a;
        }
        assert!(std::time::Instant::now() < deadline, "{a:?}");
    };
    let load = audio.load.unwrap();
    assert!((0.0..1.0).contains(&load), "{load}");
    assert!(audio.peak.unwrap() >= load && audio.stream_load.is_some());

    c.ok(Command::StreamClose);
    let deadline = std::time::Instant::now() + T;
    while health(&sub).audio.is_some() {
        assert!(
            std::time::Instant::now() < deadline,
            "audio figures after the close"
        );
    }
    h.shutdown();
}

fn welcome(h: &ac2d::Handle) -> ac2_proto::Welcome {
    let (mut c, _s) = connect(h, &[]);
    c.welcome(Command::Hello {
        client: "test".into(),
    })
}

/// The hello tells the rig's name and id; an id kept in its file is the same after a
/// restart, one set aside as damaged is replaced and reported, and an ephemeral one is new
/// every start.
#[test]
fn the_rig_id_outlives_a_restart_and_the_name_is_told() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("rig-id");
    let start = |name: &str| {
        let mut cfg = config(manual_rig(), inproc(&unique(name)));
        cfg.name = "FOH rack".into();
        cfg.rig_id = ac2d::RigIdSource::File(file.clone());
        Daemon::start(cfg).unwrap()
    };
    let h = start("rig-id-a");
    let first = welcome(&h);
    assert_eq!(first.name, "FOH rack");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        format!("{}\n", first.rig_id)
    );
    h.shutdown();
    let h = start("rig-id-b");
    assert_eq!(welcome(&h).rig_id, first.rig_id);
    h.shutdown();

    std::fs::write(&file, b"torn").unwrap();
    let h = start("rig-id-c");
    let fresh = welcome(&h).rig_id;
    assert_ne!(fresh, first.rig_id);
    let (st, _, _) = snapshot(&mut connect(&h, &[]).0);
    let d = &st.rig.damaged_files;
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].kind, ac2_proto::model::StateFile::RigId);
    assert!(d[0].set_aside_as.is_some());
    h.shutdown();

    let a = Daemon::start(config(manual_rig(), inproc(&unique("rig-id-e1")))).unwrap();
    let b = Daemon::start(config(manual_rig(), inproc(&unique("rig-id-e2")))).unwrap();
    assert_ne!(welcome(&a).rig_id, welcome(&b).rig_id);
    a.shutdown();
    b.shutdown();
}

/// A measurement's name follows the rule of every name an operator gives: something to read,
/// at most as long as a trace's, whether it is created or renamed.
#[test]
fn a_measurement_needs_a_readable_bounded_name() {
    init_log();
    let h = Daemon::start(config(manual_rig(), inproc("meas-names"))).unwrap();
    let (mut a, _s) = connect(&h, &[]);
    let longest = "m".repeat(ac2_traces::meta::MAX_NAME);
    let too_long = "m".repeat(ac2_traces::meta::MAX_NAME + 1);
    for bad in ["", "   ", too_long.as_str()] {
        let e = a
            .call(Command::MeasCreate {
                config: spl(bad, 1),
            })
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::Invalid, "{bad:?}: {e:?}");
    }
    let m = a.measurement(Command::MeasCreate {
        config: spl(&longest, 1),
    });
    for bad in ["", too_long.as_str()] {
        let e = a
            .call(Command::MeasUpdate {
                meas: m.id,
                base: m.edit_rev,
                config: spl(bad, 1),
            })
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::Invalid, "{bad:?}: {e:?}");
    }
    h.shutdown();
}
