//! The I/O thread: owns the two client-facing ROUTERs (ctrl and data) and the inproc PULL
//! that the rest of the daemon feeds, and polls the three with `zmq_poll`. No other thread
//! touches a client-facing socket.
//!
//! - ctrl ROUTER: each request is handed to the control thread with its routing id and ZAP
//!   user id; replies come back through the PULL.
//! - data ROUTER: one peer per client connection ([`crate::peers`]): its subscriptions
//!   update the shared [`Interest`], its credit paces what it is sent. Frames come from the
//!   jobs as codes ([`crate::outbox::FrameTx`]) into per-topic latest slots; each peer is
//!   sent the newest frame of each of its topics in turn, as its credit allows and each at
//!   most at the publish rate (bounded freshness, Q2), as a keyframe or a delta on its own
//!   chain ([`crate::chain`]). Events and keepalives go to every peer at once, outside
//!   credit.
//! - Network mode: the ZAP handler's status is checked every iteration; if it has exited,
//!   both network sockets are closed (a CURVE server without a handler fails open) and the
//!   daemon is told to shut down.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ac2_proto::Topic;
use ac2_zmq::{Error as ZmqError, Message, Part, PollItem, SecureContext, Socket, poll};

use crate::chain::Slot;
use crate::control::{ControlMsg, CurvePeer};
use crate::outbox::{
    FrameRx, TAG_AUDIO, TAG_CLEAR, TAG_EVENT, TAG_FRAME, TAG_KA, TAG_REPLY, TAG_REVOKE, TAG_STOP,
};
use crate::peers::{Outside, Peers};

/// Topics with at least one subscriber, shared with jobs that compute optional derivations
/// only when someone receives them: the subscription prefixes of every data peer, counted
/// per peer holding them.
#[derive(Debug, Default)]
pub(crate) struct Interest {
    prefixes: Mutex<HashMap<Vec<u8>, usize>>,
    /// Advances whenever the set of prefixes changes, so a thread asking every block keeps
    /// its answers and asks again only after a change.
    generation: AtomicU64,
}

impl Interest {
    fn prefixes(&self) -> MutexGuard<'_, HashMap<Vec<u8>, usize>> {
        self.prefixes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn wants(&self, topic: &[u8]) -> bool {
        self.prefixes().keys().any(|p| topic.starts_with(p))
    }

    /// Whether any subscription lies under `prefix` (`audio/`: is any channel streamed).
    pub(crate) fn any_under(&self, prefix: &[u8]) -> bool {
        self.prefixes().keys().any(|p| p.starts_with(prefix))
    }

    /// The count of changes to the set of prefixes. It advances once the set has changed,
    /// so an answer taken after reading the count is never older than the count: whoever
    /// keeps answers per count asks again at the first block after a subscription.
    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub(crate) fn add(&self, prefix: &[u8]) {
        let mut m = self.prefixes();
        let n = m.entry(prefix.to_vec()).or_default();
        *n += 1;
        if *n == 1 {
            self.generation.fetch_add(1, Ordering::Release);
        }
    }

    pub(crate) fn remove(&self, prefix: &[u8]) {
        let mut m = self.prefixes();
        if let Some(n) = m.get_mut(prefix) {
            *n -= 1;
            if *n == 0 {
                m.remove(prefix);
                self.generation.fetch_add(1, Ordering::Release);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn subscribe(&self, prefix: &[u8]) {
        self.add(prefix);
    }

    #[cfg(test)]
    pub(crate) fn unsubscribe(&self, prefix: &[u8]) {
        self.remove(prefix);
    }

    /// Peers holding `prefix`.
    #[cfg(test)]
    pub(crate) fn count(&self, prefix: &[u8]) -> usize {
        self.prefixes().get(prefix).copied().unwrap_or(0)
    }
}

/// The sockets the I/O thread owns; built (and bound) on the starting thread so bind errors
/// reach [`crate::Daemon::start`].
pub(crate) struct IoSockets {
    pub(crate) router: Socket,
    pub(crate) data: Socket,
    pub(crate) pull: Socket,
    /// Frames from the jobs.
    pub(crate) frames: FrameRx,
    /// Network mode: the context whose ZAP handler authenticates both sockets.
    pub(crate) secure: Option<SecureContext>,
}

/// Most messages taken from one socket per poll round, so no source starves the others.
const BATCH: usize = 256;

/// Longest poll in network mode, so a dead ZAP handler is noticed within it.
const ZAP_CHECK: Duration = Duration::from_millis(100);
/// Longest poll in local mode with nothing held back: every input wakes the poll anyway, and
/// a stalled peer is looked for this often.
const IDLE_POLL: Duration = Duration::from_secs(1);

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

/// The newest frame per topic, and the order peers are served them in.
#[derive(Debug, Default)]
struct Slots {
    map: HashMap<Topic, Slot>,
    order: Vec<Topic>,
    put: u64,
}

impl Slots {
    /// Every frame queued into its topic's slot, replacing any frame still waiting there.
    fn take(&mut self, frames: &FrameRx) {
        while let Ok(f) = frames.try_recv() {
            self.put += 1;
            let topic = f.topic;
            let slot = Slot {
                frame: Arc::new(f),
                put: self.put,
            };
            if self.map.insert(topic, slot).is_none() {
                self.order.push(topic);
            }
        }
    }

    fn clear(&mut self, prefix: &[u8]) {
        self.map.retain(|t, _| !t.to_bytes().starts_with(prefix));
        let map = &self.map;
        self.order.retain(|t| map.contains_key(t));
    }
}

fn run(s: IoSockets, to_control: &Sender<ControlMsg>, interest: &Interest, fps: u32) {
    let IoSockets {
        router,
        data,
        pull,
        frames,
        secure,
    } = s;
    let mut net: Option<(Socket, Socket)> = Some((router, data));
    let period = Duration::from_secs_f64(1.0 / f64::from(fps.max(1)));
    // Shortest interval between two frames of one topic to one peer. Publishers pace
    // themselves on a cadence (`crate::cadence`) that keeps the average rate by sending some
    // frames early and never sends two within half a period, so this only stops a publisher
    // that bursts. A frame held back here waits for a timer of its own, which the OS may
    // defer.
    let min_gap = period / 2;
    let mut slots = Slots::default();
    let mut peers = Peers::default();
    let mut due: Option<Instant> = None;
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
        let timeout = due
            .map_or(idle, |t| t.saturating_duration_since(now))
            .min(idle);

        let (r_ready, d_ready, p_ready) = {
            let mut items: Vec<PollItem<'_>> = Vec::with_capacity(3);
            items.push(PollItem::readable(&pull));
            if let Some((r, d)) = &net {
                items.push(PollItem::readable(r));
                items.push(PollItem::readable(d));
            }
            if let Err(e) = poll(&mut items, Some(timeout)) {
                tracing::error!("poll failed: {e}");
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            let p = items[0].is_readable();
            let r = items.get(1).is_some_and(PollItem::is_readable);
            let d = items.get(2).is_some_and(PollItem::is_readable);
            (r, d, p)
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

        // Frames first: a subscription that arrives with them is served the newest.
        slots.take(&frames);

        if d_ready && let Some((_, data)) = &net {
            let now = Instant::now();
            for _ in 0..BATCH {
                match data.try_recv() {
                    Ok(Some(m)) => {
                        let client = m
                            .user_id()
                            .zip(m.client_key())
                            .map(|(name, key)| CurvePeer {
                                name: name.to_owned(),
                                key,
                            });
                        // A CURVE peer exists only with a secure context; without one
                        // nothing was checked to hold it to.
                        let admission = |c: &CurvePeer| {
                            secure.as_ref().map_or(ac2_zmq::Admission::Revoked, |sc| {
                                sc.admission(&c.name, &c.key)
                            })
                        };
                        peers.request(
                            data,
                            client.as_ref(),
                            m.into_frames(),
                            interest,
                            now,
                            admission,
                        );
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
                let mut msg = match pull.try_recv_parts() {
                    Ok(Some(m)) => m,
                    Ok(None) => break,
                    Err(e) => {
                        tracing::warn!("internal pipe recv: {e}");
                        break;
                    }
                };
                if msg.is_empty() || msg[0].len() != 1 {
                    continue;
                }
                let tag = msg.remove(0)[0];
                match tag {
                    TAG_REPLY => {
                        // [routing id, reply]
                        if let Some((router, _)) = &net
                            && msg.len() == 2
                        {
                            match router.try_send_parts(&msg) {
                                Ok(()) => {}
                                Err(ZmqError::HostUnreachable) => {
                                    tracing::debug!("reply to a departed client dropped");
                                }
                                Err(e) => tracing::warn!("reply send: {e}"),
                            }
                        }
                    }
                    TAG_EVENT => {
                        if let (Some((_, data)), [body]) = (&net, msg.as_slice()) {
                            match (evt_topic.share(), body.share()) {
                                (Ok(t), Ok(b)) => {
                                    peers.outside(data, &[t, b], Outside::Event, interest)
                                }
                                (Err(e), _) | (_, Err(e)) => tracing::warn!("event not sent: {e}"),
                            }
                        }
                    }
                    TAG_KA => {
                        if let Some((_, data)) = &net
                            && msg.len() == 2
                        {
                            peers.outside(data, &msg, Outside::Ka, interest);
                        }
                    }
                    // The frames themselves wait in the channel, drained below.
                    TAG_FRAME => {}
                    TAG_AUDIO => {
                        if let Some((_, data)) = &net
                            && msg.len() == 3
                            && let Ok(topic) = Topic::parse(&msg[0])
                        {
                            peers.audio(data, topic, msg, interest);
                        }
                    }
                    TAG_REVOKE => {
                        if let [key] = msg.as_slice()
                            && let Ok(k) = <[u8; 32]>::try_from(&key[..])
                        {
                            peers.drop_key(&ac2_zmq::PublicKey::from_bytes(k), interest);
                        }
                    }
                    TAG_CLEAR => {
                        if let [prefix] = msg.as_slice() {
                            // Frames queued before the clear go first, as they were sent.
                            slots.take(&frames);
                            slots.clear(prefix);
                            peers.clear(prefix);
                        }
                    }
                    TAG_STOP => stop = true,
                    _ => {}
                }
            }
        }

        // Every frame queued, wake-up or not: a wake-up dropped on a full pipe had others
        // before it.
        let now = Instant::now();
        slots.take(&frames);
        due = None;
        if let Some((_, data)) = &net {
            peers.sweep(now, interest);
            due = peers.serve(data, &slots.map, &slots.order, now, min_gap, interest);
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
    let peer = match (m.user_id(), m.client_key()) {
        (None, _) => None,
        (Some(name), Some(key)) => Some(CurvePeer {
            name: name.to_owned(),
            key,
        }),
        // Our ZAP handler attaches both or neither; without the key the connection cannot
        // be tied to the authorization it was granted, so it is not served.
        (Some(name), None) => {
            tracing::warn!("ctrl message from {name:?} without its client key dropped");
            return;
        }
    };
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
        peer,
        payload,
    });
}
