use std::sync::atomic::AtomicU32;

use super::patience::Liveness;
use super::retry_tests::core;
use super::*;
use crate::mirror::Mirror;
use ac2_proto::units::{DaemonIncarnation, Rev};
use ac2_proto::{Reply, decode_request, encode_reply};

/// A core whose file operations watch a view the test drives: keepalives and incarnation.
fn watched(
    retry: Retry,
    dead_after: Duration,
) -> (CallCore, Socket, Context, watch::Sender<Arc<MirrorView>>) {
    let (core, router, ctx) = core(retry);
    let mut v = Mirror::new(false).view();
    v.incarnation = Some(DaemonIncarnation(1));
    v.last_ka = Some(Instant::now());
    let (tx, rx) = watch::channel(Arc::new(v));
    core.liveness
        .set(Liveness {
            view: rx,
            dead_after,
        })
        .unwrap();
    (core, router, ctx, tx)
}

/// Keepalives every 100 ms until `stop` is set.
fn keepalives(tx: watch::Sender<Arc<MirrorView>>, stop: Arc<AtomicU32>) {
    std::thread::spawn(move || {
        while stop.load(Ordering::Relaxed) == 0 {
            tx.send_modify(|v| Arc::make_mut(v).last_ka = Some(Instant::now()));
            std::thread::sleep(Duration::from_millis(100));
        }
    });
}

/// Far past the tries a plain call gets: the same id is resent each try and the reply,
/// when the daemon is done, is taken.
#[tokio::test(flavor = "multi_thread")]
async fn a_file_operation_is_waited_for_while_keepalives_come() {
    let retry = Retry {
        timeout: Duration::from_millis(50),
        retries: 2,
    };
    let (core, router, _ctx, tx) = watched(retry, Duration::from_secs(2));
    let stop = Arc::new(AtomicU32::new(0));
    keepalives(tx, stop.clone());
    let daemon = std::thread::spawn(move || {
        let mut got = Vec::new();
        let end = Instant::now() + Duration::from_millis(800);
        while let Some(left) = end.checked_duration_since(Instant::now()) {
            if let Some(m) = router.recv_timeout(left).unwrap() {
                let f = m.frames();
                got.push((f[0].clone(), decode_request(&f[f.len() - 1]).unwrap()));
            }
        }
        let (peer, req) = got.last().cloned().unwrap();
        let reply = Reply::new(req.id, Ok(ReplyBody::Ack { rev: Rev(5) }));
        router.send(&[peer, encode_reply(&reply).unwrap()]).unwrap();
        got.into_iter().map(|(_, r)| r).collect::<Vec<_>>()
    });
    let mut waited = Vec::new();
    let (body, _) = core
        .call_waiting(Command::FileList, retry, &mut |w| waited.push(w))
        .await
        .unwrap();
    stop.store(1, Ordering::Relaxed);
    assert_eq!(body, ReplyBody::Ack { rev: Rev(5) });
    let got = daemon.join().unwrap();
    assert!(got.len() > 3, "resent each try: {}", got.len());
    assert!(got.iter().all(|r| r.id == got[0].id), "one id: {got:?}");
    assert!(waited.len() > 3, "{waited:?}");
    assert!(waited.windows(2).all(|w| w[0] < w[1]), "{waited:?}");
}

/// A daemon that falls silent is reported after the silence limit, however long the
/// per-try deadline.
#[tokio::test(flavor = "multi_thread")]
async fn a_file_operation_ends_when_the_daemon_falls_silent() {
    let retry = Retry {
        timeout: Duration::from_secs(600),
        retries: 2,
    };
    let (core, _router, _ctx, _tx) = watched(retry, Duration::from_millis(400));
    let t0 = Instant::now();
    let e = core
        .call_waiting(Command::FileList, retry, &mut |_| {})
        .await
        .unwrap_err();
    assert!(
        matches!(
            e,
            ClientError::Silent {
                op: "file.list",
                ..
            }
        ),
        "{e}"
    );
    assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
}

/// A restart ends the wait: the work went with the old daemon, and the resent id would
/// run anew on the new one.
#[tokio::test(flavor = "multi_thread")]
async fn a_file_operation_ends_when_the_daemon_restarts() {
    let retry = Retry {
        timeout: Duration::from_millis(50),
        retries: 2,
    };
    let (core, _router, _ctx, tx) = watched(retry, Duration::from_secs(10));
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        tx.send_modify(|v| {
            let v = Arc::make_mut(v);
            v.incarnation = Some(DaemonIncarnation(2));
            v.last_ka = Some(Instant::now());
        });
        // Kept alive past the client's look at it.
        std::thread::sleep(Duration::from_secs(2));
    });
    let e = core
        .call_waiting(Command::FileList, retry, &mut |_| {})
        .await
        .unwrap_err();
    assert!(
        matches!(e, ClientError::Restarted { op: "file.list" }),
        "{e}"
    );
}

/// Anything else keeps its tries: a silent daemon is a timeout after them.
#[tokio::test(flavor = "multi_thread")]
async fn other_commands_keep_their_tries() {
    let retry = Retry {
        timeout: Duration::from_millis(50),
        retries: 2,
    };
    let (core, _router, _ctx, tx) = watched(retry, Duration::from_secs(10));
    let stop = Arc::new(AtomicU32::new(0));
    keepalives(tx, stop.clone());
    let e = core.call(Command::StreamStatus, retry).await.unwrap_err();
    stop.store(1, Ordering::Relaxed);
    assert!(
        matches!(
            e,
            ClientError::Timeout {
                op: "stream.status",
                attempts: 3
            }
        ),
        "{e}"
    );
}
