//! Zero-copy message parts: a `Vec` handed to libzmq arrives intact, a received part can be
//! forwarded (and shared between several sends) without copying, and an undeliverable send
//! leaves the caller's parts usable.

use crate::common;

use ac2_zmq::{Context, Error, Part, SocketType};
use common::*;

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31) ^ seed)
        .collect()
}

#[test]
fn owned_parts_round_trip_and_forward() -> TestResult {
    let ctx = Context::new()?;
    let ep = format!("inproc://{}", unique("parts"));
    let pull = ctx.socket(SocketType::Pull)?;
    pull.bind(&ep)?;
    let push = ctx.socket(SocketType::Push)?;
    push.connect(&ep)?;

    // Small (stored inline by libzmq), empty and large parts in one message.
    let big = pattern(1 << 20, 7);
    let parts = vec![
        Part::copy_from(b"F")?,
        Part::from_vec(Vec::new())?,
        Part::from_vec(big.clone())?,
    ];
    push.try_send_parts(&parts)?;
    drop(parts);
    let got = wait_parts(&pull)?;
    assert_eq!(got.len(), 3);
    assert_eq!(&*got[0], b"F");
    assert!(got[1].is_empty());
    assert_eq!(&*got[2], big.as_slice());

    // Forward the received parts to two subscribers, twice: content is shared, not moved.
    let xpub = ctx.socket(SocketType::XPub)?;
    xpub.set_xpub_verbose(true)?;
    let xep = format!("inproc://{}", unique("parts-x"));
    xpub.bind(&xep)?;
    let subs = (0..2)
        .map(|_| -> R<_> {
            let s = ctx.socket(SocketType::Sub)?;
            s.subscribe(b"")?;
            s.connect(&xep)?;
            Ok(s)
        })
        .collect::<R<Vec<_>>>()?;
    for _ in 0..2 {
        // Subscriptions reach the XPUB asynchronously.
        let _ = xpub.recv_timeout(TIMEOUT)?;
    }
    let fwd = &got[..];
    xpub.try_send_parts(fwd)?;
    xpub.try_send_parts(fwd)?;
    for s in &subs {
        for _ in 0..2 {
            let m = s.recv_timeout(TIMEOUT)?.ok_or("nothing forwarded")?;
            assert_eq!(m.frames()[0], b"F");
            assert_eq!(m.frames()[2], big);
        }
    }
    assert_eq!(&*got[2], big.as_slice(), "the source part is intact");

    // Shares outlive the original.
    let shared = got[2].share()?;
    drop(got);
    assert_eq!(&*shared, big.as_slice());
    Ok(())
}

#[test]
fn would_block_keeps_the_parts() -> TestResult {
    let ctx = Context::new()?;
    // A PUSH without a peer cannot queue anything.
    let push = ctx.socket(SocketType::Push)?;
    let parts = vec![Part::from_vec(pattern(4096, 1))?];
    assert_eq!(push.try_send_parts(&parts), Err(Error::WouldBlock));
    assert_eq!(&*parts[0], pattern(4096, 1).as_slice());
    assert!(matches!(
        push.try_send_parts(&[]),
        Err(Error::InvalidArgument(_))
    ));
    let pull = ctx.socket(SocketType::Pull)?;
    assert!(pull.try_recv_parts()?.is_none());
    Ok(())
}

fn wait_parts(s: &ac2_zmq::Socket) -> R<Vec<Part>> {
    if !s.wait_readable(TIMEOUT)? {
        return Err("nothing arrived".into());
    }
    s.try_recv_parts()?
        .ok_or_else(|| "readable but empty".into())
}

/// A part over the receiver's bound never arrives: the connection that carried it closes and
/// the sender's next message comes over a new one.
#[test]
fn a_part_over_the_bound_closes_its_connection() -> TestResult {
    let ctx = Context::new()?;
    let pull = ctx.socket(SocketType::Pull)?;
    assert_eq!(pull.max_message_size()?, None);
    pull.set_max_message_size(Some(1024))?;
    assert_eq!(pull.max_message_size()?, Some(1024));
    pull.bind("tcp://127.0.0.1:*")?;
    let push = ctx.socket(SocketType::Push)?;
    push.set_reconnect_interval(std::time::Duration::from_millis(10))?;
    push.connect(&pull.last_endpoint()?)?;

    push.send(&[pattern(1024, 1)])?;
    assert_eq!(wait_parts(&pull)?[0].len(), 1024);
    push.send(&[pattern(4096, 2)])?;
    // Sent until one arrives: the first sends may land on the closed connection's pipe.
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        push.try_send(&[b"after".as_slice()]).ok();
        if let Some(p) = pull.recv_timeout(std::time::Duration::from_millis(50))? {
            assert_eq!(
                p.frames()[0].as_slice(),
                b"after",
                "the oversize part arrived"
            );
            return Ok(());
        }
        if std::time::Instant::now() > deadline {
            return Err("nothing arrived after the oversize part".into());
        }
    }
}
