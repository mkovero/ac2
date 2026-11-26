//! The rejoin's `hello` against a ROUTER the test answers by hand: one lost on the way is
//! asked again until a welcome comes, one the daemon refuses is not, and keepalives are
//! applied while it waits. A working daemon neither loses nor stalls a reply on demand.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ac2_proto::frame::{DaemonHealth, GenSummary, KaMeta};
use ac2_proto::model::{ClientRole, TimingState};
use ac2_proto::units::{RequestId, Rev, WallNs};
use ac2_proto::{
    Command, ErrorCode, ProtoError, Reply, ReplyBody, Welcome, decode_request,
    encode_reply,
};
use ac2_zmq::Socket;
use tokio::sync::{mpsc, watch};

use super::run_sync;
use crate::client::Retry;
use crate::client::retry_tests::core;
use crate::io::SyncIn;
use crate::mirror::{Mirror, MirrorView};

fn welcome(role: ClientRole) -> Welcome {
    match ac2_proto::samples::replies().swap_remove(1) {
        Ok(ReplyBody::Welcome(w)) => Welcome { role, ..w },
        r => panic!("{r:?}"),
    }
}

/// A sync task over `retry`, its welcome slot and view, and the ROUTER its requests reach.
#[allow(clippy::type_complexity)]
fn sync(
    retry: Retry,
) -> (
    mpsc::UnboundedSender<SyncIn>,
    Arc<Mutex<Welcome>>,
    watch::Receiver<Arc<MirrorView>>,
    Socket,
    tokio::task::JoinHandle<()>,
    ac2_zmq::Context,
) {
    let (core, router, ctx) = core(retry);
    let slot = Arc::new(Mutex::new(welcome(ClientRole::Operator)));
    let mirror = Mirror::new(false);
    let (view_tx, view_rx) = watch::channel(Arc::new(mirror.view()));
    let (tx, rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(run_sync(
        Arc::new(core),
        rx,
        view_tx,
        mirror,
        "rejoin test".into(),
        slot.clone(),
    ));
    (tx, slot, view_rx, router, task, ctx)
}

/// The `hello`s the ROUTER receives within `wait`, by request id, each answered by
/// `answer(n)` (`n` counting distinct ids from 1; `None`: not answered).
fn daemon(
    router: Socket,
    wait: Duration,
    answer: impl Fn(usize) -> Option<Result<ReplyBody, ProtoError>> + Send + 'static,
) -> std::thread::JoinHandle<Vec<RequestId>> {
    std::thread::spawn(move || {
        let mut ids: Vec<RequestId> = Vec::new();
        let end = Instant::now() + wait;
        while let Some(left) = end.checked_duration_since(Instant::now()) {
            let Some(m) = router.recv_timeout(left).unwrap() else {
                break;
            };
            let f = m.frames();
            let req = decode_request(&f[f.len() - 1]).unwrap();
            assert!(matches!(req.cmd, Command::Hello { .. }), "{req:?}");
            if !ids.contains(&req.id) {
                ids.push(req.id);
            }
            if let Some(body) = answer(ids.len()) {
                router
                    .send(&[
                        f[0].clone(),
                        encode_reply(&Reply::new(req.id, body)).unwrap(),
                    ])
                    .unwrap();
            }
        }
        ids
    })
}

/// A rejoin whose `hello` is lost on the way asks again until a welcome comes: the key's
/// new role is shown, not the one it had.
#[tokio::test(flavor = "multi_thread")]
async fn a_rejoin_asks_for_the_welcome_until_one_comes() {
    let retry = Retry {
        timeout: Duration::from_millis(200),
        retries: 1,
    };
    let (tx, slot, view_rx, router, task, _ctx) = sync(retry);
    let d = daemon(router, Duration::from_secs(3), |n| {
        (n >= 2).then(|| Ok(ReplyBody::Welcome(welcome(ClientRole::Viewer))))
    });
    tx.send(SyncIn::Rejoined).unwrap();
    let end = Instant::now() + Duration::from_secs(3);
    while slot.lock().unwrap().role != ClientRole::Viewer {
        assert!(Instant::now() < end, "the welcome never came again");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(view_rx.borrow().role, Some(ClientRole::Viewer));
    assert_eq!(d.join().unwrap().len(), 2, "answered: not asked again");
    task.abort();
}

/// A `hello` the daemon refuses (a revoked key's, a refusal) is not asked again: the
/// answer would be the same.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_rejoin_is_not_asked_again() {
    let retry = Retry {
        timeout: Duration::from_millis(200),
        retries: 1,
    };
    let (tx, slot, _view_rx, router, task, _ctx) = sync(retry);
    let d = daemon(router, Duration::from_millis(1500), |_| {
        Some(Err(ProtoError {
            code: ErrorCode::Refused,
            msg: "no".into(),
            detail: None,
        }))
    });
    tx.send(SyncIn::Rejoined).unwrap();
    assert_eq!(d.join().unwrap().len(), 1);
    assert_eq!(slot.lock().unwrap().role, ClientRole::Operator);
    task.abort();
}

/// Keepalives are applied while a rejoin's `hello` waits for its reply.
#[tokio::test(flavor = "multi_thread")]
async fn keepalives_are_applied_while_a_rejoin_waits() {
    let retry = Retry {
        timeout: Duration::from_secs(3),
        retries: 1,
    };
    let (tx, _slot, view_rx, router, task, _ctx) = sync(retry);
    let d = daemon(router, Duration::from_secs(1), |_| None);
    tx.send(SyncIn::Rejoined).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let at = Instant::now();
    tx.send(SyncIn::Ka {
        stamp: ac2_proto::samples::stamp(None),
        meta: Box::new(KaMeta {
            rev: Rev(1),
            daemon_wall_ns: WallNs(1_000),
            timing: TimingState::NoStimulus,
            generator: GenSummary {
                owner: None,
                owner_local: false,
                armed: false,
                firing: false,
            },
            health: DaemonHealth {
                host: "rig".into(),
                cpu: None,
                audio: None,
            },
        }),
        at,
        local_wall_ns: 0,
    })
    .unwrap();
    let end = Instant::now() + Duration::from_millis(500);
    while view_rx.borrow().last_ka != Some(at) {
        assert!(Instant::now() < end, "the keepalive waited for the hello");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(d.join().unwrap().len(), 1, "one hello, still waiting");
    task.abort();
}
