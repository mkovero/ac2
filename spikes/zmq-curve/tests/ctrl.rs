//! ROUTER (daemon) / DEALER (client) async request-reply.

mod common;

use std::sync::Arc;

use common::*;
use spike_zmq_curve::ctrl::{ClientSecurity, CtrlServer, Gates, ServerSecurity};
use spike_zmq_curve::proto::{Cmd, ErrorCode, Request, encode_request};
use spike_zmq_curve::zmq::Context;

#[test]
fn slow_handler_does_not_block_other_client() -> TestResult {
    let ctx = Context::new()?;
    let gates = Arc::new(Gates::default());
    let server = CtrlServer::start(
        &ctx,
        "tcp://127.0.0.1:*",
        &ServerSecurity::Null,
        gates.clone(),
    )?;
    let a = dealer(&ctx, server.endpoint(), &ClientSecurity::Null)?;
    let b = dealer(&ctx, server.endpoint(), &ClientSecurity::Null)?;

    // A's first request parks on a gate only the test opens.
    send_req(&a, 1, Cmd::Slow { gate: 1 })?;
    // The daemon has the slow request before B starts (so B really runs concurrently with it).
    wait_until(|| server.seen().iter().any(|(_, r)| r.id == 1))?;

    for id in 10..20 {
        send_req(
            &b,
            id,
            Cmd::Echo {
                text: format!("b{id}"),
            },
        )?;
    }
    let mut got: Vec<u64> = (0..10)
        .map(|_| recv_reply(&b).map(|r| r.id))
        .collect::<Result<_, _>>()?;
    got.sort_unstable();
    assert_eq!(got, (10..20).collect::<Vec<_>>());

    // Same client, later request overtakes the parked one: replies are matched by id, not order.
    send_req(&a, 2, Cmd::Echo { text: "a2".into() })?;
    let r = recv_reply(&a)?;
    assert_eq!((r.id, r.result), (2, Ok("a2".to_owned())));
    assert!(
        a.try_recv()?.is_none(),
        "slow request answered before its gate opened"
    );

    gates.open(1);
    let r = recv_reply(&a)?;
    assert_eq!((r.id, r.result), (1, Ok("slow 1 done".to_owned())));
    Ok(())
}

#[test]
fn bad_version_and_garbage_get_typed_errors() -> TestResult {
    let ctx = Context::new()?;
    let server = CtrlServer::start(
        &ctx,
        "tcp://127.0.0.1:*",
        &ServerSecurity::Null,
        Arc::new(Gates::default()),
    )?;
    let c = dealer(&ctx, server.endpoint(), &ClientSecurity::Null)?;

    c.send_multipart(
        &[encode_request(&Request {
            v: 99,
            id: 5,
            cmd: Cmd::Whoami,
        })],
        0,
    )?;
    let r = recv_reply(&c)?;
    assert_eq!(r.id, 5);
    assert_eq!(
        r.result.map_err(|e| e.code),
        Err(ErrorCode::VersionMismatch)
    );

    c.send_multipart(&[b"\xc1not msgpack".as_slice()], 0)?;
    let r = recv_reply(&c)?;
    assert_eq!(r.result.map_err(|e| e.code), Err(ErrorCode::BadRequest));
    Ok(())
}
