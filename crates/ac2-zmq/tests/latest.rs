//! Latest-wins delivery: what a stalled subscriber has queued, and how `drain_latest` gets it
//! back to fresh frames.

mod common;

use std::collections::BTreeMap;

use ac2_zmq::{Context, Socket, SocketType, drain_latest};
use common::*;

const TOPICS: [&str; 2] = ["d/a/tf", "d/b/tf"];
const STALL_ROUNDS: u64 = 1000;
const HWM: u32 = 4;
/// Upper bound on messages read per drain pass.
const DRAIN_LIMIT: usize = 10_000;

fn publish_round(xpub: &Socket, seq: u64) -> TestResult {
    let payload = [0u8; 6144];
    for t in TOPICS {
        xpub.send(&frame(t, seq, &payload))?;
    }
    Ok(())
}

fn stalled_pair(ctx: &Context, bind: &str, small_kernel_buffers: bool) -> R<(Socket, Socket)> {
    let xpub = ctx.socket(SocketType::XPub)?;
    xpub.set_xpub_verbose(true)?;
    xpub.set_send_hwm(HWM)?;
    let s = ctx.socket(SocketType::Sub)?;
    s.set_recv_hwm(HWM)?;
    if small_kernel_buffers {
        // Keeps the TCP backlog at a few frames instead of MBs.
        for sock in [&xpub, &s] {
            sock.set_send_buffer(Some(16 * 1024))?;
            sock.set_recv_buffer(Some(16 * 1024))?;
        }
    }
    xpub.bind(bind)?;
    for t in TOPICS {
        s.subscribe(t.as_bytes())?;
    }
    s.connect(&xpub.last_endpoint()?)?;
    for t in TOPICS {
        wait_sub_event(&xpub, &subscribed(t))?;
    }
    Ok((xpub, s))
}

/// inproc has no kernel buffers, so the backlog is exactly what the HWMs allow.
#[test]
fn inproc_backlog_is_bounded_by_hwm_and_one_drain_recovers() -> TestResult {
    let ctx = Context::new()?;
    let (xpub, s) = stalled_pair(&ctx, &format!("inproc://{}", unique("latest")), false)?;

    // The subscriber stalls (reads nothing) while the publisher sends 1000 rounds.
    for seq in 0..STALL_ROUNDS {
        publish_round(&xpub, seq)?;
    }

    let d = drain_latest(&s, DRAIN_LIMIT, seq_of)?;
    assert!(d.emptied);
    assert_eq!(d.malformed, 0);
    // XPUB drops the newest messages once a peer's pipe is full: the queue holds the start of
    // the stall, so a naive reader would replay frames from seq 0.
    assert_eq!(d.oldest_seq, Some(0));
    // inproc pipe capacity is SNDHWM + RCVHWM whole messages per peer.
    assert!(
        d.read <= 2 * HWM as usize + 2,
        "backlog {} exceeds HWM bound",
        d.read
    );
    assert_eq!(d.latest.len(), TOPICS.len());

    // After one drain the pipe is empty and no stale frame can appear any more. The
    // publisher learns about the freed space asynchronously, so the first round or two after
    // the drain may still be dropped; keep publishing (as the daemon would at frame rate).
    let mut seq = STALL_ROUNDS;
    let mut fresh = BTreeMap::new();
    let mut stale = None;
    wait_until(|| {
        if publish_round(&xpub, seq).is_err() {
            return false;
        }
        seq += 1;
        if let Ok(d) = drain_latest(&s, DRAIN_LIMIT, seq_of) {
            if let Some(o) = d.oldest_seq.filter(|o| *o < STALL_ROUNDS) {
                stale = Some(o);
            }
            fresh.extend(d.latest);
        }
        stale.is_some() || TOPICS.iter().all(|t| fresh.contains_key(t.as_bytes()))
    })?;
    assert_eq!(stale, None, "stale frame after the drain");
    Ok(())
}

/// Over TCP the kernel buffers add to the HWMs, so the backlog is larger and timing-dependent.
/// The drain still converges: with the publisher running, the newest frame per topic reaches
/// a post-stall seq once the stale backlog has been consumed.
#[test]
fn tcp_drain_recovers_to_fresh_frames() -> TestResult {
    let ctx = Context::new()?;
    let (xpub, s) = stalled_pair(&ctx, "tcp://127.0.0.1:*", true)?;
    for seq in 0..STALL_ROUNDS {
        publish_round(&xpub, seq)?;
    }

    let mut seq = STALL_ROUNDS;
    let mut newest: BTreeMap<Vec<u8>, u64> = BTreeMap::new();
    let mut malformed = 0;
    wait_until(|| {
        if publish_round(&xpub, seq).is_err() {
            return false;
        }
        seq += 1;
        // Wait for the next frame so each pass makes progress without spinning.
        if !s.wait_readable(TIMEOUT).unwrap_or(false) {
            return false;
        }
        let Ok(d) = drain_latest(&s, DRAIN_LIMIT, seq_of) else {
            return false;
        };
        malformed += d.malformed;
        for (topic, l) in d.latest {
            let n = newest.entry(topic).or_default();
            *n = (*n).max(l.seq);
        }
        TOPICS
            .iter()
            .all(|t| newest.get(t.as_bytes()).is_some_and(|n| *n >= STALL_ROUNDS))
    })?;
    assert_eq!(malformed, 0);
    Ok(())
}

#[test]
fn drain_counts_malformed_and_respects_limit() -> TestResult {
    let ctx = Context::new()?;
    let ep = format!("inproc://{}", unique("drain"));
    let push = ctx.socket(SocketType::Push)?;
    push.bind(&ep)?;
    let pull = ctx.socket(SocketType::Pull)?;
    pull.connect(&ep)?;
    push.send(&frame("t", 5, b"first"))?;
    push.send(&[b"garbage".as_slice()])?;
    push.send(&frame("t", 3, b""))?;
    push.send(&frame("t", 5, b"second"))?;
    push.send(&frame("t", 5, b"third"))?;
    push.send(&frame("u", 1, b""))?;
    // inproc: every message sent is in the pipe once the first is readable.
    assert!(pull.wait_readable(TIMEOUT)?);

    let first = drain_latest(&pull, 2, seq_of)?;
    assert_eq!((first.read, first.malformed, first.emptied), (2, 1, false));
    assert_eq!(first.latest[b"t".as_slice()].seq, 5);

    let rest = drain_latest(&pull, DRAIN_LIMIT, seq_of)?;
    assert_eq!((rest.read, rest.malformed, rest.emptied), (4, 0, true));
    // A lower seq never replaces a higher one; on a tie the first message read is kept.
    let t = &rest.latest[b"t".as_slice()];
    assert_eq!(
        (t.seq, t.message.frames()[2].as_slice()),
        (5, b"second".as_slice())
    );
    assert_eq!(rest.latest[b"u".as_slice()].seq, 1);
    assert_eq!(rest.oldest_seq, Some(1));
    Ok(())
}
