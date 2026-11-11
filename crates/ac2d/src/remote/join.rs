//! The joined feed (`docs/design/remote-inputs.md` §2.2, §4): this rig's device blocks held
//! back by the link offset, the remote inputs' samples merged in at the same sample index.
//!
//! Each remote input keeps a ring indexed by this rig's sample index through `local = remote
//! + k`. k is fixed when the stream starts (or restarts) and then moves only by a slip:
//!
//! - **On system time** when both rigs stamp capture, both system clocks are synced and both
//!   stamp maps are fitted: the remote block's capture stamp is put on system time through
//!   the source's map, back onto this rig's stamp clock through its own, and onto a sample
//!   index by the newest local block's stamp. k then pairs samples captured at the same
//!   system time, whatever the transit.
//! - **On arrival** otherwise, or when the system-time anchor would put the block outside
//!   the link offset of its arrival (the two system clocks disagree by more than the slack):
//!   a block is due one link offset after it arrives.
//!
//! A local block is released once this rig has captured a link offset past its end, with
//! whatever remote samples are there; a missing one is a gap, marked on that input's column
//! only (`ColumnMarks`), so a job reading another input carries on. Audio that
//! keeps arriving after it was needed for a second runs the ring dry, and audio that lands
//! more than a link offset ahead of the present runs it over: either slips k back to half
//! the link offset of slack, by a known number of samples.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use ac2_core::clock_map::StampLine;
use ac2_core::clock_rate::{ClockRate, RateWindow};
use ac2_proto::frame::{AudioFlags, AudioFrame};
use ac2_proto::model::{
    Anchor, ArrivalReason, RemoteAudio, SourceState, StreamIncarnation, SystemClock,
};
use ac2_proto::units::{RigId, Seconds, SessionEpoch};

use crate::control::ControlMsg;
use crate::fanout::Block;

use super::link::LinkFrame;
use super::{ColumnMarks, InputReport, RemoteMsg, RigReport};

/// Without stamp maps yet (a source's map needs a few seconds of pairs), the anchor waits
/// this long for them before it settles for arrival.
const MAP_WAIT: Duration = Duration::from_secs(10);
/// Audio arriving after it was needed for this long: the ring ran dry.
const DRY_AFTER: Duration = Duration::from_secs(1);
/// Audio placed further back than the link offset may only have waited in a queue (transit
/// only ever adds age): fresher frames are waited for this long before the two system
/// clocks are taken to disagree.
const ANCHOR_WAIT: Duration = Duration::from_secs(1);
/// The joiner reports to the control thread once a second.
const REPORT_EVERY: Duration = Duration::from_secs(1);
/// Late frames are counted over the last hour, arrival margins over the last minute.
const LATE_WINDOW: Duration = Duration::from_secs(3600);
const MARGIN_WINDOW: Duration = Duration::from_secs(60);
/// The cross-rig rate is fitted over a minute and over 40 s (§3.1), a point per 50 ms.
const CROSS_LONG_S: f64 = 60.0;
const CROSS_SHORT_S: f64 = 40.0;
const CROSS_STEP_S: f64 = 0.05;
/// The local blocks held back are bounded: past this the oldest go without remote audio.
const MAX_HELD_S: f64 = 1.0;

/// One remote input of the session, in `remote_inputs` order.
pub(crate) struct JoinInput {
    pub(crate) slot: u16,
    pub(crate) rig: RigId,
    pub(crate) channel: u16,
}

/// What the joiner is made from.
pub(crate) struct JoinSetup {
    pub(crate) inputs: Vec<JoinInput>,
    /// Per remote rig, the frames its link hands on.
    pub(crate) rigs: Vec<(RigId, Receiver<LinkFrame>)>,
    /// Columns of the local blocks.
    pub(crate) local_columns: usize,
    pub(crate) rate: u32,
    /// This rig's device period, frames.
    pub(crate) period: u32,
    /// The link offset set for the session; `None`: the default.
    pub(crate) link_offset: Option<f64>,
    /// This rig's system clock is stated synced (kept current by the control thread).
    pub(crate) system_synced: Arc<AtomicBool>,
    pub(crate) to_control: Sender<ControlMsg>,
    pub(crate) epoch: SessionEpoch,
}

struct RigState {
    rig: RigId,
    rx: Receiver<LinkFrame>,
    source: Option<Box<SourceState>>,
    /// The remote rig's sample clock against this rig's, through both system clocks.
    long: RateWindow,
    short: RateWindow,
    /// When its stream started or re-anchored.
    since: Option<Instant>,
    /// The system-time anchor of its last start fell outside the link offset by this, s.
    disagree: Option<f64>,
}

struct InputState {
    slot: u16,
    rig: usize,
    channel: u16,
    audio: RemoteAudio,
    /// `local = remote + k`.
    k: Option<i64>,
    anchor: Option<Anchor>,
    incarnation: Option<StreamIncarnation>,
    /// When the first frame of this run came, for the stamp-map and anchor waits.
    first: Option<Instant>,
    /// The least age, samples, a frame of this run was placed at by system time while that
    /// put it further back than the link offset.
    least_age: Option<i64>,
    /// Samples at local indices `ring_start..`; NaN where none arrived.
    ring: VecDeque<f32>,
    ring_start: u64,
    late: u64,
    /// Late frames per second: (that second's first, count).
    late_at: VecDeque<(Instant, u32)>,
    /// Since when every frame arrived late.
    late_run: Option<Instant>,
    slips: u32,
    last_slip: Option<i64>,
    /// A slip not yet marked on a released block.
    pending_slip: i64,
    /// A late frame not yet marked on a released block.
    pending_late: bool,
    /// Least arrival margin per second, s.
    margins: VecDeque<(Instant, f64)>,
}

impl InputState {
    fn restart(&mut self) {
        self.k = None;
        self.anchor = None;
        self.first = None;
        self.least_age = None;
        self.ring.clear();
        self.late_run = None;
    }

    fn ring_end(&self) -> u64 {
        self.ring_start + self.ring.len() as u64
    }

    /// The sample at local index `at`, if one arrived.
    fn sample(&self, at: u64) -> Option<f32> {
        let i = at.checked_sub(self.ring_start)?;
        self.ring
            .get(usize::try_from(i).ok()?)
            .copied()
            .filter(|v| !v.is_nan())
    }

    /// Drops what lies before local index `at`.
    fn release_to(&mut self, at: u64) {
        while self.ring_start < at && self.ring.pop_front().is_some() {
            self.ring_start += 1;
        }
        if self.ring.is_empty() {
            self.ring_start = self.ring_start.max(at);
        }
    }
}

/// The joined feed's state, on the fan-out thread.
pub(crate) struct Joiner {
    inputs: Vec<InputState>,
    rigs: Vec<RigState>,
    local_columns: usize,
    rate: f64,
    period: u32,
    set_offset: Option<f64>,
    link: u64,
    system_synced: Arc<AtomicBool>,
    held: VecDeque<Block>,
    /// One past the newest local sample captured.
    latest: u64,
    /// Next local index to release.
    released: Option<u64>,
    /// The newest local block with a capture stamp: (start, stamp).
    reference: Option<(u64, u64)>,
    to_control: Sender<ControlMsg>,
    epoch: SessionEpoch,
    next_report: Instant,
}

impl Joiner {
    pub(crate) fn new(s: JoinSetup) -> Self {
        let rate = f64::from(s.rate);
        let rigs: Vec<RigState> = s
            .rigs
            .into_iter()
            .map(|(rig, rx)| RigState {
                rig,
                rx,
                source: None,
                long: RateWindow::new(rate, CROSS_LONG_S, CROSS_STEP_S),
                short: RateWindow::new(rate, CROSS_SHORT_S, CROSS_STEP_S),
                since: None,
                disagree: None,
            })
            .collect();
        let inputs = s
            .inputs
            .iter()
            .map(|i| InputState {
                slot: i.slot,
                rig: rigs.iter().position(|r| r.rig == i.rig).unwrap_or(0),
                channel: i.channel,
                audio: RemoteAudio::Waiting,
                k: None,
                anchor: None,
                incarnation: None,
                first: None,
                least_age: None,
                ring: VecDeque::new(),
                ring_start: 0,
                late: 0,
                late_at: VecDeque::new(),
                late_run: None,
                slips: 0,
                last_slip: None,
                pending_slip: 0,
                pending_late: false,
                margins: VecDeque::new(),
            })
            .collect();
        let mut j = Self {
            inputs,
            rigs,
            local_columns: s.local_columns,
            rate,
            period: s.period,
            set_offset: s.link_offset,
            link: 0,
            system_synced: s.system_synced,
            held: VecDeque::new(),
            latest: 0,
            released: None,
            reference: None,
            to_control: s.to_control,
            epoch: s.epoch,
            next_report: Instant::now(),
        };
        j.link = j.link_frames();
        j
    }

    /// The link offset in frames: the set one, floored at the default from both periods.
    fn link_frames(&self) -> u64 {
        let period_s = |frames: u32| f64::from(frames) / self.rate;
        let theirs = self
            .rigs
            .iter()
            .filter_map(|r| r.source.as_ref()?.session.as_ref())
            .map(|s| period_s(s.buffer_frames))
            .fold(period_s(self.period), f64::max);
        let s = super::link_offset(self.set_offset, period_s(self.period), theirs);
        (s * self.rate).round() as u64
    }

    /// Whether a remote input waits for this rig's audio (the fan-out then hands off every
    /// device period).
    pub(crate) fn link_seconds(&self) -> f64 {
        self.link as f64 / self.rate
    }

    /// Takes a hand-off's local blocks and whatever the links brought; returns the joined
    /// blocks now due. `line`: this rig's stamp clock on its system clock.
    pub(crate) fn push(
        &mut self,
        batch: &[Block],
        line: Option<StampLine>,
        now: Instant,
    ) -> Vec<Block> {
        for b in batch {
            if b.capture_stamp {
                self.reference = Some((b.start_sample, b.host_ns));
            }
            self.latest = b.end_sample();
            self.released.get_or_insert(b.start_sample);
            self.held.push_back(Block {
                start_sample: b.start_sample,
                frames: b.frames,
                channels: b.channels,
                flags: b.flags,
                wall_ns: b.wall_ns,
                host_ns: b.host_ns,
                capture_stamp: b.capture_stamp,
                callback_ns: b.callback_ns,
                marks: ColumnMarks::default(),
                data: b.data.clone(),
                stats: Vec::new(),
            });
        }
        self.take_links(line, now);
        let out = self.release();
        if now >= self.next_report {
            self.next_report = now + REPORT_EVERY;
            self.report(now);
        }
        out
    }

    fn take_links(&mut self, line: Option<StampLine>, now: Instant) {
        for r in 0..self.rigs.len() {
            loop {
                match self.rigs[r].rx.try_recv() {
                    Ok(LinkFrame::Source(s)) => self.source(r, s),
                    Ok(LinkFrame::Audio(a)) => self.audio(r, &a, line, now),
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                }
            }
        }
    }

    fn source(&mut self, r: usize, s: Box<SourceState>) {
        self.rigs[r].source = Some(s);
        let link = self.link_frames();
        if link != self.link {
            // A new offset moves every k: the streams start over on it.
            self.link = link;
            for i in &mut self.inputs {
                i.restart();
            }
        }
        let ours = self.rate as u32;
        for i in self.inputs.iter_mut().filter(|i| i.rig == r) {
            let fault = source_fault(self.rigs[r].source.as_deref(), i.channel, ours);
            i.audio = match fault {
                Some(f) => {
                    i.restart();
                    f
                }
                None if i.k.is_some() => RemoteAudio::Streaming,
                None => RemoteAudio::Starting,
            };
        }
    }

    /// Local sample index (fractional) captured at the same system time as remote `stamp`.
    fn local_of(&self, r: usize, stamp: u64, line: Option<StampLine>) -> Option<f64> {
        let map = self.rigs[r].source.as_ref()?.clocks.stamp_map?;
        let line = line?;
        let (ref_sample, ref_stamp) = self.reference?;
        let ours = line.stamp_of(map.system_of(stamp));
        Some(ref_sample as f64 + (ours - ref_stamp as f64) * self.rate / 1e9)
    }

    fn audio(&mut self, r: usize, a: &AudioFrame, line: Option<StampLine>, now: Instant) {
        let h = &a.header;
        let ours = self.rate as u32;
        let first_of_rig = self.inputs.iter().find(|i| i.rig == r).map(|i| i.channel);
        // The cross-rig rate from one channel per rig: every channel carries the same stamps.
        if first_of_rig == Some(a.channel)
            && let Some(local) = h.capture_ns.and_then(|c| self.local_of(r, c, line))
            && local >= 0.0
        {
            let ns = (local * 1e9 / self.rate) as u64;
            let rig = &mut self.rigs[r];
            let gap = h.flags.contains(AudioFlags::GAP);
            rig.long.push(h.start_sample, ns, gap);
            rig.short.push(h.start_sample, ns, gap);
        }
        let synced = self.system_synced.load(Ordering::Relaxed)
            && self.rigs[r]
                .source
                .as_ref()
                .is_some_and(|s| matches!(s.clocks.system, SystemClock::Synced { .. }));
        let local = h.capture_ns.and_then(|c| self.local_of(r, c, line));
        let (link, latest) = (self.link, self.latest);
        let period = u64::from(self.period);
        let has_source = self.rigs[r].source.is_some();
        let mut restarted = false;
        for i in self
            .inputs
            .iter_mut()
            .filter(|i| i.rig == r && i.channel == a.channel)
        {
            if h.rate != ours {
                i.audio = RemoteAudio::RateMismatch {
                    theirs: h.rate,
                    ours,
                };
                i.restart();
                continue;
            }
            if matches!(
                i.audio,
                RemoteAudio::NoSession
                    | RemoteAudio::NotCaptured
                    | RemoteAudio::Replaying
                    | RemoteAudio::OnThisRig
            ) {
                continue;
            }
            if i.incarnation != Some(h.incarnation) {
                // The source's capture restarted: its sample index means something new.
                i.incarnation = Some(h.incarnation);
                i.restart();
            }
            let end = h.start_sample + u64::from(h.frames);
            // Anchored before the source's state is read, the anchor could never become
            // system time; before this rig's first block, against no present at all.
            let Some(released) = self.released.filter(|_| has_source) else {
                i.audio = RemoteAudio::Starting;
                continue;
            };
            // On arrival only for want of a synced clock or a map: once both are there the
            // input moves to system time, or a delay measured across the rigs would include
            // the network's transit and the link offset.
            if matches!(
                i.anchor,
                Some(Anchor::Arrival {
                    reason: ArrivalReason::SystemClock | ArrivalReason::NoStampMap
                })
            ) && local.is_some()
                && synced
            {
                i.restart();
            }
            if i.k.is_none() {
                let first = *i.first.get_or_insert(now);
                let anchor = match (h.capture_ns, local, synced) {
                    (None, _, _) => Some(Anchor::Arrival {
                        reason: ArrivalReason::NoCaptureStamps,
                    }),
                    (Some(_), _, false) => Some(Anchor::Arrival {
                        reason: ArrivalReason::SystemClock,
                    }),
                    (Some(_), Some(_), true) => Some(Anchor::SystemTime),
                    (Some(_), None, true) if now.duration_since(first) >= MAP_WAIT => {
                        Some(Anchor::Arrival {
                            reason: ArrivalReason::NoStampMap,
                        })
                    }
                    (Some(_), None, true) => None,
                };
                let Some(mut anchor) = anchor else {
                    i.audio = RemoteAudio::Starting;
                    continue;
                };
                let arrival = latest as i64 - end as i64;
                let mut k = arrival;
                if anchor == Anchor::SystemTime
                    && let Some(l) = local
                {
                    let ks = l.round() as i64 - h.start_sample as i64;
                    // How long before now this block's end was captured, by system time:
                    // between none (less one hand-off of this rig) and the link offset.
                    let age = latest as i64 - (end as i64 + ks);
                    // How far outside that span: at least this far apart are the clocks
                    // (positive: the source's ahead, its audio placed in the future).
                    let outside = if age < -(period as i64) {
                        Some(-age - period as i64)
                    } else if age > link as i64 {
                        let least = i.least_age.map_or(age, |a| a.min(age));
                        i.least_age = Some(least);
                        if now.duration_since(first) < ANCHOR_WAIT {
                            i.audio = RemoteAudio::Starting;
                            continue;
                        }
                        (least > link as i64).then(|| -(least - link as i64))
                    } else {
                        None
                    };
                    match outside {
                        None => k = ks,
                        Some(by) => {
                            anchor = Anchor::Arrival {
                                reason: ArrivalReason::ClocksDisagree {
                                    by: Seconds(by as f64 / self.rate),
                                },
                            };
                        }
                    }
                }
                self.rigs[r].disagree = match anchor {
                    Anchor::Arrival {
                        reason: ArrivalReason::ClocksDisagree { by },
                    } => Some(by.0),
                    _ => None,
                };
                i.k = Some(k);
                i.anchor = Some(anchor);
                i.audio = RemoteAudio::Streaming;
                i.ring.clear();
                restarted = true;
            }
            let Some(k) = i.k else { continue };
            let start = h.start_sample as i64 + k;
            let stop = end as i64 + k;
            if stop <= released as i64 {
                late(i, now);
                if i.late_run
                    .is_some_and(|t| now.duration_since(t) >= DRY_AFTER)
                {
                    // The ring ran dry: move the map so audio lands with half the slack.
                    let to = latest as i64 - link as i64 / 2;
                    slip(i, to - stop, released);
                }
                continue;
            }
            if start < released as i64 {
                late(i, now);
            } else {
                i.late_run = None;
            }
            if stop > latest as i64 + link as i64 {
                // Audio landing more than a link offset ahead of the present: the ring
                // runs over.
                let to = latest as i64 - link as i64 / 2;
                slip(i, to - stop, released);
                continue;
            }
            let margin = (stop + link as i64 - latest as i64) as f64 / self.rate;
            margin_seen(i, margin, now);
            write(i, start, &a.samples, released);
        }
        if restarted {
            let rig = &mut self.rigs[r];
            rig.since = Some(now);
            rig.long = RateWindow::new(self.rate, CROSS_LONG_S, CROSS_STEP_S);
            rig.short = RateWindow::new(self.rate, CROSS_SHORT_S, CROSS_STEP_S);
        }
    }

    /// The held local blocks now a link offset old, joined.
    fn release(&mut self) -> Vec<Block> {
        let mut out = Vec::new();
        let max_held = (MAX_HELD_S * self.rate) as u64 + self.link;
        while let Some(front) = self.held.front() {
            let due = front.end_sample() + self.link <= self.latest
                || self.latest.saturating_sub(front.end_sample()) > max_held;
            if !due {
                break;
            }
            let Some(b) = self.held.pop_front() else {
                break;
            };
            out.push(self.join(b));
        }
        out
    }

    fn join(&mut self, b: Block) -> Block {
        let n = self.local_columns;
        let width = n + self.inputs.len();
        let frames = b.frames as usize;
        let mut data = Vec::with_capacity(frames * width);
        let mut marks = ColumnMarks::default();
        let column = |slot: usize| u16::try_from(n + slot).unwrap_or(u16::MAX);
        for f in 0..frames {
            data.extend_from_slice(&b.data[f * n..(f + 1) * n]);
            let at = b.start_sample + f as u64;
            for (s, i) in self.inputs.iter().enumerate() {
                match i.sample(at) {
                    Some(v) => data.push(v),
                    None => {
                        marks.gap(column(s), f as u32);
                        data.push(0.0);
                    }
                }
            }
        }
        let end = b.end_sample();
        for (s, i) in self.inputs.iter_mut().enumerate() {
            i.release_to(end);
            let slip = std::mem::take(&mut i.pending_slip);
            if slip != 0 {
                marks.slip(column(s), slip);
            }
            if std::mem::take(&mut i.pending_late) {
                marks.late(column(s));
            }
        }
        self.released = Some(end);
        let mut j = Block {
            channels: u16::try_from(width).unwrap_or(u16::MAX),
            marks,
            data,
            stats: Vec::new(),
            ..b
        };
        j.compute_stats();
        j
    }

    fn report(&mut self, now: Instant) {
        let inputs = self
            .inputs
            .iter_mut()
            .map(|i| {
                while i
                    .late_at
                    .front()
                    .is_some_and(|(t, _)| now.duration_since(*t) > LATE_WINDOW)
                {
                    i.late_at.pop_front();
                }
                while i
                    .margins
                    .front()
                    .is_some_and(|(t, _)| now.duration_since(*t) > MARGIN_WINDOW)
                {
                    i.margins.pop_front();
                }
                InputReport {
                    slot: i.slot,
                    audio: i.audio,
                    anchor: i.anchor,
                    late_frames: i.late,
                    late_frames_hour: i.late_at.iter().map(|(_, n)| *n).sum(),
                    slips: i.slips,
                    last_slip: i.last_slip,
                    arrival_margin: i
                        .margins
                        .iter()
                        .map(|(_, m)| *m)
                        .reduce(f64::min)
                        .map(Seconds),
                }
            })
            .collect();
        let rigs = self
            .rigs
            .iter()
            .map(|r| RigReport {
                rig: r.rig,
                long: r.long.rate().map(cross),
                short: r.short.rate().map(cross),
                streaming_for: r.since.map(|t| now.duration_since(t)),
                disagree: r.disagree,
            })
            .collect();
        let _ = self.to_control.send(ControlMsg::Remote(RemoteMsg::Joined {
            epoch: self.epoch,
            inputs,
            rigs,
            link_offset: Seconds(self.link_seconds()),
        }));
    }
}

/// The cross-rig figure of a window: ppm and the span it covers.
fn cross(r: ClockRate) -> (f64, f64) {
    (r.ppm, r.span_s)
}

/// Why `source` gives no audio on `channel` at `ours` Hz; `None` when it can.
fn source_fault(source: Option<&SourceState>, channel: u16, ours: u32) -> Option<RemoteAudio> {
    let Some(src) = source else {
        return Some(RemoteAudio::Waiting);
    };
    let Some(s) = &src.session else {
        return Some(RemoteAudio::NoSession);
    };
    if s.replaying {
        return Some(RemoteAudio::Replaying);
    }
    if s.sample_rate_hz != ours {
        return Some(RemoteAudio::RateMismatch {
            theirs: s.sample_rate_hz,
            ours,
        });
    }
    if !s.inputs.iter().any(|i| i.channel == channel) {
        return Some(RemoteAudio::NotCaptured);
    }
    None
}

fn late(i: &mut InputState, now: Instant) {
    i.late += 1;
    i.pending_late = true;
    match i.late_at.back_mut() {
        Some((t, n)) if now.duration_since(*t) < Duration::from_secs(1) => *n += 1,
        _ => i.late_at.push_back((now, 1)),
    }
    i.late_run.get_or_insert(now);
}

/// Moves k by `by` samples: the ring starts over at the new map, one gap.
fn slip(i: &mut InputState, by: i64, released: u64) {
    if by == 0 {
        return;
    }
    if let Some(k) = &mut i.k {
        *k += by;
    }
    i.slips += 1;
    i.last_slip = Some(by);
    i.pending_slip += by;
    i.late_run = None;
    i.ring.clear();
    i.ring_start = released;
    tracing::warn!("remote input {} slipped {by} samples", i.slot);
}

fn margin_seen(i: &mut InputState, margin: f64, now: Instant) {
    match i.margins.back_mut() {
        Some((t, m)) if now.duration_since(*t) < Duration::from_secs(1) => *m = m.min(margin),
        _ => i.margins.push_back((now, margin)),
    }
}

/// Writes `samples` at local index `start` into the ring, skipping what was released.
fn write(i: &mut InputState, start: i64, samples: &[f32], released: u64) {
    let skip = usize::try_from((released as i64 - start).max(0)).unwrap_or(usize::MAX);
    let Some(samples) = samples.get(skip..) else {
        return;
    };
    let at = u64::try_from(start.max(released as i64)).unwrap_or(0);
    if i.ring.is_empty() {
        i.ring_start = at;
    }
    while i.ring_start > at {
        i.ring.push_front(f32::NAN);
        i.ring_start -= 1;
    }
    while i.ring_end() < at {
        i.ring.push_back(f32::NAN);
    }
    let base = usize::try_from(at - i.ring_start).unwrap_or(usize::MAX);
    for (k, &v) in samples.iter().enumerate() {
        if let Some(slot) = i.ring.get_mut(base.saturating_add(k)) {
            *slot = v;
        } else {
            i.ring.push_back(v);
        }
    }
}

#[cfg(test)]
#[path = "join_tests.rs"]
mod tests;
