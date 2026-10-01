//! The internal pipe into the I/O thread: every thread that has something for a client
//! owns one PUSH socket connected to the I/O thread's inproc PULL. Messages are multipart
//! with a one-byte tag first.
//!
//! Ordering: messages from one PUSH socket arrive in order. Control sends events, replies
//! and keepalives through the same socket, so an event always reaches the data socket before
//! the reply to the command that caused it and before a keepalive carrying its rev.

use ac2_zmq::{Context, Error as ZmqError, Socket, SocketType};

/// Reply: `[R][routing id][reply]`.
pub(crate) const TAG_REPLY: u8 = b'R';
/// State event: `[E][event body]` → data socket `[evt][body]`.
pub(crate) const TAG_EVENT: u8 = b'E';
/// Frame: `[F][topic][header][arrays…]` → latest slot of its topic.
pub(crate) const TAG_FRAME: u8 = b'F';
/// Keepalive frame, sent at once: `[K][topic][header]`.
pub(crate) const TAG_KA: u8 = b'K';
/// Forget latest slots under a prefix: `[C][prefix]`.
pub(crate) const TAG_CLEAR: u8 = b'C';
/// Stop the I/O thread: `[X]`.
pub(crate) const TAG_STOP: u8 = b'X';

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

    pub(crate) fn ka(&self, parts: &[Vec<u8>]) {
        let v: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
        self.send(TAG_KA, &v);
    }

    pub(crate) fn clear(&self, prefix: &[u8]) {
        self.send(TAG_CLEAR, &[prefix]);
    }

    pub(crate) fn stop(&self) {
        self.send(TAG_STOP, &[]);
    }

    /// Queues a frame without blocking. A full pipe drops it: frames are latest-wins and the
    /// next one supersedes it.
    pub(crate) fn frame(&self, parts: &[Vec<u8>]) -> bool {
        let t = [TAG_FRAME];
        let mut v: Vec<&[u8]> = Vec::with_capacity(parts.len() + 1);
        v.push(&t);
        v.extend(parts.iter().map(Vec::as_slice));
        match self.sock.try_send(&v) {
            Ok(()) => true,
            Err(ZmqError::WouldBlock) => false,
            Err(e) => {
                tracing::warn!("internal pipe frame send failed: {e}");
                false
            }
        }
    }
}
