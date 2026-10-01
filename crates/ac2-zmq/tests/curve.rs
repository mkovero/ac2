//! CURVE + ZAP on both daemon sockets (ROUTER ctrl, XPUB data): authorized clients work and
//! carry their ZAP identity; unauthorized, plaintext and wrong-server-key clients are refused
//! on both and never reach the application or receive a byte.

mod common;

use std::sync::{Arc, Mutex};

use ac2_zmq::zap::{DenyReason, Verdict};
use ac2_zmq::{
    AuthorizedKeys, Context, CurveClient, Error, KeyPair, Monitor, MonitorEvent, SecureContext,
    Socket, SocketType, ZapDecision,
};
use common::*;

const DOMAIN: &str = "ac2";

struct Daemon {
    secure: SecureContext,
    server_keys: KeyPair,
    decisions: Arc<Mutex<Vec<ZapDecision>>>,
    ctrl: CtrlServer,
    ctrl_mon: Monitor,
    xpub: Socket,
    xpub_ep: String,
    xpub_mon: Monitor,
}

impl Daemon {
    fn start(authorized: &[(&KeyPair, &str)]) -> R<Self> {
        let mut keys = AuthorizedKeys::new();
        for (k, name) in authorized {
            keys.insert(name, k.public)?;
        }
        let decisions = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&decisions);
        let secure = SecureContext::new(DOMAIN, keys, move |d| {
            if let Ok(mut l) = log.lock() {
                l.push(d.clone());
            }
        })?;
        let server_keys = KeyPair::generate()?;

        let router = secure.curve_server_socket(SocketType::Router, &server_keys)?;
        // Monitors attach before bind so no connection event is missed.
        let ctrl_mon = router.monitor()?;
        let ctrl = CtrlServer::start(secure.context(), router, "tcp://127.0.0.1:*")?;

        let xpub = secure.curve_server_socket(SocketType::XPub, &server_keys)?;
        xpub.set_xpub_verbose(true)?;
        let xpub_mon = xpub.monitor()?;
        xpub.bind("tcp://127.0.0.1:*")?;
        let xpub_ep = xpub.last_endpoint()?;
        Ok(Self {
            secure,
            server_keys,
            decisions,
            ctrl,
            ctrl_mon,
            xpub,
            xpub_ep,
            xpub_mon,
        })
    }

    fn client(&self, keys: &KeyPair) -> CurveClient {
        CurveClient {
            keys: keys.clone(),
            server_key: self.server_keys.public,
        }
    }

    fn decisions(&self) -> Vec<ZapDecision> {
        self.decisions.lock().map(|d| d.clone()).unwrap_or_default()
    }
}

/// A client socket whose monitor is attached before it connects. Refused clients retry
/// rarely so each one produces one refusal per test.
fn monitored(
    ctx: &Context,
    kind: SocketType,
    curve: Option<&CurveClient>,
    ep: &str,
) -> R<(Socket, Monitor)> {
    let s = ctx.socket(kind)?;
    s.set_reconnect_interval(TIMEOUT)?;
    if let Some(c) = curve {
        s.set_curve_client(c)?;
    }
    if kind == SocketType::Sub {
        s.subscribe(b"")?;
    }
    let m = s.monitor()?;
    s.connect(ep)?;
    Ok((s, m))
}

fn wait_failure(m: &Monitor) -> R<MonitorEvent> {
    Ok(m.wait_for(TIMEOUT, MonitorEvent::is_handshake_failure)?
        .ok_or("no handshake failure")?
        .event)
}

#[test]
fn authorized_client_works_on_ctrl_and_data_with_zap_identity() -> TestResult {
    let alice = KeyPair::generate()?;
    let d = Daemon::start(&[(&alice, "alice")])?;
    let ctx = Context::new()?;
    let cfg = d.client(&alice);

    let ctrl = dealer(&ctx, d.ctrl.endpoint(), Some(&cfg))?;
    send_req(&ctrl, 1, b"whoami")?;
    // The ZAP user id travels with every message of the connection: the daemon binds leases
    // and audit entries to an authenticated identity, not to a self-declared name.
    assert_eq!(recv_reply(&ctrl)?, (1, "alice".to_owned()));
    assert_eq!(d.ctrl.seen(), vec![(Some("alice".to_owned()), 1)]);

    let s = sub(&ctx, &d.xpub_ep, Some(&cfg), &["d/"])?;
    let subs = wait_sub_event(&d.xpub, &subscribed("d/"))?;
    assert_eq!(subs.len(), 1);
    d.xpub.send(&frame("d/m1/tf", 1, &[7; 64]))?;
    let m = recv(&s)?;
    assert_eq!(seq_of(&m), Some(1));
    let ev = d
        .xpub_mon
        .wait_for(TIMEOUT, |e| *e == MonitorEvent::HandshakeSucceeded)?;
    assert!(ev.is_some(), "no CURVE handshake event on XPUB");

    let decisions = d.decisions();
    assert_eq!(decisions.len(), 2, "{decisions:?}");
    for z in &decisions {
        assert_eq!(z.domain, DOMAIN);
        assert_eq!(z.address, "127.0.0.1");
        assert_eq!(z.client_key, Some(alice.public));
        assert_eq!(
            z.verdict,
            Verdict::Allowed {
                user_id: "alice".into()
            }
        );
    }
    Ok(())
}

#[test]
fn unauthorized_key_refused_on_ctrl_and_data() -> TestResult {
    let alice = KeyPair::generate()?;
    let mallory = KeyPair::generate()?;
    let d = Daemon::start(&[(&alice, "alice")])?;
    let ctx = Context::new()?;
    // A well-formed keypair that pins the right server key: only ZAP stands in the way.
    let bad = d.client(&mallory);

    let (bad_ctrl, bad_ctrl_mon) =
        monitored(&ctx, SocketType::Dealer, Some(&bad), d.ctrl.endpoint())?;
    send_req(&bad_ctrl, 66, b"echo:let me in")?;
    // 400 is the ZAP status the server put in its ERROR command.
    let refused = MonitorEvent::HandshakeFailedAuth { status: 400 };
    assert_eq!(wait_failure(&bad_ctrl_mon)?, refused);
    assert_eq!(wait_failure(&d.ctrl_mon)?, refused);

    let (bad_sub, bad_sub_mon) = monitored(&ctx, SocketType::Sub, Some(&bad), &d.xpub_ep)?;
    assert_eq!(wait_failure(&bad_sub_mon)?, refused);
    assert_eq!(wait_failure(&d.xpub_mon)?, refused);

    // An authorized client on the same daemon still works and is the barrier: once it has
    // everything, anything meant for the refused client would have arrived too.
    let good = d.client(&alice);
    let good_ctrl = dealer(&ctx, d.ctrl.endpoint(), Some(&good))?;
    send_req(&good_ctrl, 1, b"whoami")?;
    assert_eq!(recv_reply(&good_ctrl)?, (1, "alice".to_owned()));
    let good_sub = sub(&ctx, &d.xpub_ep, Some(&good), &[""])?;
    // The only subscription XPUB ever saw is Alice's: the refused one never passed ZAP.
    assert_eq!(
        wait_sub_event(&d.xpub, &subscribed(""))?,
        vec![subscribed("")]
    );
    for seq in 0..20 {
        d.xpub.send(&frame("d/m1/tf", seq, &[0; 16]))?;
    }
    for seq in 0..20 {
        assert_eq!(seq_of(&recv(&good_sub)?), Some(seq));
    }

    assert_eq!(
        bad_sub.try_recv()?,
        None,
        "unauthorized subscriber got data"
    );
    assert_eq!(
        bad_ctrl.try_recv()?,
        None,
        "unauthorized client got a reply"
    );
    assert_eq!(d.ctrl.seen(), vec![(Some("alice".to_owned()), 1)]);

    let decisions = d.decisions();
    assert!(
        decisions
            .iter()
            .any(|z| z.client_key == Some(mallory.public)
                && z.verdict == Verdict::Denied(DenyReason::UnknownKey))
    );
    assert!(
        decisions
            .iter()
            .all(|z| z.allowed() == (z.client_key == Some(alice.public)))
    );
    Ok(())
}

#[test]
fn plaintext_and_wrong_server_key_refused_on_ctrl_and_data() -> TestResult {
    let alice = KeyPair::generate()?;
    let d = Daemon::start(&[(&alice, "alice")])?;
    let ctx = Context::new()?;
    let protocol_failure =
        |e: &MonitorEvent| matches!(e, MonitorEvent::HandshakeFailedProtocol { .. });

    // NULL mechanism against a CURVE server: mechanism mismatch, refused before ZAP.
    let (plain_ctrl, plain_ctrl_mon) =
        monitored(&ctx, SocketType::Dealer, None, d.ctrl.endpoint())?;
    send_req(&plain_ctrl, 7, b"whoami")?;
    assert!(wait_failure(&plain_ctrl_mon)?.is_handshake_failure());
    let (plain_sub, plain_sub_mon) = monitored(&ctx, SocketType::Sub, None, &d.xpub_ep)?;
    assert!(wait_failure(&plain_sub_mon)?.is_handshake_failure());
    // Server side: a protocol failure, or a no-detail one (EPIPE) when the client noticed the
    // mismatch first and hung up. Either way the peer is gone before any message passes.
    for server in [&d.ctrl_mon, &d.xpub_mon] {
        wait_failure(server)?;
    }

    // Authorized client key pinned to a different server key (wrong daemon, or a MITM): the
    // server cannot open the client's HELLO, so the handshake never completes.
    let pinned_wrong = CurveClient {
        keys: alice.clone(),
        server_key: KeyPair::generate()?.public,
    };
    let (wrong_ctrl, _m1) = monitored(
        &ctx,
        SocketType::Dealer,
        Some(&pinned_wrong),
        d.ctrl.endpoint(),
    )?;
    send_req(&wrong_ctrl, 8, b"whoami")?;
    let (wrong_sub, _m2) = monitored(&ctx, SocketType::Sub, Some(&pinned_wrong), &d.xpub_ep)?;
    for server in [&d.ctrl_mon, &d.xpub_mon] {
        let ev = wait_failure(server)?;
        assert!(protocol_failure(&ev), "{ev:?}");
    }

    // Barrier: an authorized client completes a round trip on both sockets.
    let good = d.client(&alice);
    let good_ctrl = dealer(&ctx, d.ctrl.endpoint(), Some(&good))?;
    send_req(&good_ctrl, 1, b"whoami")?;
    assert_eq!(recv_reply(&good_ctrl)?, (1, "alice".to_owned()));
    let good_sub = sub(&ctx, &d.xpub_ep, Some(&good), &[""])?;
    let subs = wait_sub_event(&d.xpub, &subscribed(""))?;
    assert_eq!(
        subs.len(),
        1,
        "XPUB saw subscriptions from refused peers: {subs:?}"
    );
    d.xpub.send(&frame("d/x", 0, b""))?;
    recv(&good_sub)?;

    for s in [&plain_ctrl, &plain_sub, &wrong_ctrl, &wrong_sub] {
        assert_eq!(s.try_recv()?, None);
    }
    assert_eq!(d.ctrl.seen(), vec![(Some("alice".to_owned()), 1)]);
    // Neither refusal reached the ZAP handler: only Alice's two handshakes did.
    assert!(d.decisions().iter().all(ZapDecision::allowed));
    Ok(())
}

#[test]
fn zap_handler_is_bound_before_new_returns() -> TestResult {
    let secure = SecureContext::new(DOMAIN, AuthorizedKeys::new(), |_| {})?;
    secure.zap_status()?;
    // Nothing else can take the ZAP endpoint in this context: the handler already holds it.
    let rep = secure.context().socket(SocketType::Rep)?;
    assert_eq!(rep.bind("inproc://zeromq.zap.01"), Err(Error::AddressInUse));
    assert!(matches!(
        SecureContext::new("", AuthorizedKeys::new(), |_| {}),
        Err(Error::InvalidArgument(_))
    ));
    Ok(())
}

#[test]
fn curve_server_socket_keeps_zap_running_after_secure_context_is_dropped() -> TestResult {
    let alice = KeyPair::generate()?;
    let mallory = KeyPair::generate()?;
    let mut keys = AuthorizedKeys::new();
    keys.insert("alice", alice.public)?;
    let server_keys = KeyPair::generate()?;
    let secure = SecureContext::new(DOMAIN, keys, |_| {})?;
    let xpub = secure.curve_server_socket(SocketType::XPub, &server_keys)?;
    drop(secure);
    xpub.set_xpub_verbose(true)?;
    let xpub_mon = xpub.monitor()?;
    xpub.bind("tcp://127.0.0.1:*")?;
    let ep = xpub.last_endpoint()?;

    let ctx = Context::new()?;
    let cfg = |k: &KeyPair| CurveClient {
        keys: k.clone(),
        server_key: server_keys.public,
    };
    let (_bad, bad_mon) = monitored(&ctx, SocketType::Sub, Some(&cfg(&mallory)), &ep)?;
    assert_eq!(
        wait_failure(&bad_mon)?,
        MonitorEvent::HandshakeFailedAuth { status: 400 }
    );
    let good = sub(&ctx, &ep, Some(&cfg(&alice)), &[""])?;
    wait_sub_event(&xpub, &subscribed(""))?;
    xpub.send(&frame("d/x", 3, b""))?;
    assert_eq!(seq_of(&recv(&good)?), Some(3));
    assert!(
        xpub_mon
            .wait_for(TIMEOUT, |e| *e == MonitorEvent::HandshakeSucceeded)?
            .is_some()
    );
    Ok(())
}

#[test]
fn revoked_key_is_refused_on_next_handshake() -> TestResult {
    let alice = KeyPair::generate()?;
    let d = Daemon::start(&[(&alice, "alice")])?;
    let ctx = Context::new()?;
    let cfg = d.client(&alice);
    let first = dealer(&ctx, d.ctrl.endpoint(), Some(&cfg))?;
    send_req(&first, 1, b"whoami")?;
    assert_eq!(recv_reply(&first)?, (1, "alice".to_owned()));

    let mut keys = d.secure.authorized();
    assert_eq!(keys.remove("alice"), Some(alice.public));
    d.secure.set_authorized(keys);

    let (second, mon) = monitored(&ctx, SocketType::Dealer, Some(&cfg), d.ctrl.endpoint())?;
    assert_eq!(
        wait_failure(&mon)?,
        MonitorEvent::HandshakeFailedAuth { status: 400 }
    );
    assert_eq!(second.try_recv()?, None);
    // Established connections are not re-checked.
    send_req(&first, 2, b"whoami")?;
    assert_eq!(recv_reply(&first)?, (2, "alice".to_owned()));
    Ok(())
}
