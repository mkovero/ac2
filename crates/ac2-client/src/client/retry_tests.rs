use super::*;
use ac2_proto::units::Rev;
use ac2_proto::{Reply, Request, decode_request, encode_reply};

/// A [`CallCore`] whose ctrl DEALER is connected to a ROUTER handed back, and a context
/// both live on.
pub(super) fn core(retry: Retry) -> (CallCore, Socket, Context) {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let ep = format!(
        "inproc://ac2-client-retry/{}",
        SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let ctx = Context::new().unwrap();
    let router = ctx.socket(SocketType::Router).unwrap();
    router.bind(&ep).unwrap();
    let dealer = ctx.socket(SocketType::Dealer).unwrap();
    let dealer_mon = dealer.monitor().unwrap();
    dealer.connect(&ep).unwrap();
    // Events are not under test: the sync DEALER is connected to nothing.
    let sync = ctx.socket(SocketType::Dealer).unwrap();
    let mon = sync.monitor().unwrap();
    let sync = crate::io::SyncSocket::new(sync, mon, &[]).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let io = Io::start(&ctx, (dealer, dealer_mon), sync, tx, Arc::default()).unwrap();
    let core = CallCore {
        io,
        next_id: AtomicU64::new(7),
        retry,
        key: None,
    };
    (core, router, ctx)
}

/// Requests as the ROUTER received them, each with the peer's routing id, for `wait`.
fn received(router: &Socket, wait: Duration) -> Vec<(Vec<u8>, Request)> {
    let mut out = Vec::new();
    let end = Instant::now() + wait;
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        let Some(m) = router.recv_timeout(left).unwrap() else {
            break;
        };
        let f = m.frames();
        out.push((f[0].clone(), decode_request(&f[f.len() - 1]).unwrap()));
    }
    out
}

/// A local socket in a directory others may enter could be anyone's: no connect,
/// whatever answers there.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_socket_directory_open_to_others_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("run");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
    let ctx = Context::new().unwrap();
    let router = ctx.socket(SocketType::Router).unwrap();
    let eps = crate::Endpoints::local_in(&dir);
    router.bind(&eps.ctrl).unwrap();
    let mut cfg = ClientConfig::new(eps, "socket-dir test");
    cfg.context = Some(ctx.clone());
    let Err(e) = Client::connect(cfg).await else {
        panic!("connected through a directory open to others");
    };
    assert!(matches!(e, ClientError::SocketDir(_)), "{e}");
    assert!(e.to_string().contains("mode is 777"), "{e}");
}

/// A socket directory not made yet (the daemon not started) is a daemon not running, as
/// a missing socket is: no answer, not a socket-directory fault.
#[tokio::test(flavor = "multi_thread")]
async fn a_socket_directory_not_there_yet_is_no_daemon() {
    let t = tempfile::tempdir().unwrap();
    let eps = crate::Endpoints::local_in(&t.path().join("not-yet"));
    let mut cfg = ClientConfig::new(eps, "socket-dir test");
    cfg.retry = Retry {
        timeout: Duration::from_millis(100),
        retries: 1,
    };
    let Err(e) = Client::connect(cfg).await else {
        panic!("connected with no daemon");
    };
    assert!(matches!(e, ClientError::Timeout { .. }), "{e}");
}

/// A socket path longer than the socket address holds is refused by its length before any
/// connect, not left to wait on a connect libzmq can never make.
#[tokio::test(flavor = "multi_thread")]
async fn a_socket_path_too_long_is_refused_by_length() {
    let t = tempfile::tempdir().unwrap();
    let dir = t
        .path()
        .join("d".repeat(ac2_paths::socket_dir::MAX_SOCKET_PATH));
    let mut cfg = ClientConfig::new(crate::Endpoints::local_in(&dir), "socket-path test");
    cfg.retry = Retry {
        timeout: Duration::from_millis(100),
        retries: 1,
    };
    let Err(e) = Client::connect(cfg).await else {
        panic!("connected through a path no socket address holds");
    };
    assert!(matches!(e, ClientError::SocketDir(_)), "{e}");
    assert!(e.to_string().contains("bytes"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_retry_resends_the_same_id_and_takes_the_reply() {
    let retry = Retry {
        timeout: Duration::from_millis(300),
        retries: 3,
    };
    let (core, router, _ctx) = core(retry);
    // The first reply is lost: the ROUTER answers only the resend.
    let daemon = std::thread::spawn(move || {
        let got = received(&router, Duration::from_millis(450));
        let (peer, req) = got.last().cloned().unwrap();
        let reply = Reply::new(req.id, Ok(ReplyBody::Ack { rev: Rev(3) }));
        router.send(&[peer, encode_reply(&reply).unwrap()]).unwrap();
        got.into_iter().map(|(_, r)| r).collect::<Vec<_>>()
    });
    let r = core.call(Command::StreamStatus, retry).await.unwrap();
    assert_eq!(r, ReplyBody::Ack { rev: Rev(3) });
    let got = daemon.join().unwrap();
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0].id, got[1].id);
    assert_eq!(got[0].cmd, got[1].cmd);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_silent_daemon_times_out_after_every_retry() {
    let retry = Retry {
        timeout: Duration::from_millis(100),
        retries: 2,
    };
    let (core, router, _ctx) = core(retry);
    let e = core.call(Command::StreamStatus, retry).await.unwrap_err();
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
    assert_eq!(received(&router, Duration::from_millis(50)).len(), 3);
}

/// Calls sent one after another are on the socket in that order before any reply is
/// awaited, and each reply reaches its own call whichever is awaited first.
#[tokio::test(flavor = "multi_thread")]
async fn calls_are_sent_in_order_before_their_replies_are_awaited() {
    let retry = Retry {
        timeout: Duration::from_secs(5),
        retries: 0,
    };
    let (core, router, _ctx) = core(retry);
    let core = Arc::new(core);
    let sent: Vec<SentCall> = (0..4)
        .map(|_| core.send(Command::StreamStatus, retry).unwrap())
        .collect();
    let mut got = Vec::new();
    while got.len() < sent.len() {
        let m = router
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .expect("sent without being awaited");
        let f = m.frames();
        got.push((f[0].clone(), decode_request(&f[f.len() - 1]).unwrap()));
    }
    let ids: Vec<u64> = got.iter().map(|(_, r)| r.id.0).collect();
    assert_eq!(ids, [7, 8, 9, 10]);
    for (peer, req) in got.iter().rev() {
        let reply = Reply::new(req.id, Ok(ReplyBody::Ack { rev: Rev(req.id.0) }));
        router
            .send(&[peer.clone(), encode_reply(&reply).unwrap()])
            .unwrap();
    }
    for (call, id) in sent.into_iter().rev().zip([10, 9, 8, 7]) {
        let (body, _) = call.reply().await.unwrap();
        assert_eq!(body, ReplyBody::Ack { rev: Rev(id) });
    }
}

/// A snapshot whose bytes all came in and match their hash is the state, though ending its
/// download fails (the daemon dropped the transfer, the reply to the end was lost).
#[tokio::test(flavor = "multi_thread")]
async fn a_whole_snapshot_stands_when_its_download_end_fails() {
    use ac2_proto::model::Transfer;
    use ac2_proto::units::{Blob, TransferId};
    use ac2_proto::{ErrorCode, ProtoError, SnapshotTransfer, StateSnapshot};
    let retry = Retry {
        timeout: Duration::from_millis(500),
        retries: 1,
    };
    let (core, router, _ctx) = core(retry);
    let snap = StateSnapshot {
        state: ac2_proto::samples::state(),
        rev: Rev(41),
        daemon_incarnation: Default::default(),
        stream_epoch: Default::default(),
    };
    let bytes = ac2_proto::encode_snapshot(&snap).unwrap();
    let transfer = Transfer {
        transfer: TransferId(9),
        file_name: SnapshotTransfer::file_name(Rev(41)),
        bytes: bytes.len() as u64,
        sha256: ac2_archive::sha256_hex(&bytes),
        chunk_bytes: 4096,
    };
    let daemon = std::thread::spawn(move || {
        loop {
            let Some(m) = router.recv_timeout(Duration::from_secs(5)).unwrap() else {
                return;
            };
            let f = m.frames();
            let req = decode_request(&f[f.len() - 1]).unwrap();
            let (body, done) = match req.cmd {
                Command::StateSnapshot => (
                    Ok(ReplyBody::SnapshotTransfer(SnapshotTransfer {
                        rev: Rev(41),
                        transfer: transfer.clone(),
                    })),
                    false,
                ),
                Command::FileDownloadChunk { offset, .. } => {
                    let at = offset as usize;
                    let end = (at + 4096).min(bytes.len());
                    (Ok(ReplyBody::Chunk(Blob(bytes[at..end].to_vec()))), false)
                }
                Command::FileDownloadEnd { .. } => (
                    Err(ProtoError {
                        code: ErrorCode::NotFound,
                        msg: "no such transfer".into(),
                        detail: None,
                    }),
                    true,
                ),
                other => panic!("{other:?}"),
            };
            let reply = Reply::new(req.id, body);
            router
                .send(&[f[0].clone(), encode_reply(&reply).unwrap()])
                .unwrap();
            if done {
                return;
            }
        }
    });
    let got = crate::snapshot::take(&core, retry, &mut || {})
        .await
        .unwrap();
    assert_eq!(got.rev, Rev(41));
    assert_eq!(got.state, snap.state);
    daemon.join().unwrap();
}
