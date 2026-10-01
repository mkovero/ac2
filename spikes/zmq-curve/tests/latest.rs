//! Latest-wins delivery: what a stalled subscriber sees, and how the client drain recovers.
//!
//! Findings are printed (`cargo test -p spike-zmq-curve --test latest -- --nocapture`) and
//! recorded in docs/design/spike-zmq-curve.md.

mod common;

use common::*;
use spike_zmq_curve::ctrl::{ServerSecurity, unique};
use spike_zmq_curve::data::{SubEvent, bind_xpub, drain_latest};
use spike_zmq_curve::ffi;
use spike_zmq_curve::proto::{decode_frame, encode_frame};
use spike_zmq_curve::zmq::{Context, Socket, SocketType};

const TOPICS: [&str; 2] = ["d/a/tf", "d/b/tf"];
const STALL_FRAMES: u64 = 1000;
const HWM: i32 = 4;

fn publish_round(xpub: &Socket, seq: u64) -> TestResult {
    for t in TOPICS {
        xpub.send_multipart(&encode_frame(t, &header(seq, TF_N), &tf_values(seq)), 0)?;
    }
    Ok(())
}

fn stalled_pair(
    ctx: &Context,
    bind: &str,
    tcp_buffers: bool,
) -> Result<(Socket, Socket), Box<dyn std::error::Error>> {
    let xpub = ctx.socket(SocketType::XPub)?;
    xpub.set_int(ffi::ZMQ_XPUB_VERBOSE, 1)?;
    xpub.set_int(ffi::ZMQ_SNDHWM, HWM)?;
    if tcp_buffers {
        small_kernel_buffers(&xpub)?;
    }
    xpub.bind(bind)?;
    let ep = xpub.last_endpoint()?;

    let s = ctx.socket(SocketType::Sub)?;
    s.set_int(ffi::ZMQ_RCVHWM, HWM)?;
    if tcp_buffers {
        small_kernel_buffers(&s)?;
    }
    for t in TOPICS {
        s.subscribe(t.as_bytes())?;
    }
    s.connect(&ep)?;
    for t in TOPICS {
        wait_sub_event(&xpub, SubEvent::Subscribe, t)?;
    }
    Ok((xpub, s))
}

/// inproc has no kernel buffers, so the backlog is exactly what the HWMs allow.
#[test]
fn stalled_subscriber_inproc_backlog_is_bounded_by_hwm_and_drain_recovers() -> TestResult {
    let ctx = Context::new()?;
    let (xpub, s) = stalled_pair(&ctx, &format!("inproc://{}", unique("latest")), false)?;

    // Subscriber stalls (does not read) while the daemon publishes 1000 rounds.
    for seq in 0..STALL_FRAMES {
        publish_round(&xpub, seq)?;
    }

    // What a naive reader gets first: the OLDEST frame. XPUB drops new frames once a peer's
    // pipe is full, so the queue holds the start of the stall, not its end.
    let first = decode_frame(&s.recv_timeout(TIMEOUT)?.ok_or("nothing queued")?.frames)?;
    assert_eq!(first.header.seq, 0);

    let d = drain_latest(&s)?;
    let backlog = d.read + 1;
    let newest_queued = d.latest.values().map(|f| f.header.seq).max().unwrap_or(0);
    println!(
        "inproc: SNDHWM=RCVHWM={HWM}, {} published, {backlog} queued (newest queued seq \
         {newest_queued}), {} dropped",
        STALL_FRAMES * 2,
        STALL_FRAMES as usize * 2 - backlog
    );
    // inproc pipe capacity is SNDHWM + RCVHWM messages per peer.
    assert!(
        backlog <= 2 * HWM as usize + 2,
        "backlog {backlog} exceeds HWM bound"
    );
    assert!(newest_queued < 10, "queue should hold the stall's start");

    // After one drain the pipe is empty. The daemon keeps publishing at frame rate; the
    // publisher learns the pipe has room again asynchronously, so the first round or two after
    // the drain may still be dropped, but no stale frame can appear any more.
    let mut seq = STALL_FRAMES;
    let mut fresh = std::collections::BTreeMap::new();
    let mut read = 0;
    wait_until(|| {
        if publish_round(&xpub, seq).is_err() {
            return false;
        }
        seq += 1;
        if let Ok(d) = drain_latest(&s) {
            assert!(
                d.oldest_seq.is_none_or(|o| o >= STALL_FRAMES),
                "stale frame after drain"
            );
            read += d.read;
            fresh.extend(d.latest);
        }
        TOPICS.iter().all(|t| fresh.contains_key(*t))
    })?;
    println!(
        "inproc: fresh on both topics after {} publish rounds, {read} frames read",
        seq - STALL_FRAMES
    );
    Ok(())
}

/// Over TCP the kernel socket buffers add to the HWMs, so the backlog (and with it the age of
/// the oldest queued frame) is larger and timing-dependent. The drain still converges: the
/// publisher keeps sending (as the daemon would at frame rate) and the newest seq per topic
/// reaches a fresh frame after the stale backlog is consumed.
#[test]
fn stalled_subscriber_tcp_drain_recovers_to_fresh_frames() -> TestResult {
    let ctx = Context::new()?;
    let (xpub, s) = stalled_pair(&ctx, "tcp://127.0.0.1:*", true)?;

    for seq in 0..STALL_FRAMES {
        publish_round(&xpub, seq)?;
    }

    let mut seq = STALL_FRAMES;
    let mut total_read = 0usize;
    let mut passes = 0usize;
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        publish_round(&xpub, seq)?;
        // Block for one frame so each pass makes progress without spinning, then drain.
        let Some(m) = s.recv_timeout(TIMEOUT)? else {
            return Err("subscriber starved".into());
        };
        let first = decode_frame(&m.frames)?;
        let mut d = drain_latest(&s)?;
        d.read += 1;
        if d.latest
            .get(&first.topic)
            .is_none_or(|old| old.header.seq < first.header.seq)
        {
            d.latest.insert(first.topic.clone(), first);
        }
        total_read += d.read;
        passes += 1;
        if TOPICS.iter().all(|t| {
            d.latest
                .get(*t)
                .is_some_and(|f| f.header.seq >= STALL_FRAMES)
        }) {
            break;
        }
        if std::time::Instant::now() > deadline {
            return Err("never recovered to fresh frames".into());
        }
        seq += 1;
    }
    let fresh_published = (seq - STALL_FRAMES + 1) as usize * TOPICS.len();
    println!(
        "tcp: SNDHWM=RCVHWM={HWM}, SNDBUF=RCVBUF=16KiB: recovered after {passes} drain passes, \
         {total_read} frames read, of which at most {fresh_published} fresh; publisher at \
         seq {seq}"
    );
    Ok(())
}

/// ZMQ_CONFLATE keeps one *message part*, not one multipart message: libzmq documents it as
/// unsupported for multipart, and the frames below show why it cannot replace the drain.
#[test]
fn conflate_does_not_support_multipart() -> TestResult {
    let ctx = Context::new()?;
    let ep = format!("inproc://{}", unique("conflate"));
    let xpub = bind_xpub(&ctx, &ep, &ServerSecurity::Null, 1000)?;
    let s = ctx.socket(SocketType::Sub)?;
    s.set_int(ffi::ZMQ_CONFLATE, 1)?;
    s.subscribe(b"")?;
    s.connect(&ep)?;
    wait_sub_event(&xpub, SubEvent::Subscribe, "")?;

    let mut last = Vec::new();
    for seq in 0..10 {
        let parts = encode_frame("d/a/tf", &header(seq, 4), &[seq as f32; 4]);
        last = parts[2].clone();
        xpub.send_multipart(&parts, 0)?;
    }
    // inproc sends complete before the first receive, so the conflated slot is final here.
    let m = s.recv_timeout(TIMEOUT)?.ok_or("nothing received")?;
    println!(
        "conflate: 10 x 3-part messages sent; received {} part(s): {:?} bytes",
        m.frames.len(),
        m.frames.iter().map(Vec::len).collect::<Vec<_>>()
    );
    // Topic and header are gone; only the last part of the last message survives.
    assert_eq!(m.frames, vec![last]);
    assert!(decode_frame(&m.frames).is_err());
    assert!(s.try_recv()?.is_none());
    Ok(())
}
