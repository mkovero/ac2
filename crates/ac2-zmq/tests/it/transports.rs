//! The same ctrl + data round trip on every transport ac2 uses: tcp://127.0.0.1, ipc://
//! (Linux/macOS), inproc:// (embedded daemon). CURVE runs on tcp and ipc; libzmq never runs a
//! security mechanism on inproc.

use crate::common;

use ac2_zmq::{AuthorizedKeys, Context, CurveClient, KeyPair, SecureContext, SocketType};
use common::*;

fn round_trip(t: Transport, curve: bool) -> TestResult {
    let dir = tempfile::tempdir()?;
    let client_keys = KeyPair::generate()?;
    let server_keys = KeyPair::generate()?;
    let mut keys = AuthorizedKeys::new();
    keys.insert("local", client_keys.public)?;
    let secure = SecureContext::new("ac2", keys, |_| {})?;
    let ctx = secure.context();
    let (router, xpub) = if curve {
        (
            secure.curve_server_socket(SocketType::Router, &server_keys)?,
            secure.curve_server_socket(SocketType::XPub, &server_keys)?,
        )
    } else {
        (
            ctx.socket(SocketType::Router)?,
            ctx.socket(SocketType::XPub)?,
        )
    };
    let client_cfg = CurveClient {
        keys: client_keys,
        server_key: server_keys.public,
    };
    let client_cfg = curve.then_some(&client_cfg);
    // inproc endpoints live in one context; network clients get their own, as in a separate
    // process.
    let client_ctx = match t {
        Transport::Inproc => ctx.clone(),
        _ => Context::new()?,
    };
    let want_user = curve.then_some("local".to_owned());

    let server = CtrlServer::start(ctx, router, &bind_endpoint(t, dir.path(), "ctrl"))?;
    let d = dealer(&client_ctx, server.endpoint(), client_cfg)?;
    send_req(&d, 1, format!("echo:{t:?}").as_bytes())?;
    assert_eq!(recv_reply(&d)?, (1, format!("{t:?}")));
    assert_eq!(server.seen(), vec![(want_user.clone(), 1)]);

    xpub.set_xpub_verbose(true)?;
    xpub.bind(&bind_endpoint(t, dir.path(), "data"))?;
    let s = sub(&client_ctx, &xpub.last_endpoint()?, client_cfg, &["d/"])?;
    wait_sub_event(&xpub, &subscribed("d/"))?;
    let payload: Vec<u8> = (0..=255).collect();
    xpub.send(&frame("d/m/tf", 3, &payload))?;
    assert_eq!(recv(&s)?.frames(), frame("d/m/tf", 3, &payload).as_slice());
    Ok(())
}

#[test]
fn tcp_loopback() -> TestResult {
    round_trip(Transport::Tcp, false)
}

#[test]
fn tcp_loopback_curve() -> TestResult {
    round_trip(Transport::Tcp, true)
}

#[cfg(unix)]
#[test]
fn ipc() -> TestResult {
    round_trip(Transport::Ipc, false)
}

#[cfg(unix)]
#[test]
fn ipc_curve() -> TestResult {
    round_trip(Transport::Ipc, true)
}

#[test]
fn inproc() -> TestResult {
    round_trip(Transport::Inproc, false)
}
