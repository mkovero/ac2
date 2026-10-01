//! The client's I/O thread: the only owner of the ctrl DEALER and the sync SUB (`evt`, `ka`).
//!
//! libzmq sockets are single-threaded, so async callers never touch them. They hand encoded
//! requests to the thread through an inproc PUSH (behind a mutex) whose PULL end the thread
//! polls together with the DEALER and the SUB. Replies are routed to the waiting caller by
//! request id; `evt` and `ka` go to the sync task over an unbounded channel, because a lost
//! event would desynchronise the mirror (they are small and paced by the daemon).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ac2_proto::frame::KaMeta;
use ac2_proto::{
    CtrlError, DataMessage, Event, FrameData, FrameStamp, Reply, decode_data_message, decode_reply,
    peek_envelope,
};
use ac2_zmq::{Context, PollItem, Socket, SocketType, poll};
use tokio::sync::{mpsc, oneshot};

use crate::error::ClientError;

/// What the sync task receives from the data socket.
#[derive(Debug)]
pub(crate) enum SyncIn {
    /// A state event.
    Event(Event),
    /// A keepalive and when it was received.
    Ka {
        stamp: FrameStamp,
        meta: KaMeta,
        at: Instant,
        local_wall_ns: i128,
    },
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
}

impl Io {
    /// Starts the thread with sockets already connected.
    pub(crate) fn start(
        ctx: &Context,
        dealer: Socket,
        sync_sub: Socket,
        sync_tx: mpsc::UnboundedSender<SyncIn>,
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
        let worker = Worker {
            dealer,
            sub: sync_sub,
            pull,
            pending: pending.clone(),
            sync_tx,
            malformed: malformed.clone(),
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
        })
    }

    /// Queues an encoded request for the DEALER.
    pub(crate) fn send(&self, bytes: &[u8]) -> Result<(), ClientError> {
        let push = self.push.lock().map_err(|_| ClientError::Closed)?;
        push.send(&[TAG_REQUEST, bytes]).map_err(|e| match e {
            ac2_zmq::Error::ContextTerminated => ClientError::Closed,
            e => ClientError::Zmq(e),
        })
    }

    /// Tells the thread to send what is queued and stop.
    pub(crate) fn quit(&self) {
        if let Ok(push) = self.push.lock() {
            let _ = push.send(&[TAG_QUIT]);
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
    sub: Socket,
    pull: Socket,
    pending: Pending,
    sync_tx: mpsc::UnboundedSender<SyncIn>,
    malformed: Arc<AtomicU64>,
}

impl Worker {
    fn run(self) {
        loop {
            let mut items = [
                PollItem::readable(&self.dealer),
                PollItem::readable(&self.sub),
                PollItem::readable(&self.pull),
            ];
            match poll(&mut items, Some(Duration::from_millis(200))) {
                Ok(_) => {}
                Err(ac2_zmq::Error::ContextTerminated) => return,
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
            }
            let (d, s, p) = (
                items[0].is_readable(),
                items[1].is_readable(),
                items[2].is_readable(),
            );
            if d && self.drain_dealer().is_err() {
                return;
            }
            if s && self.drain_sub().is_err() {
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

    fn drain_sub(&self) -> Result<(), ac2_zmq::Error> {
        while let Some(m) = self.sub.try_recv()? {
            let at = Instant::now();
            let local_wall_ns = wall_ns();
            let parts: Vec<&[u8]> = m.frames().iter().map(Vec::as_slice).collect();
            let msg = match decode_data_message(&parts) {
                Ok(DataMessage::Event(e)) => SyncIn::Event(e),
                Ok(DataMessage::Frame(f)) => match f.data {
                    FrameData::Ka(meta) => SyncIn::Ka {
                        stamp: f.stamp,
                        meta,
                        at,
                        local_wall_ns,
                    },
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
