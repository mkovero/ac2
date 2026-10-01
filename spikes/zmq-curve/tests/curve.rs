//! CURVE + ZAP on both daemon sockets: authorized clients work, everything else is refused on
//! ctrl AND data, and a refused client never reaches the application nor receives a byte.

mod common;

use std::sync::Arc;

use common::*;
use spike_zmq_curve::ctrl::{ClientSecurity, CtrlServer, Gates, ServerSecurity, unique};
use spike_zmq_curve::data::{SubEvent, bind_xpub};
use spike_zmq_curve::ffi::{
    ZMQ_EVENT_HANDSHAKE_FAILED_AUTH, ZMQ_EVENT_HANDSHAKE_FAILED_NO_DETAIL,
    ZMQ_EVENT_HANDSHAKE_FAILED_PROTOCOL, ZMQ_EVENT_HANDSHAKE_SUCCEEDED,
};
use spike_zmq_curve::proto::{Cmd, decode_frame, encode_frame};
use spike_zmq_curve::zap::{AuthorizedClients, ZapHandler};
use spike_zmq_curve::zmq::{Context, CurveKeyPair, Monitor, Socket, z85_decode_key};

const DOMAIN: &str = "ac2";
const ANY_HANDSHAKE_FAILURE: u16 = ZMQ_EVENT_HANDSHAKE_FAILED_AUTH
    | ZMQ_EVENT_HANDSHAKE_FAILED_PROTOCOL
    | ZMQ_EVENT_HANDSHAKE_FAILED_NO_DETAIL;

struct Daemon {
    ctx: Context,
    server_keys: CurveKeyPair,
    zap: ZapHandler,
    ctrl: CtrlServer,
    xpub: Socket,
    xpub_ep: String,
    xpub_mon: Monitor,
}

fn daemon(authorized: &[(&CurveKeyPair, &str)]) -> Result<Daemon, Box<dyn std::error::Error>> {
    let ctx = Context::new()?;
    let server_keys = CurveKeyPair::generate()?;
    let mut list = AuthorizedClients::new();
    for (k, name) in authorized {
        list.insert(z85_decode_key(&k.public)?, (*name).to_owned());
    }
    // ZAP must be bound before the CURVE sockets accept anything.
    let zap = ZapHandler::start(&ctx, DOMAIN, list)?;
    let sec = ServerSecurity::Curve {
        keys: server_keys.clone(),
        zap_domain: DOMAIN.into(),
    };
    let ctrl = CtrlServer::start(&ctx, "tcp://127.0.0.1:*", &sec, Arc::new(Gates::default()))?;
    let xpub = bind_xpub(&ctx, "tcp://127.0.0.1:*", &sec, 1000)?;
    let xpub_ep = xpub.last_endpoint()?;
    let xpub_mon = Monitor::attach(&ctx, &xpub, &unique("xpub"))?;
    Ok(Daemon {
        ctx,
        server_keys,
        zap,
        ctrl,
        xpub,
        xpub_ep,
        xpub_mon,
    })
}

/// A client socket with a monitor attached before it connects, so no event is missed.
fn client_monitor(
    ctx: &Context,
    s: &Socket,
    ep: &str,
) -> Result<Monitor, Box<dyn std::error::Error>> {
    let m = Monitor::attach(ctx, s, &unique("client"))?;
    s.connect(ep)?;
    Ok(m)
}

fn curve_client(keys: &CurveKeyPair, server_public: &str) -> ClientSecurity {
    ClientSecurity::Curve {
        keys: keys.clone(),
        server_public: server_public.to_owned(),
    }
}

#[test]
fn authorized_client_works_on_ctrl_and_data() -> TestResult {
    let alice = CurveKeyPair::generate()?;
    let d = daemon(&[(&alice, "alice")])?;
    let sec = curve_client(&alice, &d.server_keys.public);

    let ctrl = dealer(&d.ctx, d.ctrl.endpoint(), &sec)?;
    send_req(&ctrl, 1, Cmd::Whoami)?;
    let r = recv_reply(&ctrl)?;
    // ZAP's user id travels with every message of the connection: the daemon can bind leases
    // and audit entries to an authenticated identity, not to a self-declared name.
    assert_eq!((r.id, r.result), (1, Ok("alice".to_owned())));

    let s = sub(&d.ctx, &d.xpub_ep, &sec, &["d/"])?;
    wait_sub_event(&d.xpub, SubEvent::Subscribe, "d/")?;
    d.xpub
        .send_multipart(&encode_frame("d/m1/tf", &header(1, TF_N), &tf_values(1)), 0)?;
    let m = s
        .recv_timeout(TIMEOUT)?
        .ok_or("authorized subscriber got nothing")?;
    assert_eq!(decode_frame(&m.frames)?.header.seq, 1);
    assert!(
        d.xpub_mon
            .wait_for(ZMQ_EVENT_HANDSHAKE_SUCCEEDED, TIMEOUT)?
            .is_some(),
        "no CURVE handshake event on XPUB"
    );
    Ok(())
}

#[test]
fn unauthorized_client_refused_on_ctrl_and_data() -> TestResult {
    let alice = CurveKeyPair::generate()?;
    let mallory = CurveKeyPair::generate()?;
    let d = daemon(&[(&alice, "alice")])?;
    let good = curve_client(&alice, &d.server_keys.public);
    // Mallory has a well-formed keypair and the right server key; only ZAP stands in the way.
    let bad = curve_client(&mallory, &d.server_keys.public);

    // Unauthorized ctrl: queue a request, then watch the handshake get refused.
    let bad_ctrl = d.ctx.socket(spike_zmq_curve::zmq::SocketType::Dealer)?;
    spike_zmq_curve::ctrl::apply_client(&bad_ctrl, &bad)?;
    let bad_ctrl_mon = client_monitor(&d.ctx, &bad_ctrl, d.ctrl.endpoint())?;
    send_req(
        &bad_ctrl,
        66,
        Cmd::Echo {
            text: "let me in".into(),
        },
    )?;
    let ev = bad_ctrl_mon
        .wait_for(ANY_HANDSHAKE_FAILURE, TIMEOUT)?
        .ok_or("ctrl not refused")?;
    // 400 is the ZAP status code the server returned in its ERROR command.
    assert_eq!((ev.event, ev.value), (ZMQ_EVENT_HANDSHAKE_FAILED_AUTH, 400));
    let srv = d
        .ctrl
        .monitor()
        .wait_for(ANY_HANDSHAKE_FAILURE, TIMEOUT)?
        .ok_or("no ctrl event")?;
    assert_eq!(srv.event, ZMQ_EVENT_HANDSHAKE_FAILED_AUTH);

    // Unauthorized data.
    let bad_sub = d.ctx.socket(spike_zmq_curve::zmq::SocketType::Sub)?;
    spike_zmq_curve::ctrl::apply_client(&bad_sub, &bad)?;
    bad_sub.subscribe(b"")?;
    let bad_sub_mon = client_monitor(&d.ctx, &bad_sub, &d.xpub_ep)?;
    let ev = bad_sub_mon
        .wait_for(ANY_HANDSHAKE_FAILURE, TIMEOUT)?
        .ok_or("data not refused")?;
    assert_eq!((ev.event, ev.value), (ZMQ_EVENT_HANDSHAKE_FAILED_AUTH, 400));
    let srv = d
        .xpub_mon
        .wait_for(ANY_HANDSHAKE_FAILURE, TIMEOUT)?
        .ok_or("no server event")?;
    assert_eq!(srv.event, ZMQ_EVENT_HANDSHAKE_FAILED_AUTH);

    // An authorized client on the same daemon still works, and serves as the barrier: once it
    // has everything, anything meant for Mallory would have arrived too.
    let good_ctrl = dealer(&d.ctx, d.ctrl.endpoint(), &good)?;
    send_req(&good_ctrl, 1, Cmd::Whoami)?;
    assert_eq!(recv_reply(&good_ctrl)?.result, Ok("alice".to_owned()));
    let good_sub = sub(&d.ctx, &d.xpub_ep, &good, &[""])?;
    let subs = wait_sub_event(&d.xpub, SubEvent::Subscribe, "")?;
    // The only subscription the daemon ever saw is Alice's: Mallory's never got past ZAP.
    assert_eq!(subs, vec![SubEvent::Subscribe(Vec::new())]);
    for seq in 0..20 {
        d.xpub
            .send_multipart(&encode_frame("d/m1/tf", &header(seq, 4), &[0.0; 4]), 0)?;
    }
    for seq in 0..20 {
        let m = good_sub
            .recv_timeout(TIMEOUT)?
            .ok_or("authorized subscriber starved")?;
        assert_eq!(decode_frame(&m.frames)?.header.seq, seq);
    }

    assert!(
        bad_sub.try_recv()?.is_none(),
        "unauthorized subscriber received data"
    );
    assert!(
        bad_ctrl.try_recv()?.is_none(),
        "unauthorized ctrl client received a reply"
    );
    let seen = d.ctrl.seen();
    assert!(
        seen.iter()
            .all(|(user, _)| user.as_deref() == Some("alice")),
        "daemon saw a request from an unauthenticated peer: {seen:?}"
    );
    assert!(!seen.iter().any(|(_, r)| r.id == 66));

    let mallory_z85 = mallory.public.clone();
    let decisions = d.zap.decisions();
    assert!(
        decisions
            .iter()
            .any(|z| z.client_key == mallory_z85 && !z.allowed)
    );
    assert!(
        decisions
            .iter()
            .all(|z| z.allowed == (z.client_key == alice.public))
    );
    Ok(())
}

#[test]
fn plaintext_and_wrong_server_key_refused() -> TestResult {
    let alice = CurveKeyPair::generate()?;
    let d = daemon(&[(&alice, "alice")])?;

    // NULL mechanism against a CURVE server: mechanism mismatch, refused before ZAP.
    let plain = d.ctx.socket(spike_zmq_curve::zmq::SocketType::Sub)?;
    plain.subscribe(b"")?;
    let plain_mon = client_monitor(&d.ctx, &plain, &d.xpub_ep)?;
    assert!(
        plain_mon
            .wait_for(ANY_HANDSHAKE_FAILURE, TIMEOUT)?
            .is_some()
    );

    // Authorized client key but pinned to a different server key (MITM / wrong daemon): the
    // server cannot open the client's HELLO box, so the handshake never completes.
    let imposter = CurveKeyPair::generate()?;
    let pinned_wrong = curve_client(&alice, &imposter.public);
    let wrong = d.ctx.socket(spike_zmq_curve::zmq::SocketType::Dealer)?;
    spike_zmq_curve::ctrl::apply_client(&wrong, &pinned_wrong)?;
    wrong.connect(d.ctrl.endpoint())?;
    send_req(&wrong, 7, Cmd::Whoami)?;
    let wrong_sub = d.ctx.socket(spike_zmq_curve::zmq::SocketType::Sub)?;
    spike_zmq_curve::ctrl::apply_client(&wrong_sub, &pinned_wrong)?;
    wrong_sub.subscribe(b"")?;
    wrong_sub.connect(&d.xpub_ep)?;
    // Server side: two refused peers on data (plaintext, wrong key), one on ctrl (wrong key).
    let mut data_failures = Vec::new();
    for _ in 0..2 {
        let ev = d.xpub_mon.wait_for(ANY_HANDSHAKE_FAILURE, TIMEOUT)?;
        data_failures.push(ev.ok_or("XPUB did not reject a peer")?.event);
    }
    assert!(
        data_failures
            .iter()
            .all(|e| *e == ZMQ_EVENT_HANDSHAKE_FAILED_PROTOCOL)
    );
    let ev = d.ctrl.monitor().wait_for(ANY_HANDSHAKE_FAILURE, TIMEOUT)?;
    assert_eq!(
        ev.ok_or("ROUTER did not reject wrong key")?.event,
        ZMQ_EVENT_HANDSHAKE_FAILED_PROTOCOL
    );

    // Barrier: an authorized client completes a round trip on both sockets.
    let good = curve_client(&alice, &d.server_keys.public);
    let good_ctrl = dealer(&d.ctx, d.ctrl.endpoint(), &good)?;
    send_req(&good_ctrl, 1, Cmd::Whoami)?;
    assert_eq!(recv_reply(&good_ctrl)?.id, 1);
    let good_sub = sub(&d.ctx, &d.xpub_ep, &good, &[""])?;
    let subs = wait_sub_event(&d.xpub, SubEvent::Subscribe, "")?;
    assert_eq!(
        subs.len(),
        1,
        "XPUB saw subscriptions from refused peers: {subs:?}"
    );
    d.xpub
        .send_multipart(&encode_frame("d/x", &header(0, 1), &[1.0]), 0)?;
    assert!(good_sub.recv_timeout(TIMEOUT)?.is_some());

    assert!(plain.try_recv()?.is_none());
    assert!(wrong.try_recv()?.is_none());
    assert!(wrong_sub.try_recv()?.is_none());
    assert!(!d.ctrl.seen().iter().any(|(_, r)| r.id == 7));
    Ok(())
}

/// Gotcha: libzmq only consults ZAP if a handler is bound. A CURVE server whose context has
/// no handler accepts ANY client that knows the server public key (ZMQ_ZAP_ENFORCE_DOMAIN,
/// which would refuse instead, is draft API). ac2d must start the ZAP handler first.
#[test]
fn without_zap_handler_any_curve_client_is_accepted() -> TestResult {
    let ctx = Context::new()?;
    let server_keys = CurveKeyPair::generate()?;
    let sec = ServerSecurity::Curve {
        keys: server_keys.clone(),
        zap_domain: DOMAIN.into(),
    };
    let xpub = bind_xpub(&ctx, "tcp://127.0.0.1:*", &sec, 100)?;
    let stranger = curve_client(&CurveKeyPair::generate()?, &server_keys.public);
    let s = sub(&ctx, &xpub.last_endpoint()?, &stranger, &[""])?;
    wait_sub_event(&xpub, SubEvent::Subscribe, "")?;
    xpub.send_multipart(&encode_frame("d/x", &header(0, 1), &[1.0]), 0)?;
    assert!(
        s.recv_timeout(TIMEOUT)?.is_some(),
        "stranger was refused without ZAP"
    );
    Ok(())
}
