//! A topic's latest slot and its delta chain on the data socket.
//!
//! One chain per topic, shared by every subscriber: the base of a delta is the last frame of
//! the topic handed to libzmq, so conflation in the slot never breaks the chain (a frame
//! replaced before it was sent was never a base). The base advances only on a successful
//! hand-off; after a refused send the frame stays in the slot and goes out next as a
//! keyframe. A keyframe goes every [`KEY_INTERVAL`], on a subscription, when no base exists
//! or the base does not fit (another `n`, grid, array set, epoch or static field), when the
//! delta is not smaller than the topic's last keyframe, and on `data.keyframe`.

use std::time::{Duration, Instant};

use ac2_proto::{CodedFrame, EncodeError, encode_delta, encode_key};
use ac2_zmq::{Error as ZmqError, Part, Socket};

/// Longest a topic goes without a keyframe. A receiver that lost a delta and could not ask
/// for a keyframe draws again within this, half the STALE threshold
/// (`ac2_client::data::STALE_AFTER`), so a single loss never dims a pane.
pub(crate) const KEY_INTERVAL: Duration = Duration::from_millis(500);

/// Shortest interval between two keyframes asked for by `data.keyframe`, whoever asks: a
/// burst of requests after a hiccup (every receiver of the topic at once, or one asking
/// again before its keyframe arrived) costs one keyframe.
pub(crate) const KEY_REQUEST_GAP: Duration = Duration::from_millis(250);

/// A topic's newest frame and its chain.
#[derive(Debug, Default)]
pub(crate) struct Slot {
    /// The newest frame not yet sent.
    pending: Option<CodedFrame>,
    /// The last frame handed to libzmq: the base of the next delta.
    base: Option<CodedFrame>,
    last_sent: Option<Instant>,
    keyed_at: Option<Instant>,
    /// Bytes of the topic's last keyframe: a delta at least this large goes as a keyframe.
    key_bytes: usize,
    /// The next frame goes as a keyframe.
    key_due: bool,
    asked_at: Option<Instant>,
}

/// What a send handed to libzmq.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    Key,
    Delta,
}

fn to_parts(v: Vec<Vec<u8>>) -> Result<Vec<Part>, ZmqError> {
    v.into_iter().map(Part::from_vec).collect()
}

fn bytes(v: &[Vec<u8>]) -> usize {
    v.iter().map(Vec::len).sum()
}

impl Slot {
    /// `f` replaces any frame still waiting.
    pub(crate) fn put(&mut self, f: CodedFrame) {
        self.pending = Some(f);
    }

    /// A frame is waiting.
    pub(crate) fn dirty(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn last_sent(&self) -> Option<Instant> {
        self.last_sent
    }

    /// `data.keyframe`: the next frame goes as a keyframe, unless one was asked for within
    /// [`KEY_REQUEST_GAP`].
    pub(crate) fn ask_key(&mut self, now: Instant) {
        if self
            .asked_at
            .is_none_or(|t| now.duration_since(t) >= KEY_REQUEST_GAP)
        {
            self.key_due = true;
            self.asked_at = Some(now);
        }
    }

    /// The pending frame as a delta on the base, or as a keyframe.
    fn form(&self, f: &CodedFrame, now: Instant) -> Result<(Vec<Vec<u8>>, Form), EncodeError> {
        let key_due = self.key_due
            || self
                .keyed_at
                .is_none_or(|t| now.duration_since(t) >= KEY_INTERVAL);
        if !key_due
            && let Some(base) = &self.base
            && let Some(d) = encode_delta(f, base)?
            && bytes(&d) < self.key_bytes
        {
            return Ok((d, Form::Delta));
        }
        Ok((encode_key(f)?, Form::Key))
    }

    /// Sends the pending frame, if any.
    pub(crate) fn send(&mut self, xpub: &Socket, now: Instant) {
        let Some(f) = self.pending.take() else {
            return;
        };
        let (parts, form) = match self.form(&f, now) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("{}: frame not encodable: {e}", f.topic);
                return;
            }
        };
        let len = bytes(&parts);
        self.last_sent = Some(now);
        match hand_off(xpub, parts) {
            Ok(()) => {
                if form == Form::Key {
                    self.keyed_at = Some(now);
                    self.key_bytes = len;
                    self.key_due = false;
                }
                self.base = Some(f);
            }
            Err(e) => {
                // Not handed off: the base stays, the frame waits for the next round, and
                // it goes as a keyframe.
                if !matches!(e, ZmqError::WouldBlock) {
                    tracing::warn!("{}: data send: {e}", f.topic);
                }
                self.pending = Some(f);
                self.key_due = true;
            }
        }
    }

    /// A new subscription: the newest frame now, as a keyframe, so the subscriber draws
    /// without waiting for the next one (every subscriber of the topic receives it).
    pub(crate) fn send_key_now(&mut self, xpub: &Socket, now: Instant) {
        self.key_due = true;
        if self.pending.is_some() {
            self.send(xpub, now);
            return;
        }
        let Some(base) = &self.base else {
            return;
        };
        let parts = match encode_key(base) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("{}: frame not encodable: {e}", base.topic);
                return;
            }
        };
        let len = bytes(&parts);
        if hand_off(xpub, parts).is_ok() {
            self.keyed_at = Some(now);
            self.key_bytes = len;
            self.key_due = false;
            self.last_sent = Some(now);
        }
    }
}

/// Hands one message to the XPUB without blocking.
fn hand_off(xpub: &Socket, parts: Vec<Vec<u8>>) -> Result<(), ZmqError> {
    xpub.try_send_parts(&to_parts(parts)?)
}
