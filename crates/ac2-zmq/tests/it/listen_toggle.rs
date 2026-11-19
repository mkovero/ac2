//! What libzmq does for a daemon that serves local on plain sockets for its whole life and
//! the network on a CURVE pair made and dropped at runtime: the facts the live network
//! switch relies on (docs/design/network-toggle.md).

use crate::common;

use std::sync::{Arc, Mutex};

use ac2_zmq::{
    AuthorizedKeys, Context, CurveClient, KeyPair, PollItem, SecureContext, Socket, SocketType,
    ZapDecision, poll,
};
use common::*;

/// Sends `[n]` from the dealer and answers it from the router; returns once the reply
/// arrived, so the connection is known to be up end to end.
fn round_trip(router: &Socket, dealer: &Socket, n: u8) -> TestResult {
    dealer.send(&[[n]])?;
    let m = router
        .recv_timeout(TIMEOUT)?
        .ok_or("router got no request")?;
    let f = m.frames();
    router.send(&[f[0].as_slice(), f[1].as_slice()])?;
    let r = dealer.recv_timeout(TIMEOUT)?.ok_or("dealer got no reply")?;
    assert_eq!(r.frames(), &[vec![n]]);
    Ok(())
}

struct Curve {
    secure: SecureContext,
    keys: KeyPair,
    decisions: Arc<Mutex<Vec<ZapDecision>>>,
}

impl Curve {
    fn new() -> R<Self> {
        let decisions = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&decisions);
        let secure = SecureContext::new("ac2", AuthorizedKeys::new(), move |d| {
            if let Ok(mut l) = log.lock() {
                l.push(d.clone());
            }
        })?;
        Ok(Self {
            secure,
            keys: KeyPair::generate()?,
            decisions,
        })
    }

    /// Connects an unknown key to `ep` and returns the ZAP decision libzmq asked for.
    fn refused_address(&self, ep: &str) -> R<String> {
        let before = self.decisions.lock().map(|d| d.len()).unwrap_or(0);
        let ctx = Context::new()?;
        let s = ctx.socket(SocketType::Dealer)?;
        s.set_reconnect_interval(TIMEOUT)?;
        s.set_curve_client(&CurveClient {
            keys: KeyPair::generate()?,
            server_key: self.keys.public,
        })?;
        s.connect(ep)?;
        wait_until(|| self.decisions.lock().map(|d| d.len()).unwrap_or(0) > before)?;
        let d = self.decisions.lock().map_err(|_| "poisoned")?;
        let z = d.last().ok_or("no decision")?;
        assert!(!z.allowed());
        Ok(z.address.clone())
    }
}

/// What the ZAP request's address frame carries per transport. libzmq fills it from the
/// accepted socket, never from the peer's bytes: on ipc "localhost" (Linux) or empty
/// (macOS), "127.0.0.1" on loopback tcp. Neither names a user: loopback is every local user and every tunnel or
/// port forward ending on this host, so "127.0.0.1" is no proof of a local operator.
#[test]
fn zap_address_identifies_the_transport() -> TestResult {
    let c = Curve::new()?;
    let router = c.secure.curve_server_socket(SocketType::Router, &c.keys)?;
    router.bind("tcp://127.0.0.1:*")?;
    let tcp = router.last_endpoint()?;
    assert_eq!(c.refused_address(&tcp)?, "127.0.0.1");

    #[cfg(unix)]
    {
        let dir = tempfile::tempdir()?;
        let ipc = format!("ipc://{}", dir.path().join("ctrl").display());
        router.bind(&ipc)?;
        // The libzmq built here reads no peer credentials, so ipc names no user, only the
        // transport, and not the same way on every OS; the socket file's permissions are the
        // whole boundary.
        let a = c.refused_address(&ipc)?;
        assert!(a == "localhost" || a.is_empty(), "{a:?}");
    }
    Ok(())
}

/// Off as "close the network sockets and drop their secure context", on as "a new secure
/// context, new sockets, bind the same port": the port is free again as soon as the old
/// context is gone (its term waits for the sockets' listeners to close), with a client's
/// connection just closed and in TIME_WAIT.
#[test]
fn new_secure_context_rebinds_the_port_of_a_dropped_one() -> TestResult {
    let alice = KeyPair::generate()?;
    let server = KeyPair::generate()?;
    let mut keys = AuthorizedKeys::new();
    keys.insert("alice", alice.public)?;
    let client = CurveClient {
        keys: alice,
        server_key: server.public,
    };
    let ctx = Context::new()?;
    let mut ep = "tcp://127.0.0.1:*".to_owned();
    for round in 0..20u8 {
        let secure = SecureContext::new("ac2", keys.clone(), |_| {})?;
        let router = secure.curve_server_socket(SocketType::Router, &server)?;
        router.set_linger(Some(std::time::Duration::ZERO))?;
        router.bind(&ep)?;
        ep = router.last_endpoint()?;
        let d = dealer(&ctx, &ep, Some(&client))?;
        round_trip(&router, &d, round)?;
        drop(router);
        drop(secure);
        drop(d);
    }
    Ok(())
}

/// A plain local ROUTER and a CURVE ROUTER from a separate secure context, polled together
/// on one thread, each with a client that picked the same routing id: the id names a peer
/// only together with the socket it came in on, and libzmq routes each reply on its own
/// socket. The daemon's io thread keys peers by (origin, id) for this reason.
#[test]
fn local_and_network_routers_share_one_poll_and_routing_ids() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ctx = Context::new()?;
    let local = ctx.socket(SocketType::Router)?;
    let local_ep = bind_endpoint(Transport::Tcp, dir.path(), "local");
    local.bind(&local_ep)?;
    let local_ep = local.last_endpoint()?;

    let alice = KeyPair::generate()?;
    let server = KeyPair::generate()?;
    let mut keys = AuthorizedKeys::new();
    keys.insert("alice", alice.public)?;
    let secure = SecureContext::new("ac2", keys, |_| {})?;
    let net = secure.curve_server_socket(SocketType::Router, &server)?;
    net.bind("tcp://127.0.0.1:*")?;
    let net_ep = net.last_endpoint()?;

    let a = ctx.socket(SocketType::Dealer)?;
    a.set_routing_id(b"same")?;
    a.connect(&local_ep)?;
    let b = ctx.socket(SocketType::Dealer)?;
    b.set_routing_id(b"same")?;
    b.set_curve_client(&CurveClient {
        keys: alice,
        server_key: server.public,
    })?;
    b.connect(&net_ep)?;
    a.send(&[b"from-local"])?;
    b.send(&[b"from-net"])?;

    let mut seen = Vec::new();
    while seen.len() < 2 {
        let mut items = [PollItem::readable(&local), PollItem::readable(&net)];
        poll(&mut items, Some(TIMEOUT))?;
        let (l, n) = (items[0].is_readable(), items[1].is_readable());
        for (ready, sock, origin) in [(l, &local, "local"), (n, &net, "net")] {
            if ready && let Some(m) = sock.try_recv()? {
                assert_eq!(m.frames()[0], b"same");
                // Only the network pair carries a ZAP identity.
                assert_eq!(m.user_id().is_some(), origin == "net");
                sock.send(&[b"same".as_slice(), origin.as_bytes()])?;
                seen.push(origin);
            }
        }
    }
    let ra = a.recv_timeout(TIMEOUT)?.ok_or("no local reply")?;
    let rb = b.recv_timeout(TIMEOUT)?.ok_or("no network reply")?;
    assert_eq!(ra.frames(), &[b"local".to_vec()]);
    assert_eq!(rb.frames(), &[b"net".to_vec()]);
    Ok(())
}

/// Off then on at once, as the daemon does it: the old pair (200 ms linger, a client still
/// connected and a message still queued to it) is dropped on a closer thread while the new
/// pair binds the same port. The new bind may meet the old listener: it fails "address in
/// use", its own context is torn down, and a retry binds once the closer is done.
#[test]
fn rebind_while_the_old_pair_closes_elsewhere_succeeds_on_retry() -> TestResult {
    let alice = KeyPair::generate()?;
    let server = KeyPair::generate()?;
    let mut keys = AuthorizedKeys::new();
    keys.insert("alice", alice.public)?;
    let client = CurveClient {
        keys: alice,
        server_key: server.public,
    };
    let ctx = Context::new()?;
    let old_secure = SecureContext::new("ac2", keys.clone(), |_| {})?;
    let old = old_secure.curve_server_socket(SocketType::Router, &server)?;
    old.set_linger(Some(std::time::Duration::from_millis(200)))?;
    old.bind("tcp://127.0.0.1:*")?;
    let ep = old.last_endpoint()?;
    let d = dealer(&ctx, &ep, Some(&client))?;
    round_trip(&old, &d, 1)?;
    let rid = {
        d.send(&[[2u8]])?;
        let m = old.recv_timeout(TIMEOUT)?.ok_or("no request")?;
        m.frames()[0].clone()
    };
    old.send(&[rid.as_slice(), &[3]])?;

    let closer = std::thread::spawn(move || {
        drop(old);
        drop(old_secure);
    });
    let started = std::time::Instant::now();
    let (secure, router) = loop {
        let secure = SecureContext::new("ac2", keys.clone(), |_| {})?;
        let router = secure.curve_server_socket(SocketType::Router, &server)?;
        match router.bind(&ep) {
            Ok(()) => break (secure, router),
            Err(ac2_zmq::Error::AddressInUse) if started.elapsed() < TIMEOUT => {
                drop(router);
                drop(secure);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(e) => return Err(e.into()),
        }
    };
    closer.join().map_err(|_| "closer panicked")?;
    let d2 = dealer(&ctx, &ep, Some(&client))?;
    round_trip(&router, &d2, 4)?;
    drop(router);
    drop(secure);
    Ok(())
}
