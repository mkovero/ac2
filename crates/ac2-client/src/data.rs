//! Data subscriptions and the drain-based latest-frame view (Q2).
//!
//! The data SUB is never read frame by frame. [`DataState::drain`] reads everything queued
//! (until EAGAIN, bounded), keeps the highest `seq` per topic, counts malformed frames and
//! discards frames of older session epochs or another daemon incarnation. Because a
//! publisher drops the *newest* messages for a stalled subscriber, only a drain gets back to
//! fresh data in one pass.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ac2_proto::units::{DaemonIncarnation, SessionEpoch};
use ac2_proto::{Frame, GridId, Topic, decode_frame};
use ac2_zmq::{Socket, drain_latest};

use crate::error::ClientError;
use crate::mirror::MirrorView;

/// No new frame on a topic for this long, or a frame older than this: STALE.
pub const STALE_AFTER: Duration = Duration::from_secs(1);
/// Most messages read by one drain; bounds the time spent when the publisher outpaces us.
pub const DRAIN_LIMIT: usize = 4096;

#[derive(Debug, Clone)]
struct Kept {
    frame: Arc<Frame>,
    /// When this seq first arrived.
    received: Instant,
}

/// Owner of the data SUB.
#[derive(Debug)]
pub(crate) struct DataState {
    sub: Socket,
    prefixes: BTreeMap<Vec<u8>, usize>,
    kept: HashMap<Topic, Kept>,
    malformed: u64,
    discarded: u64,
}

/// The newest frame of one topic.
#[derive(Debug, Clone)]
pub struct TopicFrame {
    /// Topic.
    pub topic: Topic,
    /// Frame.
    pub frame: Arc<Frame>,
    /// When it arrived (local monotonic clock).
    pub received: Instant,
    /// Time since a frame with a new `seq` arrived on this topic.
    pub since_new: Duration,
    /// Frame age, `now_local + offset − capture_wall_ns`, seconds; `None` until a keepalive
    /// gave a clock offset.
    pub age: Option<f64>,
    /// STALE (Q2): no new frame for 1 s, age above 1 s, or the daemon not responding.
    pub stale: bool,
}

/// Result of one drain.
#[derive(Debug, Clone, Default)]
pub struct Latest {
    /// Newest frame per topic, by topic text.
    pub frames: BTreeMap<String, TopicFrame>,
    /// Messages read by this drain (including superseded and discarded ones).
    pub read: usize,
    /// Malformed frames dropped since the client started.
    pub malformed_total: u64,
    /// Frames of an older epoch or another incarnation discarded since the client started.
    pub discarded_total: u64,
    /// The daemon's keepalives arrive.
    pub responding: bool,
}

impl Latest {
    /// The frame of `topic`.
    pub fn get(&self, topic: &Topic) -> Option<&TopicFrame> {
        self.frames.get(&topic.to_string())
    }

    /// Every grid referenced by a kept frame.
    pub fn grid_ids(&self) -> BTreeSet<GridId> {
        self.frames
            .values()
            .filter_map(|f| f.frame.stamp.grid_id)
            .collect()
    }
}

fn current(f: &Frame, inc: Option<DaemonIncarnation>, epoch: Option<SessionEpoch>) -> bool {
    inc.is_none_or(|i| f.stamp.daemon_incarnation == i)
        && epoch.is_none_or(|e| f.stamp.session_epoch >= e)
}

/// Frame age in seconds at local wall time `now_wall_ns`, given the `daemon − local` offset.
pub fn frame_age(f: &Frame, now_wall_ns: i128, offset_ns: Option<i128>) -> Option<f64> {
    let off = offset_ns?;
    let age_ns = now_wall_ns + off - i128::from(f.stamp.capture_wall_ns.0);
    Some(age_ns as f64 / 1e9)
}

impl DataState {
    pub(crate) fn new(sub: Socket) -> Self {
        Self {
            sub,
            prefixes: BTreeMap::new(),
            kept: HashMap::new(),
            malformed: 0,
            discarded: 0,
        }
    }

    pub(crate) fn subscribe(&mut self, prefix: &[u8]) -> Result<(), ClientError> {
        let n = self.prefixes.entry(prefix.to_vec()).or_insert(0);
        if *n == 0 {
            self.sub.subscribe(prefix)?;
        }
        *n += 1;
        Ok(())
    }

    pub(crate) fn unsubscribe(&mut self, prefix: &[u8]) -> Result<(), ClientError> {
        let Some(n) = self.prefixes.get_mut(prefix) else {
            return Err(ClientError::Invalid(format!(
                "not subscribed to {:?}",
                String::from_utf8_lossy(prefix)
            )));
        };
        *n -= 1;
        if *n == 0 {
            self.prefixes.remove(prefix);
            self.sub.unsubscribe(prefix)?;
            let still = |t: &Topic| {
                let b = t.to_bytes();
                self.prefixes.keys().any(|p| b.starts_with(p))
            };
            let keep: Vec<Topic> = self.kept.keys().copied().filter(still).collect();
            self.kept.retain(|t, _| keep.contains(t));
        }
        Ok(())
    }

    pub(crate) fn drain(
        &mut self,
        view: &MirrorView,
        now: Instant,
        now_wall_ns: i128,
    ) -> Result<Latest, ClientError> {
        let inc = view.incarnation;
        let epoch = view.session_epoch;
        let mut fresh: HashMap<Topic, Frame> = HashMap::new();
        let mut discarded = 0u64;
        let drained = drain_latest(&self.sub, DRAIN_LIMIT, |m| {
            let parts: Vec<&[u8]> = m.frames().iter().map(Vec::as_slice).collect();
            let f = decode_frame(&parts).ok()?;
            let seq = f.stamp.seq;
            if !current(&f, inc, epoch) {
                discarded += 1;
                return Some(seq);
            }
            let topic = f.topic();
            match fresh.get(&topic) {
                Some(k) if k.stamp.seq >= seq => {}
                _ => {
                    fresh.insert(topic, f);
                }
            }
            Some(seq)
        })?;
        self.malformed += drained.malformed as u64;
        self.discarded += discarded;

        // Kept frames from an older epoch or another incarnation go too.
        let before = self.kept.len();
        self.kept.retain(|_, k| current(&k.frame, inc, epoch));
        self.discarded += (before - self.kept.len()) as u64;

        for (topic, f) in fresh {
            let newer = match self.kept.get(&topic) {
                Some(k) => {
                    k.frame.stamp.daemon_incarnation != f.stamp.daemon_incarnation
                        || k.frame.stamp.session_epoch != f.stamp.session_epoch
                        || f.stamp.seq > k.frame.stamp.seq
                }
                None => true,
            };
            if newer {
                self.kept.insert(
                    topic,
                    Kept {
                        frame: Arc::new(f),
                        received: now,
                    },
                );
            }
        }

        let responding = view.responding(now);
        let frames = self
            .kept
            .iter()
            .map(|(topic, k)| {
                let since_new = now.saturating_duration_since(k.received);
                let age = frame_age(&k.frame, now_wall_ns, view.clock_offset_ns);
                let stale = !responding
                    || since_new > STALE_AFTER
                    || age.is_some_and(|a| a > STALE_AFTER.as_secs_f64());
                (
                    topic.to_string(),
                    TopicFrame {
                        topic: *topic,
                        frame: k.frame.clone(),
                        received: k.received,
                        since_new,
                        age,
                        stale,
                    },
                )
            })
            .collect();
        Ok(Latest {
            frames,
            read: drained.read,
            malformed_total: self.malformed,
            discarded_total: self.discarded,
            responding,
        })
    }
}
