use std::time::Duration;

use super::*;
use crate::{CurveClient, MonitorEvent, SubscriptionEvent};

const TIMEOUT: Duration = Duration::from_secs(10);

fn subscriber(ctx: &Context, ep: &str, keys: &KeyPair, server: PublicKey) -> Result<Socket> {
    let s = ctx.socket(SocketType::Sub)?;
    s.set_curve_client(&CurveClient {
        keys: keys.clone(),
        server_key: server,
    })?;
    s.subscribe(b"")?;
    s.connect(ep)?;
    Ok(s)
}

/// Why server-side CURVE is only reachable through `SecureContext`: libzmq skips ZAP when no
/// handler is bound, and the CURVE server then accepts any client that knows its public key.
/// If this ever fails, libzmq changed behaviour and the module docs need a look.
#[test]
fn libzmq_fails_open_without_zap_handler() -> Result<()> {
    let ctx = Context::new()?;
    let server = KeyPair::generate()?;
    let xpub = ctx.socket(SocketType::XPub)?;
    apply_curve_server(&xpub, &server, "ac2")?;
    xpub.bind("tcp://127.0.0.1:*")?;
    let stranger = KeyPair::generate()?;
    let s = subscriber(&ctx, &xpub.last_endpoint()?, &stranger, server.public)?;
    let m = xpub.recv_timeout(TIMEOUT)?;
    assert_eq!(
        m.as_ref().and_then(SubscriptionEvent::parse),
        Some(SubscriptionEvent::Subscribe(Vec::new())),
        "stranger's subscription did not arrive"
    );
    xpub.send(&[b"x"])?;
    assert!(
        s.recv_timeout(TIMEOUT)?.is_some(),
        "stranger was refused without ZAP"
    );
    Ok(())
}

/// An exited handler is reported, blocks new CURVE server sockets, and leaves existing ones
/// failing closed: handshakes stall on the unanswered handler socket and time out.
#[test]
fn handler_exit_is_fatal_and_fails_closed() -> Result<()> {
    let alice = KeyPair::generate()?;
    let mut keys = AuthorizedKeys::new();
    keys.insert("alice", alice.public)
        .map_err(|_| Error::InvalidArgument("insert"))?;
    let server = KeyPair::generate()?;
    let secure = SecureContext::new("ac2", keys, |_| {})?;
    let xpub = secure.curve_server_socket(SocketType::XPub, &server)?;
    xpub.set_handshake_interval(Some(Duration::from_millis(200)))?;
    let mon = xpub.monitor()?;
    xpub.bind("tcp://127.0.0.1:*")?;

    secure.inject_handler_failure()?;
    let fatal = Err(Error::ZapHandlerExited(ZapExit::Injected));
    assert_eq!(secure.zap_status(), fatal);
    assert_eq!(
        secure
            .curve_server_socket(SocketType::Router, &server)
            .map(drop),
        fatal
    );

    // Even an authorized client gets nothing through.
    let client_ctx = Context::new()?;
    let s = subscriber(&client_ctx, &xpub.last_endpoint()?, &alice, server.public)?;
    let ev = mon
        .wait_for(TIMEOUT, MonitorEvent::is_handshake_failure)?
        .map(|e| e.event);
    assert!(
        matches!(ev, Some(MonitorEvent::HandshakeFailedNoDetail { .. })),
        "expected a handshake timeout, got {ev:?}"
    );
    assert_eq!(xpub.try_recv()?, None, "subscription passed a dead handler");
    assert_eq!(s.try_recv()?, None);
    Ok(())
}

fn request(domain: &str, mechanism: &str, credentials: &[&[u8]]) -> Message {
    let head: [&[u8]; 6] = [
        b"1.0",
        b"id",
        domain.as_bytes(),
        b"10.0.0.2",
        b"rid",
        mechanism.as_bytes(),
    ];
    let frames = head.iter().chain(credentials).map(|f| f.to_vec()).collect();
    Message::from_frames(frames)
}

#[test]
fn decisions_follow_rfc27_fields() {
    let alice = PublicKey::from_bytes([7; 32]);
    let mut keys = AuthorizedKeys::new();
    assert!(keys.insert("alice", alice).is_ok());
    let h = Handler {
        domain: "ac2".into(),
        keys: Arc::new(RwLock::new(keys)),
        audit: Box::new(|_| {}),
    };
    let verdict = |m: &Message| h.decide(m).verdict;
    let denied = Verdict::Denied;
    assert_eq!(
        verdict(&request("ac2", "CURVE", &[&[7; 32]])),
        Verdict::Allowed {
            user_id: "alice".into()
        }
    );
    let cases: [(Message, DenyReason); 6] = [
        (request("ac2", "CURVE", &[&[8; 32]]), DenyReason::UnknownKey),
        (
            request("other", "CURVE", &[&[7; 32]]),
            DenyReason::WrongDomain,
        ),
        (request("ac2", "NULL", &[]), DenyReason::NotCurve),
        (
            request("ac2", "CURVE", &[&[7; 31]]),
            DenyReason::MalformedRequest,
        ),
        (
            request("ac2", "CURVE", &[&[7; 32], b"extra"]),
            DenyReason::MalformedRequest,
        ),
        (
            Message::from_frames(vec![b"1.0".to_vec()]),
            DenyReason::MalformedRequest,
        ),
    ];
    for (m, why) in cases {
        assert_eq!(verdict(&m), denied(why), "{m:?}");
    }

    let req = request("ac2", "CURVE", &[&[7; 32]]);
    let d = h.decide(&req);
    assert_eq!(d.address, "10.0.0.2");
    assert_eq!(d.client_key, Some(alice));
    let r = reply(&req, &d);
    assert_eq!(
        (&r[1][..], &r[2][..], &r[4][..]),
        (&b"id"[..], &b"200"[..], &b"alice"[..])
    );
    let r = reply(&req, &h.decide(&request("ac2", "NULL", &[])));
    assert_eq!((&r[2][..], &r[4][..]), (&b"400"[..], &b""[..]));
}
