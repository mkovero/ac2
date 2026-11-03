//! A topic's newest frame and each peer's delta chain on it.
//!
//! The I/O thread keeps one [`Slot`] per topic: the newest frame, replaced as the jobs form
//! new ones. Every peer keeps a [`Chain`] per topic it receives: the frame last handed to
//! libzmq for that peer is the base of its next delta, so a peer that credit holds back
//! receives a delta over a longer distance, never a frame on a base it lacks, and a frame
//! replaced in the slot before it went was never anybody's base. The base advances only on
//! a successful hand-off; after a refused send the next frame goes as a keyframe. A keyframe
//! goes every [`KEY_INTERVAL`] per peer, on its subscription (no base yet), when the base
//! does not fit (another `n`, grid, array set, epoch or static field), when the delta is not
//! smaller than the peer's last keyframe of the topic, and on its `keyframe` request.
//!
//! Encodes are shared: peers in step hold the same base and so ask for the same (frame,
//! base) pair, which one send round encodes once ([`Encodes`]).

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ac2_proto::{CodedFrame, Topic, encode_delta, encode_key};
use ac2_zmq::Part;

/// Longest a topic goes without a keyframe to one peer. A receiver that lost a delta and
/// could not ask for a keyframe draws again within this, half the STALE threshold
/// (`ac2_client::data::STALE_AFTER`), so a single loss never dims a pane.
pub(crate) const KEY_INTERVAL: Duration = Duration::from_millis(500);

/// Shortest interval between two keyframes one peer's requests bring: a burst of requests
/// after a hiccup (one asking again before its keyframe arrived) costs one keyframe.
pub(crate) const KEY_REQUEST_GAP: Duration = Duration::from_millis(250);

/// A topic's newest frame.
#[derive(Debug)]
pub(crate) struct Slot {
    pub(crate) frame: Arc<CodedFrame>,
    /// Daemon-wide count of frames put, so a peer tells a new frame from the one it has.
    pub(crate) put: u64,
}

/// What one peer last received of a topic.
#[derive(Debug, Default)]
pub(crate) struct Chain {
    /// The last frame handed to libzmq for this peer, and its slot generation.
    base: Option<(u64, Arc<CodedFrame>)>,
    last_sent: Option<Instant>,
    keyed_at: Option<Instant>,
    /// Bytes of the last keyframe: a delta at least this large goes as a keyframe.
    key_bytes: usize,
    /// The next frame goes as a keyframe.
    key_due: bool,
    asked_at: Option<Instant>,
}

/// How a message was formed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Form {
    Key,
    Delta,
}

/// One encoded message: `[topic][body]`, shared between the peers it goes to.
#[derive(Debug)]
pub(crate) struct Encoded {
    pub(crate) parts: [Part; 2],
    /// Bytes of both parts.
    pub(crate) len: usize,
    pub(crate) form: Form,
}

type EncodeKey = (Topic, u64, Option<u64>);

/// The encodes of one send round, by (topic, frame generation, base generation or none for
/// a keyframe). `None`: no delta of that pair (it does not fit), or not encodable.
#[derive(Debug, Default)]
pub(crate) struct Encodes(HashMap<EncodeKey, Option<Rc<Encoded>>>);

fn encode(f: &CodedFrame, base: Option<&CodedFrame>) -> Option<Rc<Encoded>> {
    let (r, form) = match base {
        Some(b) => (encode_delta(f, b), Form::Delta),
        None => (encode_key(f).map(Some), Form::Key),
    };
    let parts = match r {
        Ok(Some(p)) => p,
        Ok(None) => return None,
        Err(e) => {
            tracing::error!("{}: frame not encodable: {e}", f.topic);
            return None;
        }
    };
    let len = parts.iter().map(Vec::len).sum();
    let mut it = parts.into_iter().map(Part::from_vec);
    match (it.next(), it.next(), it.next()) {
        (Some(Ok(t)), Some(Ok(b)), None) => Some(Rc::new(Encoded {
            parts: [t, b],
            len,
            form,
        })),
        _ => {
            tracing::error!("{}: frame not shaped as topic and body", f.topic);
            None
        }
    }
}

impl Encodes {
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    fn get(&mut self, slot: &Slot, base: Option<&(u64, Arc<CodedFrame>)>) -> Option<Rc<Encoded>> {
        let f = &slot.frame;
        self.0
            .entry((f.topic, slot.put, base.map(|b| b.0)))
            .or_insert_with(|| encode(f, base.map(|b| &*b.1)))
            .clone()
    }
}

impl Chain {
    /// The slot holds a frame this peer has not been handed.
    pub(crate) fn dirty(&self, slot: &Slot) -> bool {
        self.base.as_ref().is_none_or(|(g, _)| *g != slot.put)
    }

    /// When a frame may next go: the shortest interval between two frames of one topic.
    pub(crate) fn ready_at(&self, min_gap: Duration) -> Option<Instant> {
        self.last_sent.map(|t| t + min_gap)
    }

    /// A `keyframe` request: the next frame goes as a keyframe, unless one was asked for
    /// within [`KEY_REQUEST_GAP`].
    pub(crate) fn ask_key(&mut self, now: Instant) {
        if self
            .asked_at
            .is_none_or(|t| now.duration_since(t) >= KEY_REQUEST_GAP)
        {
            self.key_due = true;
            self.asked_at = Some(now);
        }
    }

    /// The slot's frame as this peer's next message: a delta on its base, or a keyframe.
    pub(crate) fn form(
        &self,
        slot: &Slot,
        now: Instant,
        encodes: &mut Encodes,
    ) -> Option<Rc<Encoded>> {
        let key_due = self.key_due
            || self
                .keyed_at
                .is_none_or(|t| now.duration_since(t) >= KEY_INTERVAL);
        if !key_due
            && let Some(base) = &self.base
            && let Some(d) = encodes.get(slot, Some(base))
            && d.len < self.key_bytes
        {
            return Some(d);
        }
        encodes.get(slot, None)
    }

    /// `e`, formed from `slot`, was handed to libzmq.
    pub(crate) fn sent(&mut self, slot: &Slot, e: &Encoded, now: Instant) {
        self.last_sent = Some(now);
        if e.form == Form::Key {
            self.keyed_at = Some(now);
            self.key_bytes = e.len;
            self.key_due = false;
        }
        self.base = Some((slot.put, Arc::clone(&slot.frame)));
    }

    /// A send was refused: the base stays, and the next frame goes as a keyframe.
    pub(crate) fn refused(&mut self) {
        self.key_due = true;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use ac2_proto::samples::chains;

    fn slots() -> Vec<Slot> {
        let (_, frames) = chains().into_iter().find(|(n, _)| *n == "tf").unwrap();
        frames
            .iter()
            .zip(1..)
            .map(|(f, put)| Slot {
                frame: Arc::new(CodedFrame::from_frame(f).unwrap()),
                put,
            })
            .collect()
    }

    fn applied(e: &Encoded, base: &CodedFrame) -> CodedFrame {
        let parts: Vec<&[u8]> = e.parts.iter().map(|p| &p[..]).collect();
        ac2_proto::decode_wire(&parts)
            .unwrap()
            .apply(Some(base))
            .unwrap()
    }

    /// Peers in step share one encode; a peer held back gets a delta over the longer
    /// distance, which applies on its own base to exactly the newest frame.
    #[test]
    fn peers_share_encodes_and_one_held_back_gets_its_own_delta() {
        let s = slots();
        let now = Instant::now();
        let mut enc = Encodes::default();
        let (mut a, mut b) = (Chain::default(), Chain::default());
        let k = a.form(&s[0], now, &mut enc).unwrap();
        assert_eq!(k.form, Form::Key);
        let k2 = b.form(&s[0], now, &mut enc).unwrap();
        assert!(Rc::ptr_eq(&k, &k2), "peers in step share the keyframe");
        a.sent(&s[0], &k, now);
        b.sent(&s[0], &k2, now);

        let later = now + Duration::from_millis(20);
        let d = a.form(&s[1], later, &mut enc).unwrap();
        assert_eq!(d.form, Form::Delta);
        a.sent(&s[1], &d, later);
        // b had no credit for frame 1: frame 2 goes to each on its own base.
        let da = a.form(&s[2], later, &mut enc).unwrap();
        let db = b.form(&s[2], later, &mut enc).unwrap();
        assert!(!Rc::ptr_eq(&da, &db));
        assert_eq!((da.form, db.form), (Form::Delta, Form::Delta));
        assert_eq!(applied(&da, &s[1].frame).codes, s[2].frame.codes);
        assert_eq!(applied(&db, &s[0].frame).codes, s[2].frame.codes);
        assert!(a.dirty(&s[2]) && !a.dirty(&s[1]));
    }

    /// A refused send keeps the base and turns the next frame into a keyframe; a request
    /// inside the gap is not honoured twice.
    #[test]
    fn refusal_and_requests_bring_keyframes() {
        let s = slots();
        let now = Instant::now();
        let mut enc = Encodes::default();
        let mut c = Chain::default();
        let k = c.form(&s[0], now, &mut enc).unwrap();
        c.sent(&s[0], &k, now);
        c.refused();
        let t = now + Duration::from_millis(10);
        assert_eq!(c.form(&s[1], t, &mut enc).unwrap().form, Form::Key);
        let k = c.form(&s[1], t, &mut enc).unwrap();
        c.sent(&s[1], &k, t);
        c.ask_key(t);
        let k = c.form(&s[2], t, &mut enc).unwrap();
        assert_eq!(k.form, Form::Key);
        c.sent(&s[2], &k, t);
        c.ask_key(t + Duration::from_millis(100));
        assert_eq!(
            c.form(&s[3], t + Duration::from_millis(100), &mut enc)
                .unwrap()
                .form,
            Form::Delta
        );
    }
}
