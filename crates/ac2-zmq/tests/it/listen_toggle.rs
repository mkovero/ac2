//! What libzmq does when a daemon socket gains and loses its network endpoint at runtime: the
//! facts the live network switch relies on (docs/design/network-toggle.md). One socket
//! stays bound to its local endpoint while a tcp endpoint comes and goes beside it.

use crate::common;

use std::sync::{Arc, Mutex};

use ac2_zmq::{
    AuthorizedKeys, Context, CurveClient, KeyPair, MonitorEvent, SecureContext, Socket, SocketType,
    ZapDecision,
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

fn tcp_router(ctx: &Context) -> R<(Socket, String)> {
    let router = ctx.socket(SocketType::Router)?;
    router.set_router_mandatory(true)?;
    router.bind("tcp://127.0.0.1:*")?;
    let ep = router.last_endpoint()?;
    Ok((router, ep))
}

/// Unbinding the tcp endpoint ends the connections accepted through it at once on the
/// server side (libzmq owns them as children of their listener), while the socket's
/// connection on its other (local) endpoint stays up. Turning the network off is therefore
/// enough to cut every remote client; nothing has to track them. The client notices at its
/// next send (or its keepalive deadline), not at the unbind itself.
#[cfg(unix)]
#[test]
fn unbind_ends_accepted_tcp_connections_and_keeps_local_ones() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ctx = Context::new()?;
    let (router, tcp) = tcp_router(&ctx)?;
    let ipc = format!("ipc://{}", dir.path().join("ctrl").display());
    router.bind(&ipc)?;

    let remote = ctx.socket(SocketType::Dealer)?;
    remote.set_routing_id(b"remote")?;
    let remote_mon = remote.monitor()?;
    remote.connect(&tcp)?;
    let local = dealer(&ctx, &ipc, None)?;
    round_trip(&router, &remote, 1)?;
    round_trip(&router, &local, 2)?;
    remote_mon.wait_for(TIMEOUT, |e| *e == MonitorEvent::HandshakeSucceeded)?;

    router.unbind(&tcp)?;
    // With ROUTER_MANDATORY a send to a vanished routing id fails instead of dropping.
    wait_until(|| router.send(&[&b"remote"[..], &[9]]).is_err())?;
    round_trip(&router, &local, 3)?;

    remote.send(&[[4u8]])?;
    let gone = remote_mon.wait_for(TIMEOUT, |e| *e == MonitorEvent::Disconnected)?;
    assert!(gone.is_some(), "the tcp client never noticed");
    let late = router.recv_timeout(std::time::Duration::from_millis(300))?;
    assert!(late.is_none(), "a request crossed an unbound endpoint");
    // The client keeps retrying and is refused at the TCP level; it never gets back in.
    let back = remote_mon.wait_for(std::time::Duration::from_millis(500), |e| {
        *e == MonitorEvent::HandshakeSucceeded
    })?;
    assert!(back.is_none(), "reconnected after unbind");
    round_trip(&router, &local, 5)?;
    Ok(())
}

/// Binding the same tcp endpoint right after unbind can fail with "address in use": unbind
/// returns before libzmq's I/O thread has closed the listener (seen both ways here). A
/// rebind on the same socket has to retry; TIME_WAIT of the closed connection is no
/// obstacle, libzmq sets SO_REUSEADDR on its listeners.
#[test]
fn tcp_endpoint_rebind_after_unbind_needs_a_retry() -> TestResult {
    let ctx = Context::new()?;
    let (router, tcp) = tcp_router(&ctx)?;
    let first = dealer(&ctx, &tcp, None)?;
    round_trip(&router, &first, 1)?;

    router.unbind(&tcp)?;
    drop(first);
    let started = std::time::Instant::now();
    loop {
        match router.bind(&tcp) {
            Ok(()) => break,
            Err(ac2_zmq::Error::AddressInUse) if started.elapsed() < TIMEOUT => {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(e) => return Err(e.into()),
        }
    }
    let second = dealer(&ctx, &tcp, None)?;
    round_trip(&router, &second, 2)?;

    // Unbinding what is not bound is an error, not a silent no-op.
    router.unbind(&tcp)?;
    assert!(router.unbind(&tcp).is_err());
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
/// accepted socket, never from the peer's bytes: "localhost" on ipc, "127.0.0.1" on
/// loopback tcp. Neither names a user: loopback is every local user and every tunnel or
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
        // The libzmq built here does not read the peer's credentials (SO_PEERCRED), so ipc
        // says only "localhost"; the socket file's permissions are the whole boundary.
        assert_eq!(c.refused_address(&ipc)?, "localhost");
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
