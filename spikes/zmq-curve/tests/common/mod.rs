//! Shared test helpers. Every wait is a poll with a deadline, never a fixed sleep.

#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use spike_zmq_curve::ctrl::{ClientSecurity, unique};
use spike_zmq_curve::data::SubEvent;
use spike_zmq_curve::ffi;
use spike_zmq_curve::proto::{
    Cmd, FrameHeader, FrameKind, PROTO_VERSION, Reply, Request, decode_reply, encode_request,
};
use spike_zmq_curve::zmq::{Context, Socket, SocketType};

pub type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Generous: only reached when something is broken.
pub const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug)]
pub enum Transport {
    Tcp,
    #[cfg(unix)]
    Ipc,
    Inproc,
}

/// Bind endpoint for a fresh socket on `t`. `dir` holds ipc socket files.
pub fn bind_endpoint(t: Transport, dir: &std::path::Path, name: &str) -> String {
    match t {
        Transport::Tcp => "tcp://127.0.0.1:*".to_owned(),
        #[cfg(unix)]
        Transport::Ipc => format!("ipc://{}", dir.join(unique(name)).display()),
        Transport::Inproc => {
            let _ = dir;
            format!("inproc://{}", unique(name))
        }
    }
}

pub fn dealer(
    ctx: &Context,
    ep: &str,
    sec: &ClientSecurity,
) -> Result<Socket, Box<dyn std::error::Error>> {
    let s = ctx.socket(SocketType::Dealer)?;
    spike_zmq_curve::ctrl::apply_client(&s, sec)?;
    s.connect(ep)?;
    Ok(s)
}

pub fn sub(
    ctx: &Context,
    ep: &str,
    sec: &ClientSecurity,
    topics: &[&str],
) -> Result<Socket, Box<dyn std::error::Error>> {
    let s = ctx.socket(SocketType::Sub)?;
    spike_zmq_curve::ctrl::apply_client(&s, sec)?;
    for t in topics {
        s.subscribe(t.as_bytes())?;
    }
    s.connect(ep)?;
    Ok(s)
}

pub fn send_req(d: &Socket, id: u64, cmd: Cmd) -> TestResult {
    d.send_multipart(
        &[encode_request(&Request {
            v: PROTO_VERSION,
            id,
            cmd,
        })],
        0,
    )?;
    Ok(())
}

pub fn recv_reply(d: &Socket) -> Result<Reply, Box<dyn std::error::Error>> {
    let m = d
        .recv_timeout(TIMEOUT)?
        .ok_or("timed out waiting for reply")?;
    let [payload] = m.frames.as_slice() else {
        return Err("reply is not one frame".into());
    };
    Ok(decode_reply(payload)?)
}

/// Wait until XPUB reports `want` for `topic`; returns every event seen on the way.
pub fn wait_sub_event(
    xpub: &Socket,
    want: fn(Vec<u8>) -> SubEvent,
    topic: &str,
) -> Result<Vec<SubEvent>, Box<dyn std::error::Error>> {
    let target = want(topic.as_bytes().to_vec());
    let deadline = Instant::now() + TIMEOUT;
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let m = xpub
            .recv_timeout(left)?
            .ok_or_else(|| format!("no {target:?} on XPUB"))?;
        let ev = SubEvent::parse(&m).ok_or("XPUB delivered a non-subscription message")?;
        let hit = ev == target;
        seen.push(ev);
        if hit {
            return Ok(seen);
        }
    }
}

pub fn header(seq: u64, n: u32) -> FrameHeader {
    FrameHeader {
        seq,
        audio_sample: seq * 800,
        daemon_incarnation: 1,
        config_rev: 1,
        grid_id: 1,
        kind: FrameKind::Tf,
        n,
        capture_wall_ns: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)),
    }
}

/// 6 KB TF-sized payload: 1536 f32 (≈ 3 arrays × 512 columns).
pub const TF_N: u32 = 1536;

pub fn tf_values(seq: u64) -> Vec<f32> {
    (0..TF_N).map(|i| seq as f32 + i as f32 * 0.5).collect()
}

/// Small send/receive buffers so TCP backlog is visible at a few frames rather than MBs.
pub fn small_kernel_buffers(s: &Socket) -> TestResult {
    s.set_int(ffi::ZMQ_SNDBUF, 16 * 1024)?;
    s.set_int(ffi::ZMQ_RCVBUF, 16 * 1024)?;
    Ok(())
}

/// Poll a condition that becomes true through another thread's progress.
pub fn wait_until(mut f: impl FnMut() -> bool) -> TestResult {
    let deadline = Instant::now() + TIMEOUT;
    while !f() {
        if Instant::now() > deadline {
            return Err("condition not reached".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}
