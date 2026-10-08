//! ROUTER (daemon) / DEALER (clients): async request-reply matched by request id.

use crate::common;

use ac2_zmq::{Context, SocketType};
use common::*;

#[test]
fn slow_request_does_not_block_other_clients_or_later_requests() -> TestResult {
    let ctx = Context::new()?;
    let server = CtrlServer::start(&ctx, ctx.socket(SocketType::Router)?, "tcp://127.0.0.1:*")?;
    let a = dealer(&ctx, server.endpoint(), None)?;
    let b = dealer(&ctx, server.endpoint(), None)?;

    // A's first request parks on a gate only the test opens (deterministic "slow").
    send_req(&a, 1, b"slow:\x01")?;
    // The server holds it before B starts, so B really runs concurrently with it.
    wait_until(|| server.seen().iter().any(|(_, id)| *id == 1))?;

    for id in 10..20 {
        send_req(&b, id, format!("echo:b{id}").as_bytes())?;
    }
    let mut got = (0..10)
        .map(|_| recv_reply(&b))
        .collect::<Result<Vec<_>, _>>()?;
    got.sort_unstable();
    let want: Vec<_> = (10..20).map(|id| (id, format!("b{id}"))).collect();
    assert_eq!(got, want);

    // The same client's later request overtakes its parked one: replies match by id.
    send_req(&a, 2, b"echo:a2")?;
    assert_eq!(recv_reply(&a)?, (2, "a2".to_owned()));
    assert_eq!(a.try_recv()?, None, "slow request answered before its gate");

    server.open_gate(1)?;
    assert_eq!(recv_reply(&a)?, (1, "slow 1 done".to_owned()));
    Ok(())
}

#[test]
fn many_concurrent_clients_get_their_own_replies() -> TestResult {
    let ctx = Context::new()?;
    let server = CtrlServer::start(&ctx, ctx.socket(SocketType::Router)?, "tcp://127.0.0.1:*")?;
    let ep = server.endpoint().to_owned();
    let clients: Vec<_> = (0..8u64)
        .map(|c| {
            let (ctx, ep) = (ctx.clone(), ep.clone());
            std::thread::spawn(move || -> Result<(), String> {
                let d = dealer(&ctx, &ep, None).map_err(|e| e.to_string())?;
                for i in 0..50 {
                    let id = c * 1000 + i;
                    send_req(&d, id, format!("echo:{id}").as_bytes()).map_err(|e| e.to_string())?;
                }
                for i in 0..50 {
                    let id = c * 1000 + i;
                    // One DEALER, one connection: replies come back in order.
                    let r = recv_reply(&d).map_err(|e| e.to_string())?;
                    if r != (id, id.to_string()) {
                        return Err(format!("client {c} got {r:?}, wanted {id}"));
                    }
                }
                Ok(())
            })
        })
        .collect();
    for c in clients {
        c.join().map_err(|_| "client thread panicked")??;
    }
    assert_eq!(server.seen().len(), 400);
    Ok(())
}
