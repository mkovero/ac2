//! Shared test helpers. Every wait is a poll with a deadline, never a fixed sleep.

#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ac2_zmq::{
    Context, CurveClient, Message, PollItem, Socket, SocketType, SubscriptionEvent, poll,
};

pub type TestResult = Result<(), Box<dyn std::error::Error>>;
pub type R<T> = Result<T, Box<dyn std::error::Error>>;

/// Generous: only reached when something is broken.
pub const TIMEOUT: Duration = Duration::from_secs(10);

static NEXT: AtomicU64 = AtomicU64::new(0);

/// Process-unique name for inproc endpoints and ipc files.
pub fn unique(name: &str) -> String {
    format!("{name}-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

#[derive(Clone, Copy, Debug)]
pub enum Transport {
    Tcp,
    #[cfg(unix)]
    Ipc,
    Inproc,
}

/// Bind endpoint for a fresh socket on `t`; `dir` holds ipc socket files.
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

pub fn dealer(ctx: &Context, ep: &str, curve: Option<&CurveClient>) -> R<Socket> {
    let s = ctx.socket(SocketType::Dealer)?;
    if let Some(c) = curve {
        s.set_curve_client(c)?;
    }
    s.connect(ep)?;
    Ok(s)
}

pub fn sub(ctx: &Context, ep: &str, curve: Option<&CurveClient>, topics: &[&str]) -> R<Socket> {
    let s = ctx.socket(SocketType::Sub)?;
    if let Some(c) = curve {
        s.set_curve_client(c)?;
    }
    for t in topics {
        s.subscribe(t.as_bytes())?;
    }
    s.connect(ep)?;
    Ok(s)
}

pub fn recv(s: &Socket) -> R<Message> {
    Ok(s.recv_timeout(TIMEOUT)?
        .ok_or("timed out waiting for a message")?)
}

/// Waits until XPUB reports `want`; returns every event seen on the way.
pub fn wait_sub_event(xpub: &Socket, want: &SubscriptionEvent) -> R<Vec<SubscriptionEvent>> {
    let deadline = Instant::now() + TIMEOUT;
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let m = xpub
            .recv_timeout(left)?
            .ok_or_else(|| format!("no {want:?} on XPUB; saw {seen:?}"))?;
        let ev = SubscriptionEvent::parse(&m).ok_or("XPUB delivered a non-subscription")?;
        let hit = &ev == want;
        seen.push(ev);
        if hit {
            return Ok(seen);
        }
    }
}

pub fn subscribed(topic: &str) -> SubscriptionEvent {
    SubscriptionEvent::Subscribe(topic.as_bytes().to_vec())
}

pub fn unsubscribed(topic: &str) -> SubscriptionEvent {
    SubscriptionEvent::Unsubscribe(topic.as_bytes().to_vec())
}

/// A data frame `[topic][seq u64 LE][payload]`.
pub fn frame(topic: &str, seq: u64, payload: &[u8]) -> [Vec<u8>; 3] {
    [
        topic.as_bytes().to_vec(),
        seq.to_le_bytes().to_vec(),
        payload.to_vec(),
    ]
}

/// Sequence number of a [`frame`]; `None` if malformed.
pub fn seq_of(m: &Message) -> Option<u64> {
    match m.frames() {
        [_, seq, _] => Some(u64::from_le_bytes(seq.as_slice().try_into().ok()?)),
        _ => None,
    }
}

/// What a [`CtrlServer`] saw: (ZAP user id, request id).
pub type Seen = Arc<Mutex<Vec<(Option<String>, u64)>>>;

/// A minimal ctrl server on its own I/O thread, shaped like the daemon's: the thread alone owns
/// the ROUTER and polls it together with an inproc PULL that other threads feed.
///
/// Requests are `[id u64 LE][command]`, replies `[id u64 LE][result]` (no delimiter frame).
/// Commands: `echo:<text>`, `whoami` (replies with the ZAP user id), `slow:<gate>` (parked
/// until the test opens that gate, without blocking anything else).
pub struct CtrlServer {
    ctx: Context,
    control: String,
    endpoint: String,
    seen: Seen,
    thread: Option<JoinHandle<()>>,
}

impl CtrlServer {
    /// Takes a ROUTER that is configured but not bound yet, and binds it to `bind`.
    pub fn start(ctx: &Context, router: Socket, bind: &str) -> R<Self> {
        router.set_router_mandatory(true)?;
        router.bind(bind)?;
        let endpoint = router.last_endpoint()?;
        let control = format!("inproc://{}", unique("ctrl-control"));
        let pull = ctx.socket(SocketType::Pull)?;
        pull.bind(&control)?;
        let seen = Seen::default();
        let seen2 = Arc::clone(&seen);
        let thread = std::thread::spawn(move || {
            let _ = serve(&router, &pull, &seen2);
        });
        Ok(Self {
            ctx: ctx.clone(),
            control,
            endpoint,
            seen,
            thread: Some(thread),
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn seen(&self) -> Vec<(Option<String>, u64)> {
        self.seen.lock().map(|v| v.clone()).unwrap_or_default()
    }

    fn control(&self, msg: &[u8]) -> TestResult {
        let push = self.ctx.socket(SocketType::Push)?;
        push.set_linger(Some(TIMEOUT))?;
        push.connect(&self.control)?;
        push.send(&[msg])?;
        Ok(())
    }

    pub fn open_gate(&self, gate: u8) -> TestResult {
        self.control(&[b'g', gate])
    }
}

impl Drop for CtrlServer {
    fn drop(&mut self) {
        if self.control(b"stop").is_ok()
            && let Some(t) = self.thread.take()
        {
            let _ = t.join();
        }
    }
}

fn serve(router: &Socket, control: &Socket, seen: &Seen) -> ac2_zmq::Result<()> {
    // Parked slow requests: (gate, routing id, request id).
    let mut parked: Vec<(u8, Vec<u8>, [u8; 8])> = Vec::new();
    loop {
        let mut items = [PollItem::readable(router), PollItem::readable(control)];
        poll(&mut items, None)?;
        if items[1].is_readable() {
            while let Some(m) = control.try_recv()? {
                match m.frames() {
                    [c] if c.as_slice() == b"stop" => return Ok(()),
                    [c] if c.len() == 2 && c[0] == b'g' => {
                        for (_, rid, id) in parked.extract_if(.., |(g, _, _)| *g == c[1]) {
                            let text = format!("slow {} done", c[1]);
                            router.send(&[rid.as_slice(), &id, text.as_bytes()])?;
                        }
                    }
                    _ => {}
                }
            }
        }
        if items[0].is_readable() {
            while let Some(m) = router.try_recv()? {
                let [rid, req] = m.frames() else { continue };
                let Some((id, cmd)) = req.split_first_chunk::<8>() else {
                    continue;
                };
                if let Ok(mut s) = seen.lock() {
                    s.push((m.user_id().map(str::to_owned), u64::from_le_bytes(*id)));
                }
                let reply: Vec<u8> = if let Some(text) = cmd.strip_prefix(b"echo:") {
                    text.to_vec()
                } else if cmd == b"whoami" {
                    m.user_id().unwrap_or_default().as_bytes().to_vec()
                } else if let Some([gate]) = cmd.strip_prefix(b"slow:") {
                    parked.push((*gate, rid.clone(), *id));
                    continue;
                } else {
                    b"error: unknown command".to_vec()
                };
                match router.send(&[rid.as_slice(), id, &reply]) {
                    // ROUTER_MANDATORY: the client vanished; nothing to answer.
                    Ok(()) | Err(ac2_zmq::Error::HostUnreachable) => {}
                    Err(e) => return Err(e),
                }
            }
        }
    }
}

pub fn send_req(d: &Socket, id: u64, cmd: &[u8]) -> TestResult {
    let mut req = id.to_le_bytes().to_vec();
    req.extend_from_slice(cmd);
    d.send(&[req])?;
    Ok(())
}

/// Next reply on `d`: (request id, result text).
pub fn recv_reply(d: &Socket) -> R<(u64, String)> {
    let m = recv(d)?;
    let [id, result] = m.frames() else {
        return Err("reply is not two frames".into());
    };
    let id: [u8; 8] = id.as_slice().try_into()?;
    Ok((u64::from_le_bytes(id), String::from_utf8(result.clone())?))
}

/// Polls a condition that becomes true through another thread's progress.
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
