//! ac2-client's stimulus lease against the real daemon: refreshed in the background while
//! held, stopped and released on drop (even when the client closes at once), lost to a
//! forced takeover without stopping the new owner, and reported lost after the daemon let
//! it expire.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::Duration;

use ac2_audio::FakeBackend;
use ac2_client::{Client, OnDrop};
use ac2_proto::model::{GenAction, Generator, GeneratorDesired, GeneratorSettings, Signal};
use ac2_proto::units::Dbfs;
use ac2_proto::{Command, ErrorCode};
use ac2d::{Daemon, Handle};
use common::*;

const T: Duration = Duration::from_secs(30);

fn desired() -> GeneratorDesired {
    GeneratorDesired {
        settings: GeneratorSettings {
            signal: Signal::Pink,
            level: Dbfs(-30.0),
            band: None,
            outputs: vec![0],
        },
        armed: true,
        firing: true,
    }
}

/// A daemon on the manual fake device with leases expiring after `expiry`, a session open
/// (firing needs one), and a client on it.
async fn rig(expiry: Duration) -> (Handle, FakeBackend, Client) {
    init_log();
    let backend = manual_rig();
    let mut cfg = config(backend.clone(), local_tcp());
    cfg.lease_expiry = expiry;
    let h = Daemon::start(cfg).unwrap();
    let c = mirror(&h, "ac2-client-test").await;
    c.call(Command::SessionOpen {
        config: session(false),
    })
    .await
    .unwrap();
    (h, backend, c)
}

fn generator(c: &Client) -> Generator {
    c.view().state.as_ref().unwrap().generator.clone()
}

async fn stop(h: Handle, backend: FakeBackend) {
    tokio::task::spawn_blocking(move || shutdown_driving(h, backend.take_driver()))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lease_is_refreshed_while_held_and_stopped_on_drop() {
    // Twice the client's 500 ms refresh period: only refreshes keep it past its expiry.
    let expiry = Duration::from_millis(1000);
    let (h, backend, c) = rig(expiry).await;
    let lease = c
        .acquire_lease(false, OnDrop::StopAndRelease)
        .await
        .unwrap();
    assert!(lease.set(desired()).await.unwrap().firing);
    tokio::time::sleep(expiry * 5 / 2).await;
    assert!(lease.lost().is_none());
    let g = generator(&c);
    assert!(g.firing, "{g:?}");
    assert_eq!(g.owner, Some(c.client_id()));
    drop(lease);
    until("stop + release", T, || {
        let g = generator(&c);
        !g.firing && !g.armed && g.owner.is_none()
    })
    .await;
    let audit = generator(&c).last_action.unwrap();
    assert_eq!(audit.action, GenAction::Release, "{audit:?}");
    drop(c);
    stop(h, backend).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn release_queued_on_drop_leaves_even_when_the_client_closes_at_once() {
    // Long enough that only the release can end it within the test.
    let (h, backend, c) = rig(Duration::from_secs(60)).await;
    let watcher = mirror(&h, "watcher").await;
    let lease = c
        .acquire_lease(false, OnDrop::StopAndRelease)
        .await
        .unwrap();
    lease.set(desired()).await.unwrap();
    until("firing", T, || generator(&watcher).firing).await;
    // A foreground CLI ending: lease and client go together, nothing is awaited.
    drop(lease);
    drop(c);
    until("release", T, || generator(&watcher).owner.is_none()).await;
    let g = generator(&watcher);
    assert!(!g.firing && !g.armed, "{g:?}");
    assert_eq!(g.last_action.unwrap().action, GenAction::Release);
    drop(watcher);
    stop(h, backend).await;
}

/// A lease that expires sooner than the client refreshes it: the daemon stops and disarms
/// on its own with an expiry audit naming the holder, as when the network goes away, and
/// the next refresh is refused, so the lease reports itself lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lease_expires_daemon_side_when_refreshes_stop() {
    let (h, backend, c) = rig(Duration::from_millis(250)).await;
    let lease = c.acquire_lease(false, OnDrop::Release).await.unwrap();
    // The set itself may come after the expiry on a loaded machine; then it is refused.
    let _ = lease.set(desired()).await;
    until("expiry", T, || {
        generator(&c)
            .last_action
            .is_some_and(|a| a.action == GenAction::Expiry)
    })
    .await;
    let g = generator(&c);
    assert!(!g.firing && !g.armed && g.owner.is_none(), "{g:?}");
    assert_eq!(g.last_action.unwrap().client, Some(c.client_id()));
    until("lost", T, || lease.lost().is_some()).await;
    drop((lease, c));
    stop(h, backend).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forced_takeover_marks_lease_lost_and_drop_does_not_stop_new_owner() {
    let (h, backend, a) = rig(Duration::from_secs(60)).await;
    let b = mirror(&h, "b").await;
    let la = a
        .acquire_lease(false, OnDrop::StopAndRelease)
        .await
        .unwrap();
    let held = b.acquire_lease(false, OnDrop::Release).await.err();
    assert_eq!(held.and_then(|e| e.code()), Some(ErrorCode::LeaseHeld));
    let lb = b.acquire_lease(true, OnDrop::Release).await.unwrap();
    lb.set(desired()).await.unwrap();
    until("a lost", T, || la.lost().is_some()).await;
    drop(la);
    // A lost lease sends nothing on drop: `gen.stop` needs no lease, so a stop sent here
    // would silence b's output.
    tokio::time::sleep(Duration::from_millis(300)).await;
    until("b's firing mirrored", T, || generator(&b).firing).await;
    let g = generator(&b);
    assert_eq!(g.owner, Some(b.client_id()));
    assert_ne!(g.last_action.unwrap().action, GenAction::Stop);
    lb.end().await.unwrap();
    until("b stopped", T, || !generator(&b).firing).await;
    drop((a, b));
    stop(h, backend).await;
}
