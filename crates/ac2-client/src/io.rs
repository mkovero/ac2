//! The client's I/O thread: the only owner of the ctrl DEALER and the sync DEALER, the data
//! connection subscribed to `evt` and `ka`.
//!
//! libzmq sockets are single-threaded, so async callers never touch them. They hand encoded
//! requests to the thread through an inproc PUSH (behind a mutex) whose PULL end the thread
//! polls together with the two DEALERs and the sync DEALER's monitor. Replies are routed to
//! the waiting caller by request id; `evt` and `ka` go to the sync task over an unbounded
//! channel, because a lost event would desynchronise the mirror (they are small and paced by
//! the daemon). What the sync DEALER read is credited after every drain; a new handshake
//! (a reconnect is a new connection to the daemon) or a `rejoin` states its subscriptions
//! again, and `resync_required` (its events overflowed the daemon's bound) asks the sync
//! task for a new snapshot.
//!
//! The thread sleeps in `zmq_poll` without a timeout: until a socket is readable it has
//! nothing to do, and an idle client should not wake the CPU.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ac2_proto::flow::{frame_charge, outside_window};
use ac2_proto::frame::KaMeta;
use ac2_proto::{
    CtrlError, DataMessage, DataUp, Event, FlowNotice, FrameData, FrameStamp, Reply, Subscription,
    decode_data_message, decode_reply, peek_envelope,
};
use ac2_zmq::{Context, Monitor, MonitorEvent, PollItem, Socket, SocketType, poll};
use tokio::sync::{mpsc, oneshot};

use crate::data::send_up;
use crate::error::ClientError;

/// The sync DEALER, its monitor and what it subscribes to.
#[derive(Debug)]
pub(crate) struct SyncSocket {
    sock: Socket,
    monitor: Monitor,
    subs: Vec<Subscription>,
}

impl SyncSocket {
    /// Over `sock`, a DEALER connected to the data endpoint; subscribes it to `subs`.
    pub(crate) fn new(
        sock: Socket,
        monitor: Monitor,
        subs: &[Subscription],
    ) -> Result<Self, ClientError> {
        let s = Self {
            sock,
            monitor,
            subs: subs.to_vec(),
        };
        s.state()?;
        Ok(s)
    }

    /// Every subscription, to a daemon that knows nothing of this connection. It carries
    /// no frames, so the default window serves.
    fn state(&self) -> Result<(), ClientError> {
        for s in &self.subs {
            send_up(&self.sock, DataUp::Subscribe { sub: *s })?;
        }
        Ok(())
    }
}

/// What the sync task receives from the data socket.
#[derive(Debug)]
pub(crate) enum SyncIn {
    /// A state event.
    Event(Box<Event>),
    /// A keepalive and when it was received.
    Ka {
        stamp: FrameStamp,
        /// Boxed: it carries the daemon's health with its host name, larger than an event's
        /// box, and keepalives come only four times a second.
        meta: Box<KaMeta>,
        at: Instant,
        local_wall_ns: i128,
    },
    /// The daemon dropped events for this connection: a new snapshot is due.
    Resync,
    /// The daemon dropped this connection's data peer and asked it to rejoin: a stall, or
    /// the key's role changed, which `welcome` tells.
    Rejoined,
}

pub(crate) type ReplyTx = oneshot::Sender<Result<Reply, ClientError>>;
pub(crate) type Pending = Arc<Mutex<HashMap<u64, ReplyTx>>>;

const TAG_REQUEST: &[u8] = b"r";
const TAG_QUIT: &[u8] = b"q";

/// Local wall clock, Unix ns.
pub(crate) fn wall_ns() -> i128 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i128,
        Err(e) => -(e.duration().as_nanos() as i128),
    }
}

/// Handle to the I/O thread.
#[derive(Debug)]
pub(crate) struct Io {
    push: Mutex<Socket>,
    thread: Mutex<Option<JoinHandle<()>>>,
    pub(crate) pending: Pending,
    /// Ctrl or data messages the thread could not decode (dropped and counted).
    pub(crate) malformed: Arc<AtomicU64>,
    /// The ZAP status of the daemon's last refusal of this client's key (0: none since the
    /// last handshake that passed).
    refused: Arc<AtomicU32>,
    /// See [`Io::network_off`].
    network_off: Arc<AtomicBool>,
}

impl Io {
    /// Starts the thread with sockets already connected.
    pub(crate) fn start(
        ctx: &Context,
        (dealer, dealer_monitor): (Socket, Monitor),
        sync: SyncSocket,
        sync_tx: mpsc::UnboundedSender<SyncIn>,
        received: Arc<AtomicU64>,
    ) -> Result<Self, ClientError> {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let ep = format!(
            "inproc://ac2-client/wake/{}/{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        );
        let pull = ctx.socket(SocketType::Pull)?;
        pull.bind(&ep)?;
        let push = ctx.socket(SocketType::Push)?;
        push.connect(&ep)?;
        // Requests queued just before shutdown (lease release) must still leave.
        push.set_linger(Some(Duration::from_millis(500)))?;
        let pending: Pending = Arc::default();
        let malformed = Arc::new(AtomicU64::new(0));
        let refused = Arc::new(AtomicU32::new(0));
        let network_off = Arc::new(AtomicBool::new(false));
        let worker = Worker {
            refused: refused.clone(),
            network_off: network_off.clone(),
            dealer,
            dealer_monitor,
            sync,
            pull,
            pending: pending.clone(),
            sync_tx,
            malformed: malformed.clone(),
            received,
        };
        let thread = std::thread::Builder::new()
            .name("ac2-client-io".into())
            .spawn(move || worker.run())
            .map_err(|e| ClientError::Zmq(ac2_zmq::Error::Spawn(e.to_string())))?;
        Ok(Self {
            push: Mutex::new(push),
            thread: Mutex::new(Some(thread)),
            pending,
            malformed,
            refused,
            network_off,
        })
    }

    /// Whether the daemon said its network was turned off on the rig, and has not been
    /// heard from since: the connection's end is that, not a fault.
    pub(crate) fn network_off(&self) -> bool {
        self.network_off.load(Ordering::Acquire)
    }

    /// Whether the daemon refused this client's key at its last handshake on the ctrl
    /// socket, the one every request goes through.
    pub(crate) fn refused(&self) -> bool {
        self.refused.load(Ordering::Acquire) != 0
    }

    /// Queues an encoded request for the DEALER.
    pub(crate) fn send(&self, bytes: &[u8]) -> Result<(), ClientError> {
        let push = self.push.lock().map_err(|_| ClientError::Closed)?;
        // Never block: with the I/O thread gone the PUSH has no peer and a blocking send
        // would hang the caller (with the lock held).
        push.try_send(&[TAG_REQUEST, bytes]).map_err(|e| match e {
            ac2_zmq::Error::ContextTerminated | ac2_zmq::Error::WouldBlock => ClientError::Closed,
            e => ClientError::Zmq(e),
        })
    }

    /// Tells the thread to send what is queued and stop.
    pub(crate) fn quit(&self) {
        if let Ok(push) = self.push.lock() {
            // Non-blocking: a second quit after the thread has gone must not hang.
            let _ = push.try_send(&[TAG_QUIT]);
        }
    }
}

impl Drop for Io {
    fn drop(&mut self) {
        self.quit();
        let handle = self.thread.lock().ok().and_then(|mut t| t.take());
        if let Some(h) = handle {
            let _ = h.join();
        }
    }
}

struct Worker {
    dealer: Socket,
    /// The ctrl DEALER's monitor: how the daemon answered its handshakes.
    dealer_monitor: Monitor,
    sync: SyncSocket,
    pull: Socket,
    pending: Pending,
    sync_tx: mpsc::UnboundedSender<SyncIn>,
    malformed: Arc<AtomicU64>,
    /// Bytes received from the daemon ([`ClientConfig::received_bytes`](crate::ClientConfig)).
    received: Arc<AtomicU64>,
    /// See [`Io::refused`].
    refused: Arc<AtomicU32>,
    /// See [`Io::network_off`].
    network_off: Arc<AtomicBool>,
}

impl Worker {
    /// Drains the sync monitor: whether a handshake passed (a new connection to the
    /// daemon, which knows nothing of this one's subscriptions).
    fn sync_events(&self) -> bool {
        let mut passed = false;
        while let Ok(Some(ev)) = self.sync.monitor.try_recv() {
            passed |= ev.event == MonitorEvent::HandshakeSucceeded;
        }
        passed
    }

    /// Drains the ctrl monitor: the daemon's refusal of the key, kept until a handshake
    /// passes.
    fn ctrl_events(&self) {
        while let Ok(Some(ev)) = self.dealer_monitor.try_recv() {
            match ev.event {
                MonitorEvent::HandshakeSucceeded => self.refused.store(0, Ordering::Release),
                MonitorEvent::HandshakeFailedAuth { status } => {
                    self.refused.store(status.max(1), Ordering::Release);
                }
                _ => {}
            }
        }
    }

    fn count(&self, frames: &[Vec<u8>]) {
        let n: usize = frames.iter().map(Vec::len).sum();
        self.received.fetch_add(n as u64, Ordering::Relaxed);
    }

    fn run(self) {
        loop {
            let mut items = [
                PollItem::readable(&self.dealer),
                PollItem::readable(&self.sync.sock),
                PollItem::readable(&self.pull),
                PollItem::readable(self.sync.monitor.socket()),
                PollItem::readable(self.dealer_monitor.socket()),
            ];
            match poll(&mut items, None) {
                Ok(_) => {}
                Err(ac2_zmq::Error::ContextTerminated) => return,
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
            }
            let (d, s, p, m) = (
                items[0].is_readable(),
                items[1].is_readable(),
                items[2].is_readable(),
                items[3].is_readable(),
            );
            if items[4].is_readable() {
                self.ctrl_events();
            }
            if d && self.drain_dealer().is_err() {
                return;
            }
            if m && self.sync_events() {
                let _ = self.sync.state();
            }
            if s && self.drain_sync().is_err() {
                return;
            }
            if p {
                match self.drain_pull() {
                    Ok(true) => {}
                    Ok(false) | Err(_) => return,
                }
            }
        }
    }

    fn drain_dealer(&self) -> Result<(), ac2_zmq::Error> {
        while let Some(m) = self.dealer.try_recv()? {
            self.count(m.frames());
            // DEALER ↔ ROUTER without an empty delimiter: one frame per reply.
            let Some(body) = m.frames().last() else {
                continue;
            };
            self.route_reply(body);
        }
        Ok(())
    }

    fn route_reply(&self, body: &[u8]) {
        let Ok(env) = peek_envelope(body) else {
            self.malformed.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let Some(id) = env.id else {
            self.malformed.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let result = match decode_reply(body) {
            Ok(r) => Ok(r),
            Err(CtrlError::VersionMismatch { theirs, ours }) => Err(ClientError::VersionMismatch {
                daemon: theirs,
                client: ours,
            }),
            Err(CtrlError::MissingVersion) => Err(ClientError::VersionMismatch {
                daemon: 0,
                client: ac2_proto::PROTO_VERSION,
            }),
            Err(_) => {
                self.malformed.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };
        // A reply to a retried id arrives once per send; only the first finds a waiter.
        let tx = self.pending.lock().ok().and_then(|mut p| p.remove(&id.0));
        if let Some(tx) = tx {
            let _ = tx.send(result);
        }
    }

    fn drain_sync(&self) -> Result<(), ac2_zmq::Error> {
        let (mut bytes, mut events) = (0u32, 0u32);
        while let Some(m) = self.sync.sock.try_recv()? {
            self.count(m.frames());
            let at = Instant::now();
            let local_wall_ns = wall_ns();
            let parts: Vec<&[u8]> = m.frames().iter().map(Vec::as_slice).collect();
            match parts.first() {
                Some(t) if outside_window(t) => events = events.saturating_add(1),
                _ => {
                    let n: usize = parts.iter().map(|p| p.len()).sum();
                    bytes = bytes.saturating_add(frame_charge(n));
                }
            }
            let msg = match decode_data_message(&parts) {
                Ok(DataMessage::Flow(FlowNotice::Rejoin)) => {
                    let _ = self.sync.state();
                    SyncIn::Rejoined
                }
                Ok(DataMessage::Flow(FlowNotice::ResyncRequired)) => SyncIn::Resync,
                Ok(DataMessage::Flow(FlowNotice::NetworkOff)) => {
                    self.network_off.store(true, Ordering::Release);
                    continue;
                }
                Ok(DataMessage::Event(e)) => SyncIn::Event(Box::new(e)),
                Ok(DataMessage::Frame(f)) => match f.data {
                    FrameData::Ka(meta) => {
                        // Heard from again: the network is back on.
                        self.network_off.store(false, Ordering::Release);
                        SyncIn::Ka {
                            stamp: f.stamp,
                            meta: Box::new(meta),
                            at,
                            local_wall_ns,
                        }
                    }
                    // Only `evt` and `ka` are subscribed on this socket.
                    _ => continue,
                },
                Err(_) => {
                    self.malformed.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
            };
            // The sync task is gone only while the client shuts down.
            let _ = self.sync_tx.send(msg);
        }
        if bytes > 0 || events > 0 {
            // A full queue means the connection is down; the next one is a new peer.
            let _ = send_up(&self.sync.sock, DataUp::Credit { bytes, events });
        }
        Ok(())
    }

    /// Returns `Ok(false)` on quit.
    fn drain_pull(&self) -> Result<bool, ac2_zmq::Error> {
        while let Some(m) = self.pull.try_recv()? {
            match m.frames() {
                [tag, body] if tag.as_slice() == TAG_REQUEST => {
                    // Non-blocking: with the daemon away the DEALER queue may be full; the
                    // caller's deadline then resends the same id.
                    match self.dealer.try_send(&[body]) {
                        Ok(()) | Err(ac2_zmq::Error::WouldBlock) => {}
                        Err(e) => return Err(e),
                    }
                }
                [tag] if tag.as_slice() == TAG_QUIT => return Ok(false),
                _ => {}
            }
        }
        Ok(true)
    }
}
