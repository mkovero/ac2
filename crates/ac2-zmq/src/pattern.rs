//! Small reusable pieces of the ac2 socket design: the client-side latest-wins drain and the
//! publisher-side subscription interest set.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::Result;
use crate::socket::{Message, Socket};

/// The newest message of one topic found by [`drain_latest`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Latest {
    /// Sequence number the extractor returned for `message`.
    pub seq: u64,
    /// The message itself (first frame = topic).
    pub message: Message,
}

/// Result of one [`drain_latest`] pass.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Drained {
    /// Newest message per topic (first frame), by the extractor's sequence number.
    pub latest: BTreeMap<Vec<u8>, Latest>,
    /// Messages read in this pass, including superseded and malformed ones.
    pub read: usize,
    /// Messages the extractor rejected (dropped and counted, never fatal).
    pub malformed: usize,
    /// Lowest sequence number read: how far back the discarded backlog reached.
    pub oldest_seq: Option<u64>,
    /// Whether the queue was emptied. `false` means the pass stopped at its message limit
    /// while more was queued.
    pub emptied: bool,
}

/// Reads what is queued on `sub` without blocking (at most `limit` messages) and keeps only
/// the newest message per topic.
///
/// Run it before each render instead of reading frame by frame: a publisher drops the
/// *newest* messages for a stalled peer (its queue holds the start of the stall), so a backlog
/// must be discarded in one pass to get back to fresh data. `seq_of` returns a message's
/// sequence number, or `None` for a malformed message; for equal numbers the first message
/// read is kept. `limit` bounds the time spent when a publisher outpaces the reader.
pub fn drain_latest(
    sub: &Socket,
    limit: usize,
    mut seq_of: impl FnMut(&Message) -> Option<u64>,
) -> Result<Drained> {
    let mut d = Drained::default();
    while d.read < limit {
        let Some(m) = sub.try_recv()? else {
            d.emptied = true;
            return Ok(d);
        };
        d.read += 1;
        let Some(seq) = seq_of(&m) else {
            d.malformed += 1;
            continue;
        };
        d.oldest_seq = Some(d.oldest_seq.map_or(seq, |o| o.min(seq)));
        let topic = m.frames()[0].clone();
        match d.latest.get(&topic) {
            Some(kept) if kept.seq >= seq => {}
            _ => {
                d.latest.insert(topic, Latest { seq, message: m });
            }
        }
    }
    Ok(d)
}

/// A subscription change an XPUB reports.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SubscriptionEvent {
    /// A subscriber subscribed to this prefix (with `xpub_verbose`: every subscriber,
    /// including additional ones to a prefix that already had subscribers).
    Subscribe(Vec<u8>),
    /// The last subscriber of this prefix left (unsubscribed or disconnected).
    Unsubscribe(Vec<u8>),
}

impl SubscriptionEvent {
    /// XPUB delivers `[0x01 | 0x00][prefix]` as a single frame; anything else is `None`.
    pub fn parse(m: &Message) -> Option<Self> {
        let [f] = m.frames() else {
            return None;
        };
        match f.split_first()? {
            (1, prefix) => Some(Self::Subscribe(prefix.to_vec())),
            (0, prefix) => Some(Self::Unsubscribe(prefix.to_vec())),
            _ => None,
        }
    }

    /// The prefix concerned.
    pub fn prefix(&self) -> &[u8] {
        match self {
            Self::Subscribe(p) | Self::Unsubscribe(p) => p,
        }
    }

    /// Whether `topic` falls under this event's prefix (ZMQ prefix matching).
    pub fn covers(&self, topic: &[u8]) -> bool {
        topic.starts_with(self.prefix())
    }
}

/// Publisher-side interest set built from XPUB subscription events, with ZMQ's prefix
/// semantics. Use it to skip producing topics nobody wants and, on a new subscribe, to
/// re-send the latest message of every topic it [covers](SubscriptionEvent::covers) so late
/// joiners do not wait for the next one.
///
/// Expects `xpub_verbose` on: subscribe adds a prefix, the (non-verbose) unsubscribe XPUB
/// reports when a prefix's last subscriber leaves removes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubscriptionTracker {
    prefixes: BTreeSet<Vec<u8>>,
}

impl SubscriptionTracker {
    /// No interest.
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one XPUB message; returns the parsed event, or `None` if the message was not a
    /// subscription change (and the set is unchanged).
    pub fn apply(&mut self, m: &Message) -> Option<SubscriptionEvent> {
        let ev = SubscriptionEvent::parse(m)?;
        self.apply_event(&ev);
        Some(ev)
    }

    /// Applies an already parsed event.
    pub fn apply_event(&mut self, ev: &SubscriptionEvent) {
        match ev {
            SubscriptionEvent::Subscribe(p) => {
                self.prefixes.insert(p.clone());
            }
            SubscriptionEvent::Unsubscribe(p) => {
                self.prefixes.remove(p);
            }
        }
    }

    /// Whether any subscriber receives `topic`.
    pub fn wants(&self, topic: &[u8]) -> bool {
        // Every prefix of `topic` that is in the set sorts at or before `topic`, and the
        // candidates are few (one per subscribed prefix), so a scan is fine.
        self.prefixes.iter().any(|p| topic.starts_with(p))
    }

    /// The prefixes with at least one subscriber.
    pub fn prefixes(&self) -> impl Iterator<Item = &[u8]> {
        self.prefixes.iter().map(Vec::as_slice)
    }

    /// Whether nobody is subscribed to anything.
    pub fn is_empty(&self) -> bool {
        self.prefixes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_prefix_semantics() {
        let mut t = SubscriptionTracker::new();
        assert!(!t.wants(b"d/m1/tf"));
        t.apply_event(&SubscriptionEvent::Subscribe(b"d/m1".to_vec()));
        t.apply_event(&SubscriptionEvent::Subscribe(b"d/m1".to_vec()));
        assert!(t.wants(b"d/m1/tf"));
        assert!(!t.wants(b"d/m2/tf"));
        assert!(!t.wants(b"d/"));
        t.apply_event(&SubscriptionEvent::Subscribe(Vec::new()));
        assert!(t.wants(b"anything"));
        t.apply_event(&SubscriptionEvent::Unsubscribe(Vec::new()));
        t.apply_event(&SubscriptionEvent::Unsubscribe(b"d/m1".to_vec()));
        assert!(t.is_empty());
        assert!(SubscriptionEvent::Subscribe(b"d/".to_vec()).covers(b"d/m1/tf"));
    }
}
