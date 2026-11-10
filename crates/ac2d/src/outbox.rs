//! The internal pipe into the I/O thread: every thread that has something for a client
//! owns one PUSH socket connected to the I/O thread's inproc PULL. Messages are multipart
//! with a one-byte tag first.
//!
//! Ordering: messages from one PUSH socket arrive in order. Control sends events, replies
//! and keepalives through the same socket, so an event always reaches the data socket before
//! the reply to the command that caused it and before a keepalive carrying its rev.
//!
//! Frames do not go through the pipe: a job hands its quantised codes and header fields
//! ([`CodedFrame`]) to the I/O thread through a [`FrameTx`] channel, and the pipe carries only
//! a wake-up. The I/O thread forms keyframe or delta against what it last sent to each peer,
//! so the encoding that depends on the wire's history happens where that history is.

use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};

use ac2_proto::CodedFrame;
use ac2_zmq::{Context, Error as ZmqError, Part, Socket, SocketType};

/// Reply: `[R][routing id][reply]`.
pub(crate) const TAG_REPLY: u8 = b'R';
/// State event: `[E][event body]` → data socket `[evt][body]` to every peer receiving it.
pub(crate) const TAG_EVENT: u8 = b'E';
/// Frames are waiting in the [`FrameTx`] channel: `[F]`.
pub(crate) const TAG_FRAME: u8 = b'F';
/// Keepalive frame (a keyframe), sent at once: `[K][topic][body]`.
pub(crate) const TAG_KA: u8 = b'K';
/// Forget latest slots under a prefix: `[C][prefix]`.
pub(crate) const TAG_CLEAR: u8 = b'C';
/// Stop the I/O thread: `[X]`.
pub(crate) const TAG_STOP: u8 = b'X';
/// `[tag][topic][header][samples]`: one block of a captured channel for the rigs taking it.
pub(crate) const TAG_AUDIO: u8 = b'A';
/// `[tag][key]`: a client key was revoked; its data connections end now.
pub(crate) const TAG_REVOKE: u8 = b'V';

/// One thread's PUSH end.
#[derive(Debug)]
pub(crate) struct Outbox {
    sock: Socket,
}

impl Outbox {
    /// A PUSH end with room for `hwm` queued messages. Job outboxes keep this small (frames
    /// are latest-wins; a backlog only adds latency); control's is large because events and
    /// replies are never dropped.
    pub(crate) fn connect(ctx: &Context, endpoint: &str, hwm: u32) -> Result<Self, ZmqError> {
        let sock = ctx.socket(SocketType::Push)?;
        sock.set_send_hwm(hwm)?;
        sock.connect(endpoint)?;
        Ok(Self { sock })
    }

    fn send(&self, tag: u8, parts: &[&[u8]]) {
        let mut v: Vec<&[u8]> = Vec::with_capacity(parts.len() + 1);
        let t = [tag];
        v.push(&t);
        v.extend_from_slice(parts);
        // The I/O thread drains the pipe continuously; a pipe that stays full for a second
        // means it has stopped, and blocking here would hang the sender with it.
        for _ in 0..1000 {
            match self.sock.try_send(&v) {
                Ok(()) => return,
                Err(ZmqError::WouldBlock) => {
                    std::thread::sleep(std::time::Duration::from_millis(1))
                }
                Err(e) => {
                    tracing::warn!("internal pipe send failed: {e}");
                    return;
                }
            }
        }
        tracing::warn!("internal pipe full for 1 s; message dropped");
    }

    pub(crate) fn reply(&self, routing_id: &[u8], reply: &[u8]) {
        self.send(TAG_REPLY, &[routing_id, reply]);
    }

    pub(crate) fn event(&self, body: &[u8]) {
        self.send(TAG_EVENT, &[body]);
    }

    pub(crate) fn ka(&self, parts: Vec<Vec<u8>>) {
        let Some(parts) = tagged(TAG_KA, parts) else {
            return;
        };
        for _ in 0..1000 {
            match self.sock.try_send_parts(&parts) {
                Ok(()) => return,
                Err(ZmqError::WouldBlock) => {
                    std::thread::sleep(std::time::Duration::from_millis(1))
                }
                Err(e) => {
                    tracing::warn!("internal pipe send failed: {e}");
                    return;
                }
            }
        }
        tracing::warn!("internal pipe full for 1 s; keepalive dropped");
    }

    /// An audio frame for the I/O thread, never blocking: the fan-out sends it, and a
    /// full pipe means the I/O thread stopped draining. `false` when not queued.
    pub(crate) fn audio(&self, parts: Vec<Vec<u8>>) -> bool {
        let Some(parts) = tagged(TAG_AUDIO, parts) else {
            return false;
        };
        self.sock.try_send_parts(&parts).is_ok()
    }

    pub(crate) fn revoke(&self, key: &[u8]) {
        self.send(TAG_REVOKE, &[key]);
    }

    pub(crate) fn clear(&self, prefix: &[u8]) {
        self.send(TAG_CLEAR, &[prefix]);
    }

    pub(crate) fn stop(&self) {
        self.send(TAG_STOP, &[]);
    }

    /// Wakes the I/O thread for frames queued on a [`FrameTx`]. Never blocks: a full pipe
    /// already holds wake-ups, and the I/O thread drains the whole channel on each.
    pub(crate) fn wake(&self) {
        match self.sock.try_send(&[&[TAG_FRAME][..]]) {
            Ok(()) | Err(ZmqError::WouldBlock) => {}
            Err(e) => tracing::warn!("internal pipe wake failed: {e}"),
        }
    }
}

/// Frames waiting for the I/O thread, across every job: a backlog only adds latency, as
/// each topic's newest frame supersedes the ones before.
const FRAME_QUEUE: usize = 1024;

/// A job's end of the frame channel to the I/O thread.
#[derive(Debug, Clone)]
pub(crate) struct FrameTx(SyncSender<CodedFrame>);

/// The I/O thread's end.
pub(crate) type FrameRx = Receiver<CodedFrame>;

/// A new frame channel.
pub(crate) fn frame_channel() -> (FrameTx, FrameRx) {
    let (tx, rx) = sync_channel(FRAME_QUEUE);
    (FrameTx(tx), rx)
}

impl FrameTx {
    /// Queues a frame without blocking; `false` if the channel is full or closed (frames
    /// are latest-wins, the next one supersedes it).
    pub(crate) fn send(&self, f: CodedFrame) -> bool {
        match self.0.try_send(f) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => false,
            Err(TrySendError::Disconnected(_)) => false,
        }
    }
}

/// `[tag, parts…]` as zero-copy parts: the encoded arrays (up to megabytes per frame) pass
/// through the pipe, the latest slot and the data socket in the buffers they were encoded
/// into, never copied.
fn tagged(tag: u8, parts: Vec<Vec<u8>>) -> Option<Vec<Part>> {
    let mut out = Vec::with_capacity(parts.len() + 1);
    let built = Part::copy_from(&[tag]).and_then(|t| {
        out.push(t);
        parts
            .into_iter()
            .try_for_each(|p| Part::from_vec(p).map(|p| out.push(p)))
    });
    match built {
        Ok(()) => Some(out),
        Err(e) => {
            tracing::warn!("internal pipe message not built: {e}");
            None
        }
    }
}
