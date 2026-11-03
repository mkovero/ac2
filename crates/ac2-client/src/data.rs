//! Data subscriptions and the latest-frame view (Q2).
//!
//! The data DEALER is never read frame by frame. [`DataState::drain`] reads everything
//! queued (until EAGAIN, bounded) and applies it in order: frames of one topic arrive in
//! send order over the one connection, each a keyframe or a delta on the frame before it.
//! A delta applies only on the exact frame it names as its base and only if the result
//! matches its CRC; otherwise it is refused, the last applied frame is kept, and a keyframe
//! is asked for at most once per [`KEYFRAME_ASK_GAP`]. A refused frame never counts as new,
//! so the kept frame turns STALE (and is drawn dimmed) if no keyframe comes: never a curve
//! built on the wrong base, and an old one reads as old. A delta with no frame held yet
//! waits for the keyframe its subscription brings without asking. Values are built only
//! for the newest frame per topic of a drain. Malformed frames are counted, frames of older
//! session epochs or another daemon incarnation discarded.
//!
//! Flow control (`docs/protocol.md` §5.8): the connection states its window, and every
//! drain returns credit for what it read, so the daemon sends this consumer what it takes:
//! a consumer that drains rarely (a slow display, a starved link) receives the newest frame
//! of each topic in turn instead of a backlog. A reconnect is a new connection to the
//! daemon, which knows nothing of it: the window and every subscription are stated again
//! as the socket monitor reports the new handshake, and when the daemon asks (`rejoin`).
//!
//! A consumer can sleep until data is queued instead of polling: see
//! [`crate::Client::data_changed`] and [`crate::Client::wait_data`].

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ac2_proto::flow::{FLOW_TOPIC, frame_charge, outside_window};
use ac2_proto::frame::{FrameStamp, MAX_PARTS};
use ac2_proto::units::{DaemonIncarnation, SessionEpoch};
use ac2_proto::{
    CodedFrame, DataRequest, DataUp, FlowNotice, Frame, GridId, Subscription, Topic, WireFrame,
    decode_notice, decode_wire, encode_data_request,
};
use ac2_zmq::{Monitor, MonitorEvent, Part, Socket};

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

/// Shortest interval between two keyframe requests for one topic: the daemon's refresh
/// interval, so a receiver asks at most as often as an unchanged result is re-sent.
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

/// Owner of the data DEALER, the subscriptions and the decoded frames.
#[derive(Debug)]
pub(crate) struct DataState {
    sock: Socket,
    /// The DEALER's connection events: a new handshake is a new peer to the daemon.
    monitor: Monitor,
    window: u32,
    subs: HashMap<Subscription, usize>,
    kept: HashMap<Topic, Kept>,
    malformed: u64,
    discarded: u64,
    refused: u64,
    /// Bytes received from the daemon ([`ClientConfig::received_bytes`](crate::ClientConfig)).
    received: Arc<AtomicU64>,
    /// Read and not yet credited: frame bytes (as charged), and messages outside the window.
    owed: (u32, u32),
    /// Topics whose chain broke: a keyframe request is due.
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

/// Sends `msg` on a data DEALER without blocking. A message that finds the queue full is
/// dropped: the queue fills only while the connection is down, and the next connection is
/// a new peer, which is told everything again.
pub(crate) fn send_up(sock: &Socket, msg: DataUp) -> Result<(), ClientError> {
    let b = encode_data_request(&DataRequest::new(msg))
        .map_err(|e| ClientError::Invalid(format!("data request: {e}")))?;
    match sock.try_send(&[b]) {
        Ok(()) | Err(ac2_zmq::Error::WouldBlock) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Whether a monitor reported a completed handshake (draining what it holds).
pub(crate) fn handshook(m: &Monitor) -> bool {
    let mut seen = false;
    while let Ok(Some(ev)) = m.try_recv() {
        seen |= ev.event == MonitorEvent::HandshakeSucceeded;
    }
    seen
}

impl DataState {
    /// Over `sock`, a DEALER connected to the data endpoint, granting `window` bytes.
    pub(crate) fn new(
        sock: Socket,
        monitor: Monitor,
        window: u32,
        received: Arc<AtomicU64>,
    ) -> Result<Self, ClientError> {
        send_up(&sock, DataUp::Window { bytes: window })?;
        Ok(Self {
            sock,
            monitor,
            window,
            received,
            subs: HashMap::new(),
            kept: HashMap::new(),
            malformed: 0,
            discarded: 0,
            refused: 0,
            owed: (0, 0),
            asks: Vec::new(),
        })
    }

    /// The window and every subscription, to a daemon that knows nothing of this
    /// connection.
    fn restate(&mut self) -> Result<(), ClientError> {
        send_up(&self.sock, DataUp::Window { bytes: self.window })?;
        for s in self.subs.keys() {
            send_up(&self.sock, DataUp::Subscribe { sub: *s })?;
        }
        Ok(())
    }

    fn credit(&mut self) -> Result<(), ClientError> {
        let (bytes, events) = std::mem::take(&mut self.owed);
        if bytes > 0 || events > 0 {
            send_up(&self.sock, DataUp::Credit { bytes, events })?;
        }
        Ok(())
    }

    fn wants(&self, t: &Topic) -> bool {
        self.subs.keys().any(|s| s.matches(t))
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

    /// The data DEALER, for waiting on it.
    pub(crate) fn socket(&self) -> &Socket {
        &self.sock
    }

    /// The DEALER's monitor, for waiting on it: a new connection is to be told everything.
    pub(crate) fn monitor_socket(&self) -> &Socket {
        self.monitor.socket()
    }

    pub(crate) fn subscribe(&mut self, sub: Subscription) -> Result<(), ClientError> {
        let n = self.subs.entry(sub).or_insert(0);
        *n += 1;
        if *n == 1 {
            send_up(&self.sock, DataUp::Subscribe { sub })?;
        }
        Ok(())
    }

    pub(crate) fn unsubscribe(&mut self, sub: Subscription) -> Result<(), ClientError> {
        let Some(n) = self.subs.get_mut(&sub) else {
            return Err(ClientError::Invalid(format!(
                "not subscribed to {:?}",
                String::from_utf8_lossy(&sub.prefix())
            )));
        };
        *n -= 1;
        if *n > 0 {
            return Ok(());
        }
        self.subs.remove(&sub);
        // What is still queued under it is skipped as it is read.
        send_up(&self.sock, DataUp::Unsubscribe { sub })?;
        let subs = &self.subs;
        self.kept.retain(|t, _| subs.keys().any(|s| s.matches(t)));
        Ok(())
    }

    /// Drains the DEALER into `out`: applies every message in order, builds the values of
    /// the newest frame per topic, and recomputes age and STALE of every kept frame at
    /// `now`. `out` is updated in place, so a caller that keeps it reuses its map and topic
    /// names. Credit for what was read goes back before it returns, and keyframe requests
    /// for chains that broke.
    pub(crate) fn drain(
        &mut self,
        view: &MirrorView,
        now: Instant,
        now_wall_ns: i128,
        out: &mut Latest,
    ) -> Result<(), ClientError> {
        let inc = view.incarnation;
        let epoch = view.session_epoch;
        if handshook(&self.monitor) {
            self.restate()?;
        }
        let mut read = 0;
        while read < DRAIN_LIMIT {
            let Some(parts) = self.sock.try_recv_parts()? else {
                break;
            };
            read += 1;
            let bytes: usize = parts.iter().map(|p| p.len()).sum();
            self.received.fetch_add(bytes as u64, Ordering::Relaxed);
            let Some(first) = parts.first() else { continue };
            // Credit on receipt: a frame refused below still took its share of the link.
            if outside_window(first) {
                self.owed.1 = self.owed.1.saturating_add(1);
                if &first[..] == FLOW_TOPIC
                    && let Some(body) = parts.get(1)
                    && decode_notice(body) == Ok(FlowNotice::Rejoin)
                {
                    self.restate()?;
                }
                // The keepalives a quiet connection is sent are not read.
                continue;
            }
            self.owed.0 = self.owed.0.saturating_add(frame_charge(bytes));
            if self.owed.0 >= self.window / 2 {
                // The daemon may be waiting for this credit; it need not wait for the drain.
                self.credit()?;
            }
            // Queued before an unsubscribe went out: skipped.
            match Topic::parse(first) {
                Ok(t) if self.wants(&t) => {}
                _ => continue,
            }
            match decode(&parts) {
                Some(w) => self.apply(w, view, now),
                None => self.malformed += 1,
            }
        }
        self.credit()?;
        // A chain that broke: its next frame as a keyframe. Not waited for; a lost request
        // is asked again after `KEYFRAME_ASK_GAP` while deltas keep failing.
        for topic in std::mem::take(&mut self.asks) {
            send_up(&self.sock, DataUp::Keyframe { topic })?;
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
