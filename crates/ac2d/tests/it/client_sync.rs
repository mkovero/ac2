//! ac2-client's mirror against the real daemon: the snapshot at connect and live events
//! after it, `expect_rev`, the welcome naming the rig, and a daemon restart read as silence
//! and then as a new incarnation with a new identity. The sync rules for what a working
//! daemon never does on demand (a lost event, an expired replay, a lagging keepalive) are
//! the unit tests of `ac2_client::mirror`.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::{Duration, Instant};

use ac2_client::Client;
use ac2_proto::model::OwnedTraces;
use ac2_proto::units::{DaemonIncarnation, MeasId};
use ac2_proto::{Command, ErrorCode};
use ac2d::{Daemon, Listen, RigIdSource};
use common::*;

/// A ceiling, not an expectation: waits end on their condition, normally within a second.
const T: Duration = Duration::from_secs(30);

fn names(c: &Client) -> Vec<String> {
    c.view()
        .state
        .as_ref()
        .map(|s| {
            s.measurements
                .iter()
                .map(|m| m.config.name.clone())
                .collect()
        })
        .unwrap_or_default()
}

async fn stop(h: ac2d::Handle) {
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hello_snapshot_and_live_events() {
    init_log();
    let h = Daemon::start(config(manual_rig(), local_tcp())).unwrap();
    let other = mirror(&h, "other").await;
    other
        .call(Command::MeasCreate {
            config: spl("pre", 0),
        })
        .await
        .unwrap();
    let c = mirror(&h, "ac2-client-test").await;
    assert!(c.client_id().0.starts_with("local-"), "{:?}", c.client_id());
    assert_eq!(names(&c), ["pre"]);
    other
        .call(Command::MeasCreate {
            config: spl("live", 1),
        })
        .await
        .unwrap();
    until("live event", T, || names(&c) == ["pre", "live"]).await;
    let v = c.view();
    assert_eq!(v.snapshots, 1);
    assert_eq!(v.since_requests, 0);
    assert!(v.responding(Instant::now()));
    drop((c, other));
    stop(h).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expect_rev_conflict() {
    init_log();
    let h = Daemon::start(config(manual_rig(), local_tcp())).unwrap();
    let c = mirror(&h, "ac2-client-test").await;
    c.call(Command::MeasCreate {
        config: spl("x", 0),
    })
    .await
    .unwrap();
    let rev = c.snapshot().await.unwrap().rev;
    let delete = || Command::MeasDelete {
        meas: MeasId(1),
        traces: OwnedTraces::Keep,
    };
    let e = c
        .call_expect(delete(), ac2_proto::units::Rev(rev.0 - 1))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some(ErrorCode::Conflict), "{e}");
    c.call_expect(delete(), rev).await.unwrap();
    drop(c);
    stop(h).await;
}

/// The daemon going away reads as not responding once its keepalives stop; the one started
/// on the same endpoints is a new incarnation: the client says hello again, takes the new
/// (empty) state and the new identity the daemon binds, and the rig keeps its id and name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_reads_not_responding_then_a_new_incarnation() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let id_file = dir.path().join("rig-id");
    let start = |listen: Listen| {
        let mut cfg = config(manual_rig(), listen);
        cfg.name = "FOH rack".into();
        cfg.rig_id = RigIdSource::File(id_file.clone());
        Daemon::start(cfg).unwrap()
    };
    let h1 = start(local(dir.path()));
    let same = Listen::Local {
        ctrl: h1.ctrl_endpoint().to_owned(),
        data: h1.data_endpoint().to_owned(),
    };
    let c = mirror(&h1, "ac2-client-test").await;
    c.call(Command::MeasCreate {
        config: spl("old", 0),
    })
    .await
    .unwrap();
    until("old mirrored", T, || names(&c) == ["old"]).await;
    let first = c.view().incarnation;
    assert_eq!(first, Some(DaemonIncarnation(h1.incarnation())));
    let first_id = c.client_id();
    assert_eq!(c.view().client_id.as_ref(), Some(&first_id));
    let w = c.welcome();
    assert_eq!(w.name, "FOH rack");
    let rig = w.rig_id;

    stop(h1).await;
    let gone = Instant::now();
    until("not responding", T, || !c.view().responding(Instant::now())).await;
    assert!(gone.elapsed() >= Duration::from_millis(1300), "{gone:?}");
    assert!(!c.latest().unwrap().responding);

    let h2 = start(same);
    let inc2 = Some(DaemonIncarnation(h2.incarnation()));
    until("new incarnation", T, || {
        let v = c.view();
        v.incarnation == inc2 && v.synced()
    })
    .await;
    assert_ne!(first, inc2);
    assert!(names(&c).is_empty(), "{:?}", names(&c));
    assert_eq!(c.view().incarnation_changes, 1);
    until("welcome refreshed", T, || {
        Some(c.welcome().daemon_incarnation) == inc2
    })
    .await;
    // A synced view of the new incarnation carries the identity it bound, so "owned by me"
    // never compares against the old one.
    let v = c.view();
    assert_ne!(c.client_id(), first_id);
    assert_eq!(v.client_id.as_ref(), Some(&c.client_id()));
    assert!(v.responding(Instant::now()));
    let w = c.welcome();
    assert_eq!((w.rig_id, w.name.as_str()), (rig, "FOH rack"));
    drop(c);
    stop(h2).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_daemons_are_two_rigs_by_id_and_name() {
    init_log();
    let start = |name: &str| {
        let mut cfg = config(manual_rig(), local_tcp());
        cfg.name = name.into();
        Daemon::start(cfg).unwrap()
    };
    let (a, b) = (start("FOH rack"), start("Delay tower"));
    let (ca, cb) = (mirror(&a, "a").await, mirror(&b, "b").await);
    let (wa, wb) = (ca.welcome(), cb.welcome());
    assert_eq!(wa.name, "FOH rack");
    assert_eq!(wb.name, "Delay tower");
    assert_ne!(wa.rig_id, wb.rig_id);
    drop((ca, cb));
    stop(a).await;
    stop(b).await;
}
