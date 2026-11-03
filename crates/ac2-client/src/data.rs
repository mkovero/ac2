//! Data subscriptions and the latest-frame view (Q2).
//!
//! The data SUB is never read frame by frame. [`DataState::drain`] reads everything queued
//! (until EAGAIN, bounded) and applies it in order: frames of one topic arrive in publish
//! order over the one connection, each a keyframe or a delta on the frame before it. A
//! delta applies only on the exact frame it names as its base and only if the result
//! matches its CRC; otherwise it is refused, the last applied frame is kept, and a
//! `data.keyframe` is asked for at most once per [`KEYFRAME_ASK_GAP`]. A refused frame never
//! counts as new, so the kept frame turns STALE (and is drawn dimmed) if no keyframe comes:
//! never a curve built on the wrong base, and an old one reads as old. A delta with no frame
//! held yet (a new subscription sees deltas before its keyframe) waits for that keyframe
//! without asking. Values are built only for the newest frame per topic of a drain.
//! Malformed frames are counted, frames of older session epochs or another daemon
//! incarnation discarded. Because a publisher drops the *newest* messages for a stalled
//! subscriber, only a drain gets back to fresh data in one pass.
//!
//! A consumer can sleep until data is queued instead of polling: see
//! [`crate::Client::data_changed`] and [`crate::Client::wait_data`].

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ac2_proto::frame::{FrameStamp, MAX_PARTS};
use ac2_proto::units::{DaemonIncarnation, SessionEpoch};
use ac2_proto::{CodedFrame, Frame, GridId, Topic, WireFrame, decode_wire};
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

/// Shortest interval between two `data.keyframe` requests for one topic: the daemon's
/// refresh interval, so a receiver asks at most as often as an unchanged result is re-sent.
pub const KEYFRAME_ASK_GAP: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
struct Kept {
    /// Topic text, made once per topic and shared with every [`Latest`] built from it.
    name: Arc<str>,
    /// The frame applied last, as codes: the base of the next delta.
    coded: CodedFrame,
    /// Its values; `None` until the first applied frame has been built.
    frame: Option<Arc<Frame>>,
    /// Applied since its values were last built.
    fresh: bool,
    /// When a newer `seq` than the one before was last applied.
    received: Instant,
    /// When a keyframe was last asked for.
    asked_at: Option<Instant>,
}

/// Owner of the data SUB, the subscriptions and the decoded frames.
#[derive(Debug)]
pub(crate) struct DataState {
    sub: Socket,
    prefixes: BTreeMap<Vec<u8>, usize>,
    kept: HashMap<Topic, Kept>,
    malformed: u64,
    discarded: u64,
    refused: u64,
    /// Bytes received from the daemon ([`ClientConfig::received_bytes`](crate::ClientConfig)).
    received: Arc<AtomicU64>,
    /// Topics whose chain broke: a `data.keyframe` is due ([`DataState::take_asks`]).
    asks: Vec<Topic>,
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
    /// Deltas refused since the client started: not on the frame held, or not matching
    /// their CRC.
    pub refused_total: u64,
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

fn current(s: &FrameStamp, inc: Option<DaemonIncarnation>, epoch: Option<SessionEpoch>) -> bool {
    inc.is_none_or(|i| s.daemon_incarnation == i) && epoch.is_none_or(|e| s.session_epoch >= e)
}

/// Frame age in seconds at local wall time `now_wall_ns`, given the `daemon − local` offset.
pub fn frame_age(f: &Frame, now_wall_ns: i128, offset_ns: Option<i128>) -> Option<f64> {
    let off = offset_ns?;
    let age_ns = now_wall_ns + off - i128::from(f.stamp.capture_wall_ns.0);
    Some(age_ns as f64 / 1e9)
}

fn decode(parts: &[Part]) -> Option<WireFrame> {
    let mut refs: [&[u8]; MAX_PARTS] = [&[]; MAX_PARTS];
    if parts.len() > refs.len() {
        return None;
    }
    for (r, p) in refs.iter_mut().zip(parts) {
        *r = p;
    }
    decode_wire(&refs[..parts.len()]).ok()
}

/// Whether a keyframe of stamp `s` replaces the frame held (`held`): a newer `seq`, or
/// another incarnation or epoch.
fn newer(s: &FrameStamp, held: &FrameStamp) -> bool {
    s.daemon_incarnation != held.daemon_incarnation
        || s.session_epoch != held.session_epoch
        || s.seq > held.seq
}

impl DataState {
    pub(crate) fn new(sub: Socket, received: Arc<AtomicU64>) -> Self {
        Self {
            sub,
            received,
            prefixes: BTreeMap::new(),
            kept: HashMap::new(),
            malformed: 0,
            discarded: 0,
            refused: 0,
            asks: Vec::new(),
        }
    }

    /// Topics a `data.keyframe` is due for, since the last call.
    pub(crate) fn take_asks(&mut self) -> Vec<Topic> {
        std::mem::take(&mut self.asks)
    }

    /// One message, in order: a keyframe replaces the frame held, a delta applies on it.
    fn apply(&mut self, w: WireFrame, view: &MirrorView, now: Instant) {
        let topic = w.topic();
        match w {
            WireFrame::Key(k) => {
                if !current(&k.stamp, view.incarnation, view.session_epoch) {
                    self.discarded += 1;
                    return;
                }
                match self.kept.get_mut(&topic) {
                    Some(kp) => {
                        let bump = newer(&k.stamp, &kp.coded.stamp);
                        // The same frame again (a subscriber joined): nothing new to draw.
                        if bump || k.stamp.seq == kp.coded.stamp.seq {
                            kp.coded = k;
                            kp.fresh |= bump;
                            if bump {
                                kp.received = now;
                            }
                        }
                    }
                    None => {
                        self.kept.insert(
                            topic,
                            Kept {
                                name: Arc::from(topic.to_string()),
                                coded: k,
                                frame: None,
                                fresh: true,
                                received: now,
                                asked_at: None,
                            },
                        );
                    }
                }
            }
            WireFrame::Delta(d) => {
                // No frame held yet: the keyframe the subscription brings is on its way.
                let Some(kp) = self.kept.get_mut(&topic) else {
                    return;
                };
                match d.apply(&kp.coded) {
                    Ok(c) => {
                        kp.coded = c;
                        kp.fresh = true;
                        kp.received = now;
                    }
                    Err(_) => {
                        self.refused += 1;
                        if kp
                            .asked_at
                            .is_none_or(|t| now.saturating_duration_since(t) >= KEYFRAME_ASK_GAP)
                        {
                            kp.asked_at = Some(now);
                            self.asks.push(topic);
                        }
                    }
                }
            }
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
        Ok(())
    }

    /// Drains the SUB into `out`: applies every message in order, builds the values of the
    /// newest frame per topic, and recomputes age and STALE of every kept frame at `now`. `out` is updated in place, so a caller that
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
            let bytes: usize = parts.iter().map(|p| p.len()).sum();
            self.received.fetch_add(bytes as u64, Ordering::Relaxed);
            let Some(topic) = parts.first() else { continue };
            // The SUB filters queued messages as they are read, except the one a readiness
            // check (`ZMQ_EVENTS`) already took in: that one comes out unfiltered after its
            // prefix is dropped, and kept now it would stay for good.
            if !subscribed(&self.prefixes, topic) {
                continue;
            }
            match decode(&parts) {
                Some(w) => self.apply(w, view, now),
                None => self.malformed += 1,
            }
        }

        // Values only for the newest frame of each topic.
        for kp in self.kept.values_mut().filter(|k| k.fresh) {
            kp.fresh = false;
            match kp.coded.to_frame() {
                Ok(f) => kp.frame = Some(Arc::new(f)),
                Err(_) => self.malformed += 1,
            }
        }

        // Kept frames from an older epoch or another incarnation go too.
        let before = self.kept.len();
        self.kept.retain(|_, k| current(&k.coded.stamp, inc, epoch));
        self.discarded += (before - self.kept.len()) as u64;

        let responding = view.responding(now);
        out.frames
            .retain(|_, tf| self.kept.get(&tf.topic).is_some_and(|k| k.frame.is_some()));
        for (topic, k) in &self.kept {
            let Some(frame) = &k.frame else { continue };
            let since_new = now.saturating_duration_since(k.received);
            let age = frame_age(frame, now_wall_ns, view.clock_offset_ns);
            let stale = is_stale(topic, responding, since_new, age);
            let tf = TopicFrame {
                topic: *topic,
                frame: frame.clone(),
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
        out.refused_total = self.refused;
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
