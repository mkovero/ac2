//! The same ctrl + data round trip on every transport ac2 uses: tcp://127.0.0.1, ipc://
//! (Linux/macOS), inproc:// (embedded daemon). CURVE is exercised on tcp and ipc; inproc
//! never leaves the process and libzmq does not run security mechanisms on it.

mod common;

use std::sync::Arc;

use common::*;
use spike_zmq_curve::ctrl::{ClientSecurity, CtrlServer, Gates, ServerSecurity};
use spike_zmq_curve::data::{SubEvent, bind_xpub};
use spike_zmq_curve::proto::{Cmd, decode_frame, encode_frame};
use spike_zmq_curve::zap::{AuthorizedClients, ZapHandler};
use spike_zmq_curve::zmq::{Context, CurveKeyPair, z85_decode_key};

fn round_trip(t: Transport, curve: bool) -> TestResult {
    let dir = tempfile::tempdir()?;
    let ctx = Context::new()?;
    let (server_sec, client_sec, _zap) = if curve {
        let server = CurveKeyPair::generate()?;
        let client = CurveKeyPair::generate()?;
        let mut list = AuthorizedClients::new();
        list.insert(z85_decode_key(&client.public)?, "local".into());
        let zap = ZapHandler::start(&ctx, "ac2", list)?;
        (
            ServerSecurity::Curve {
                keys: server.clone(),
                zap_domain: "ac2".into(),
            },
            ClientSecurity::Curve {
                keys: client,
                server_public: server.public,
            },
            Some(zap),
        )
    } else {
        (ServerSecurity::Null, ClientSecurity::Null, None)
    };

    let ctrl_ep = bind_endpoint(t, dir.path(), "ctrl");
    let server = CtrlServer::start(&ctx, &ctrl_ep, &server_sec, Arc::new(Gates::default()))?;
    let d = dealer(&ctx, server.endpoint(), &client_sec)?;
    send_req(
        &d,
        1,
        Cmd::Echo {
            text: format!("{t:?}"),
        },
    )?;
    let r = recv_reply(&d)?;
    assert_eq!((r.id, r.result), (1, Ok(format!("{t:?}"))));

    let xpub = bind_xpub(
        &ctx,
        &bind_endpoint(t, dir.path(), "data"),
        &server_sec,
        100,
    )?;
    let s = sub(&ctx, &xpub.last_endpoint()?, &client_sec, &["d/"])?;
    wait_sub_event(&xpub, SubEvent::Subscribe, "d/")?;
    let values = tf_values(3);
    xpub.send_multipart(&encode_frame("d/m/tf", &header(3, TF_N), &values), 0)?;
    let f = decode_frame(&s.recv_timeout(TIMEOUT)?.ok_or("no frame")?.frames)?;
    assert_eq!((f.header.seq, f.values), (3, values));
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
