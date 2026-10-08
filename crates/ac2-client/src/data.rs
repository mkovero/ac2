//! Data subscriptions and the latest-frame view (Q2).
//!
//! The data SUB is never read frame by frame. [`DataState::drain`] reads everything queued
//! (until EAGAIN, bounded) without copying it and keeps the newest message per topic: frames
//! of one topic arrive in publish order over the one connection, so that is the last one
//! read. Only those are decoded; superseded messages never are. Malformed frames are
//! counted, frames of older session epochs or another daemon incarnation discarded. Because
//! a publisher drops the *newest* messages for a stalled subscriber, only a drain gets back
//! to fresh data in one pass.
//!
//! A consumer can sleep until data is queued instead of polling: see
//! [`crate::Client::data_changed`] and [`crate::Client::wait_data`].

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ac2_proto::frame::MAX_ARRAYS;
use ac2_proto::units::{DaemonIncarnation, SessionEpoch};
use ac2_proto::{Frame, GridId, Topic, decode_frame};
use ac2_zmq::{Part, Socket};

use crate::error::ClientError;
use crate::mirror::MirrorView;

/// No new frame on a topic for this long, or a frame older than this: STALE.
pub const STALE_AFTER: Duration = Duration::from_secs(1);
/// The same for the streams that come once a second by design
/// ([`ac2_proto::Stream::once_a_second`]: `leq`, `band_leq`): three missed.
pub const STALE_AFTER_LEQ: Duration = Duration::from_secs(3);

/// When a topic's newest frame counts as stale.
pub fn stale_after(t: &Topic) -> Duration {
    match t {
        Topic::Data { stream, .. } if stream.once_a_second() => STALE_AFTER_LEQ,
        _ => STALE_AFTER,
    }
}

/// Whether a topic's newest frame is STALE: the daemon not responding, or no new frame or a
/// frame age past the topic's threshold ([`stale_after`]).
pub fn is_stale(t: &Topic, responding: bool, since_new: Duration, age: Option<f64>) -> bool {
    let after = stale_after(t);
    !responding || since_new > after || age.is_some_and(|a| a > after.as_secs_f64())
}

/// Most messages read by one drain; bounds the time spent when the publisher outpaces us.
pub const DRAIN_LIMIT: usize = 4096;

#[derive(Debug, Clone)]
struct Kept {
    /// Topic text, made once per topic and shared with every [`Latest`] built from it.
    name: Arc<str>,
    frame: Arc<Frame>,
    /// When this seq first arrived.
    received: Instant,
}

/// Owner of the data SUB, the subscriptions and the decoded frames.
#[derive(Debug)]
pub(crate) struct DataState {
    sub: Socket,
    prefixes: BTreeMap<Vec<u8>, usize>,
    kept: HashMap<Topic, Kept>,
    malformed: u64,
    discarded: u64,
    /// Newest undecoded message per topic during a drain. Keys persist once seen, so a
    /// steady-state drain allocates nothing here.
    newest: HashMap<Box<[u8]>, Option<Vec<Part>>>,
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
    /// STALE (Q2): no new frame for 1 s, age above 1 s (3 s for the once-a-second `leq` and
    /// `band_leq`, [`stale_after`]), or the daemon not responding.
    pub stale: bool,
}

/// Result of one drain.
#[derive(Debug, Clone, Default)]
pub struct Latest {
    /// Newest frame per topic, by topic text.
    pub frames: BTreeMap<Arc<str>, TopicFrame>,
    /// Messages read by this drain (including superseded ones, which are never decoded).
    pub read: usize,
    /// Malformed frames dropped since the client started.
    pub malformed_total: u64,
    /// Frames of an older epoch or another incarnation discarded since the client started.
    pub discarded_total: u64,
    /// The daemon's keepalives arrive.
    pub responding: bool,
}

/// Topic text without a heap allocation (every topic is far shorter than the buffer).
struct TopicText {
    buf: [u8; 64],
    len: usize,
}

impl std::fmt::Write for TopicText {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let end = self.len + s.len();
        let dst = self.buf.get_mut(self.len..end).ok_or(std::fmt::Error)?;
        dst.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

impl Latest {
    /// The frame of `topic`.
    pub fn get(&self, topic: &Topic) -> Option<&TopicFrame> {
        let mut t = TopicText {
            buf: [0; 64],
            len: 0,
        };
        if write!(t, "{topic}").is_ok() {
            let s = std::str::from_utf8(&t.buf[..t.len]).ok()?;
            self.frames.get(s)
        } else {
            self.frames.get(topic.to_string().as_str())
        }
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

fn decode(parts: &[Part]) -> Option<Frame> {
    let mut refs: [&[u8]; 2 + MAX_ARRAYS] = [&[]; 2 + MAX_ARRAYS];
    if parts.len() > refs.len() {
        return None;
    }
    for (r, p) in refs.iter_mut().zip(parts) {
        *r = p;
    }
    decode_frame(&refs[..parts.len()]).ok()
}

impl DataState {
    pub(crate) fn new(sub: Socket) -> Self {
        Self {
            sub,
            prefixes: BTreeMap::new(),
            kept: HashMap::new(),
            malformed: 0,
            discarded: 0,
            newest: HashMap::new(),
        }
    }

    /// The data SUB, for waiting on it.
    pub(crate) fn socket(&self) -> &Socket {
        &self.sub
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
        if *n > 0 {
            return Ok(());
        }
        self.prefixes.remove(prefix);
        // The SUB filters what is still queued under the prefix as it is read.
        self.sub.unsubscribe(prefix)?;
        let prefixes = &self.prefixes;
        self.kept.retain(|t, _| subscribed(prefixes, &t.to_bytes()));
        self.newest.retain(|t, _| subscribed(prefixes, t));
        Ok(())
    }

    /// Drains the SUB into `out`: decodes the newest message per topic, and recomputes age
    /// and STALE of every kept frame at `now`. `out` is updated in place, so a caller that
    /// keeps it reuses its map and topic names.
    pub(crate) fn drain(
        &mut self,
        view: &MirrorView,
        now: Instant,
        now_wall_ns: i128,
        out: &mut Latest,
    ) -> Result<(), ClientError> {
        let inc = view.incarnation;
        let epoch = view.session_epoch;
        let mut read = 0;
        while read < DRAIN_LIMIT {
            let Some(parts) = self.sub.try_recv_parts()? else {
                break;
            };
            read += 1;
            let Some(topic) = parts.first() else { continue };
            match self.newest.get_mut(&**topic) {
                Some(slot) => *slot = Some(parts),
                None => {
                    self.newest.insert(Box::from(&**topic), Some(parts));
                }
            }
        }

        for parts in self.newest.values_mut().filter_map(Option::take) {
            let Some(f) = decode(&parts) else {
                self.malformed += 1;
                continue;
            };
            if !current(&f, inc, epoch) {
                self.discarded += 1;
                continue;
            }
            let topic = f.topic();
            match self.kept.get_mut(&topic) {
                Some(k) => {
                    let newer = k.frame.stamp.daemon_incarnation != f.stamp.daemon_incarnation
                        || k.frame.stamp.session_epoch != f.stamp.session_epoch
                        || f.stamp.seq > k.frame.stamp.seq;
                    if newer {
                        k.frame = Arc::new(f);
                        k.received = now;
                    }
                }
                None => {
                    self.kept.insert(
                        topic,
                        Kept {
                            name: Arc::from(topic.to_string()),
                            frame: Arc::new(f),
                            received: now,
                        },
                    );
                }
            }
        }

        // Kept frames from an older epoch or another incarnation go too.
        let before = self.kept.len();
        self.kept.retain(|_, k| current(&k.frame, inc, epoch));
        self.discarded += (before - self.kept.len()) as u64;

        let responding = view.responding(now);
        out.frames.retain(|_, tf| self.kept.contains_key(&tf.topic));
        for (topic, k) in &self.kept {
            let since_new = now.saturating_duration_since(k.received);
            let age = frame_age(&k.frame, now_wall_ns, view.clock_offset_ns);
            let stale = is_stale(topic, responding, since_new, age);
            let tf = TopicFrame {
                topic: *topic,
                frame: k.frame.clone(),
                received: k.received,
                since_new,
                age,
                stale,
            };
            match out.frames.get_mut(&*k.name) {
                Some(slot) => *slot = tf,
                None => {
                    out.frames.insert(k.name.clone(), tf);
                }
            }
        }
        out.read = read;
        out.malformed_total = self.malformed;
        out.discarded_total = self.discarded;
        out.responding = responding;
        Ok(())
    }
}

fn subscribed(prefixes: &BTreeMap<Vec<u8>, usize>, topic: &[u8]) -> bool {
    prefixes.keys().any(|p| topic.starts_with(p))
}

#[cfg(test)]
mod stale_tests {
    use super::*;
    use ac2_proto::Stream;
    use ac2_proto::units::MeasId;

    fn topic(stream: Stream) -> Topic {
        Topic::Data {
            meas: MeasId(1),
            stream,
        }
    }

    /// A once-a-second frame arriving a little after its second is not stale; one at the
    /// display rate is.
    #[test]
    fn once_a_second_streams_wait_three_seconds() {
        let late = Duration::from_millis(1500);
        for s in [Stream::Leq, Stream::BandLeq] {
            assert!(!is_stale(&topic(s), true, late, Some(1.5)), "{s:?}");
            assert!(is_stale(&topic(s), true, late, Some(3.1)), "{s:?}");
            assert!(is_stale(&topic(s), false, late, Some(0.1)), "{s:?}");
        }
        for s in Stream::ALL.into_iter().filter(|s| !s.once_a_second()) {
            assert!(is_stale(&topic(s), true, late, Some(1.5)), "{s:?}");
        }
    }
}
