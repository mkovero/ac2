//! The I/O thread: owns the ROUTER (ctrl) and XPUB (data) sockets and the inproc PULL that
//! the rest of the daemon feeds, and polls the three with `zmq_poll`. No other thread touches
//! a client-facing socket.
//!
//! - ROUTER: each request is handed to the control thread with its routing id and ZAP user
//!   id; replies come back through the PULL.
//! - XPUB: subscription changes update the shared [`Interest`] set; a new subscription gets
//!   the latest slot of every topic it covers at once (late joiners do not wait). Events and
//!   keepalives are sent as they arrive; frames go through per-topic latest slots, each sent
//!   at most at the publish rate (bounded freshness, Q2).
//! - Network mode: the ZAP handler's status is checked every iteration; if it has exited,
//!   both network sockets are closed (a CURVE server without a handler fails open) and the
//!   daemon is told to shut down.

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ac2_zmq::{
    Error as ZmqError, Message, Part, PollItem, SecureContext, Socket, SubscriptionTracker, poll,
};

use crate::control::ControlMsg;
use crate::outbox::{TAG_CLEAR, TAG_EVENT, TAG_FRAME, TAG_KA, TAG_REPLY, TAG_STOP};

/// Topics with at least one subscriber, shared with jobs that compute optional derivations
/// only when someone receives them.
#[derive(Debug, Default)]
pub(crate) struct Interest(Mutex<SubscriptionTracker>);

impl Interest {
    pub(crate) fn wants(&self, topic: &[u8]) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .wants(topic)
    }

    #[cfg(test)]
    pub(crate) fn subscribe(&self, prefix: &[u8]) {
        let ev = ac2_zmq::SubscriptionEvent::Subscribe(prefix.to_vec());
        self.update(|t| t.apply_event(&ev));
    }

    #[cfg(test)]
    pub(crate) fn unsubscribe(&self, prefix: &[u8]) {
        let ev = ac2_zmq::SubscriptionEvent::Unsubscribe(prefix.to_vec());
        self.update(|t| t.apply_event(&ev));
    }

    fn update(&self, f: impl FnOnce(&mut SubscriptionTracker)) -> usize {
        let mut t = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut t);
        t.prefixes().count()
    }
}

/// The sockets the I/O thread owns; built (and bound) on the starting thread so bind errors
/// reach [`crate::Daemon::start`].
pub(crate) struct IoSockets {
    pub(crate) router: Socket,
    pub(crate) xpub: Socket,
    pub(crate) pull: Socket,
    /// Network mode: the context whose ZAP handler authenticates both sockets.
    pub(crate) secure: Option<SecureContext>,
}

struct Slot {
    /// Shared with libzmq, never copied: every send of the slot adds a reference.
    parts: Vec<Part>,
    dirty: bool,
    last_sent: Option<Instant>,
}

/// Most messages taken from one socket per poll round, so no source starves the others.
const BATCH: usize = 256;

/// Longest poll in network mode, so a dead ZAP handler is noticed within it.
const ZAP_CHECK: Duration = Duration::from_millis(100);
/// Longest poll in local mode with nothing held back: every input wakes the poll anyway.
const IDLE_POLL: Duration = Duration::from_secs(1);

/// Per-peer send queue of the data socket, in messages.
///
/// The latest slots already conflate per topic, so the queue only has to hold one burst:
/// every slot a late joiner is sent at once, or one publish round of a full desk (about 40
/// topics) plus events and keepalives. Within such a burst libzmq does not yet see what the
/// peer has read, so a smaller queue would drop part of every round even for a fast local
/// peer. Anything above one burst only adds latency for a peer slower than the publish rate
/// (a Wi-Fi link): its queue stays full and every frame waits behind all the others, so the
/// standing delay is this many frames divided by what the link carries per second.
///
/// Set once before the socket binds. It is never changed on the live socket: libzmq applies
/// a new SNDHWM to existing pipes by sending commands to their peers, and such a command
/// racing with a disconnecting peer's pipe teardown is a use-after-free in libzmq 4.3.5's
/// I/O thread (it crashed the daemon at shutdown after a client with many subscriptions left).
pub(crate) const DATA_SNDHWM: u32 = 48;

pub(crate) fn spawn(
    sockets: IoSockets,
    to_control: Sender<ControlMsg>,
    interest: Arc<Interest>,
    fps: u32,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("ac2d-io".into())
        .spawn(move || run(sockets, &to_control, &interest, fps))
}

fn run(s: IoSockets, to_control: &Sender<ControlMsg>, interest: &Interest, fps: u32) {
    let IoSockets {
        router,
        xpub,
        pull,
        secure,
    } = s;
    let mut net: Option<(Socket, Socket)> = Some((router, xpub));
    let period = Duration::from_secs_f64(1.0 / f64::from(fps.max(1)));
    // Shortest interval between two frames of one topic. Publishers pace themselves on a
    // cadence (`crate::cadence`) that keeps the average rate by sending some frames early
    // and never sends two within half a period, so this only stops a publisher that
    // bursts. A frame held back here waits for a timer of its own, which the OS may defer.
    let min_gap = period / 2;
    let mut slots: HashMap<Vec<u8>, Slot> = HashMap::new();
    let evt_topic = match Part::copy_from(b"evt") {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("io thread cannot start: {e}");
            let _ = to_control.send(ControlMsg::Fatal(e.to_string()));
            return;
        }
    };
    loop {
        if let Some(sc) = &secure
            && net.is_some()
            && let Err(e) = sc.zap_status()
        {
            tracing::error!("{e}: closing network sockets and shutting down");
            net = None;
            let _ = to_control.send(ControlMsg::Fatal(e.to_string()));
        }
        let now = Instant::now();
        // With nothing held back the thread only needs to wake for socket traffic; in
        // network mode it still looks at the ZAP handler regularly, since a CURVE server
        // whose handler died must close promptly.
        let idle = if secure.is_some() {
            ZAP_CHECK
        } else {
            IDLE_POLL
        };
        let timeout = slots
            .values()
            .filter(|s| s.dirty)
            .filter_map(|s| {
                s.last_sent
                    .map(|t| (t + min_gap).saturating_duration_since(now))
            })
            .min()
            .unwrap_or(idle)
            .min(idle);

        let (r_ready, x_ready, p_ready) = {
            let mut items: Vec<PollItem<'_>> = Vec::with_capacity(3);
            items.push(PollItem::readable(&pull));
            if let Some((r, x)) = &net {
                items.push(PollItem::readable(r));
                items.push(PollItem::readable(x));
            }
            if let Err(e) = poll(&mut items, Some(timeout)) {
                tracing::error!("poll failed: {e}");
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            let p = items[0].is_readable();
            let r = items.get(1).is_some_and(PollItem::is_readable);
            let x = items.get(2).is_some_and(PollItem::is_readable);
            (r, x, p)
        };

        if r_ready && let Some((router, _)) = &net {
            for _ in 0..BATCH {
                match router.try_recv() {
                    Ok(Some(m)) => forward_request(m, to_control),
                    Ok(None) => break,
                    Err(e) => {
                        tracing::warn!("ctrl recv: {e}");
                        break;
                    }
                }
            }
        }

        if x_ready && let Some((_, xpub)) = &net {
            for _ in 0..BATCH {
                match xpub.try_recv() {
                    Ok(Some(m)) => {
                        let Some(ev) = ac2_zmq::SubscriptionEvent::parse(&m) else {
                            continue;
                        };
                        interest.update(|t| t.apply_event(&ev));
                        if let ac2_zmq::SubscriptionEvent::Subscribe(prefix) = &ev {
                            tracing::debug!("subscribe {:?}", String::from_utf8_lossy(prefix));
                            let now = Instant::now();
                            for (topic, slot) in &mut slots {
                                if topic.starts_with(prefix) {
                                    send_data(xpub, &slot.parts);
                                    slot.dirty = false;
                                    slot.last_sent = Some(now);
                                }
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        tracing::warn!("data recv: {e}");
                        break;
                    }
                }
            }
        }

        let mut stop = false;
        if p_ready {
            for _ in 0..(4 * BATCH) {
                let mut frames = match pull.try_recv_parts() {
                    Ok(Some(m)) => m,
                    Ok(None) => break,
                    Err(e) => {
                        tracing::warn!("internal pipe recv: {e}");
                        break;
                    }
                };
                if frames.is_empty() || frames[0].len() != 1 {
                    continue;
                }
                let tag = frames.remove(0)[0];
                match tag {
                    TAG_REPLY => {
                        // [routing id, reply]
                        if let Some((router, _)) = &net
                            && frames.len() == 2
                        {
                            match router.try_send_parts(&frames) {
                                Ok(()) => {}
                                Err(ZmqError::HostUnreachable) => {
                                    tracing::debug!("reply to a departed client dropped");
                                }
                                Err(e) => tracing::warn!("reply send: {e}"),
                            }
                        }
                    }
                    TAG_EVENT => {
                        if let (Some((_, xpub)), [body]) = (&net, frames.as_slice()) {
                            match (evt_topic.share(), body.share()) {
                                (Ok(t), Ok(b)) => send_data(xpub, &[t, b]),
                                (Err(e), _) | (_, Err(e)) => tracing::warn!("event not sent: {e}"),
                            }
                        }
                    }
                    TAG_KA => {
                        if let Some((_, xpub)) = &net {
                            send_data(xpub, &frames);
                        }
                    }
                    TAG_FRAME => {
                        if frames.len() < 2 {
                            continue;
                        }
                        let now = Instant::now();
                        // Topics repeat every frame; the key is allocated only the first
                        // time a topic is seen.
                        let slot = match slots.get_mut(&*frames[0]) {
                            Some(slot) => slot,
                            None => slots.entry(frames[0].to_vec()).or_insert(Slot {
                                parts: Vec::new(),
                                dirty: false,
                                last_sent: None,
                            }),
                        };
                        slot.parts = frames;
                        slot.dirty = true;
                        if let Some((_, xpub)) = &net
                            && slot
                                .last_sent
                                .is_none_or(|t| now.duration_since(t) >= min_gap)
                        {
                            send_data(xpub, &slot.parts);
                            slot.dirty = false;
                            slot.last_sent = Some(now);
                        }
                    }
                    TAG_CLEAR => {
                        if let [prefix] = frames.as_slice() {
                            slots.retain(|t, _| !t.starts_with(prefix));
                        }
                    }
                    TAG_STOP => stop = true,
                    _ => {}
                }
            }
        }

        let now = Instant::now();
        if let Some((_, xpub)) = &net {
            for slot in slots.values_mut() {
                if slot.dirty
                    && slot
                        .last_sent
                        .is_none_or(|t| now.duration_since(t) >= min_gap)
                {
                    send_data(xpub, &slot.parts);
                    slot.dirty = false;
                    slot.last_sent = Some(now);
                }
            }
        }

        if stop {
            break;
        }
    }
    // Sockets close with linger 0 when dropped here.
    drop(net);
    drop(pull);
    drop(secure);
    tracing::debug!("io thread stopped");
}

fn forward_request(m: Message, to_control: &Sender<ControlMsg>) {
    let user_id = m.user_id().map(str::to_owned);
    let mut f = m.into_frames();
    if f.len() != 2 {
        tracing::warn!(
            "ctrl message with {} frames dropped (expected routing id + 1)",
            f.len()
        );
        return;
    }
    let payload = f.pop().unwrap_or_default();
    let routing_id = f.pop().unwrap_or_default();
    let _ = to_control.send(ControlMsg::Request {
        routing_id,
        user_id,
        payload,
    });
}

fn send_data(xpub: &Socket, parts: &[Part]) {
    // XPUB never blocks: a peer at its high-water mark misses this message, and the next
    // newer one per topic supersedes it.
    match xpub.try_send_parts(parts) {
        Ok(()) | Err(ZmqError::WouldBlock) => {}
        Err(e) => tracing::warn!("data send: {e}"),
    }
}
