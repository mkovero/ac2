//! Standing evidence (`docs/design/remote-auto.md` §5.1): the clocks between this rig and a
//! paired one from the timing mark on every `source/levels` frame, with no audio between
//! the rigs.
//!
//! A mark is a block's sample index with its capture time on the source's system time. This
//! rig's own mark (its newest stamped block, its stamp clock's line onto its system clock)
//! puts that system time on this rig's sample index, the quantity the joined feed measures
//! per audio block ([`local_at`] for both); the cross-rig rate is fitted over the same 60 s
//! and 40 s windows and judged by the same judge. Only the feed differs, so the relation is
//! there whether or not any audio streams.
//!
//! Either rig's new incarnation or reopened session starts the windows again: a sample index
//! then means something new. A link gap only leaves a hole: both sample indices kept
//! counting through it, so the marks after it still fit the line of those before.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ac2_core::clock_map::StampLine;
use ac2_core::clock_rate::RateWindow;
use ac2_proto::frame::{FrameStamp, METER_FPS, SourceLevelsFrame};
use ac2_proto::units::{DaemonIncarnation, SessionEpoch};

use super::join::{CROSS_LONG_S, CROSS_SHORT_S, CROSS_STEP_S};

/// A mark waits up to one meter period for its frame beside the link offset's transit and
/// periods; the age check allows it.
fn meter_period_s() -> f64 {
    1.0 / f64::from(METER_FPS)
}

/// This rig's own mark is taken as gone this long after the fan-out last wrote it (it
/// writes at every hand-off, tens of times a second): the stream stopped.
const LOCAL_STALE: Duration = Duration::from_secs(1);
/// No frame, or no mark paired, for this long: the feed lapsed. The link reports once a
/// second, so one second of silence must not flip on a report's timing.
const LAPSED: Duration = Duration::from_secs(2);
/// Marks placed further back than the bound may only have waited on the way (transit and
/// credit only ever add age): the least age over this long is taken, as the joined feed's
/// anchor does.
const AGE_WINDOW: Duration = Duration::from_secs(1);

/// Local sample index (fractional) captured at system time `system_ns`, given a reference
/// block `(sample, stamp)` and this rig's stamp clock on its system clock.
pub(crate) fn local_at(reference: (u64, u64), line: StampLine, rate: f64, system_ns: f64) -> f64 {
    let (ref_sample, ref_stamp) = reference;
    let ours = line.stamp_of(system_ns);
    ref_sample as f64 + (ours - ref_stamp as f64) * rate / 1e9
}

/// This rig's side of the pairing, as the fan-out last saw it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalMark {
    pub(crate) epoch: SessionEpoch,
    pub(crate) rate: u32,
    /// The newest stamped block: its first frame's index and capture stamp.
    pub(crate) reference: (u64, u64),
    pub(crate) line: StampLine,
    /// One past the newest captured sample when written.
    pub(crate) latest: u64,
    /// How far behind the present `latest` may lie when written (blocks wait for the
    /// hand-off) plus a device period of stamp jitter, frames.
    pub(crate) slack: u64,
    /// This rig's device period, frames.
    pub(crate) period: u32,
    /// Continuity breaks of the capture so far.
    pub(crate) breaks: u64,
    pub(crate) at: Instant,
}

/// The cell the fan-out writes this rig's mark into and every link reads.
#[derive(Debug, Default)]
pub(crate) struct LocalClock(Mutex<Option<LocalMark>>);

impl LocalClock {
    pub(crate) fn set(&self, m: Option<LocalMark>) {
        if let Ok(mut g) = self.0.lock() {
            *g = m;
        }
    }

    /// The mark, unless the stream stopped writing it.
    pub(crate) fn get(&self, now: Instant) -> Option<LocalMark> {
        let m = (*self.0.lock().ok()?)?;
        (now.saturating_duration_since(m.at) < LOCAL_STALE).then_some(m)
    }
}

/// Why no marks feed the windows now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Absent {
    /// This rig has no session (or its stream stopped).
    OurSession,
    /// No frame from the other rig: it has no session.
    TheirSession,
    /// Frames come without marks, or none could be paired.
    Marks,
}

/// What the standing feed reports, once a second.
#[derive(Debug, Clone)]
pub(crate) struct StandingReport {
    /// The cross-rig rate over the long and the short window: (ppm, span s).
    pub(crate) long: Option<(f64, f64)>,
    pub(crate) short: Option<(f64, f64)>,
    /// Marks paired since both sessions (re)started.
    pub(crate) marks_for: Option<Duration>,
    /// The marks place the other rig's system clock outside the span transit allows, by
    /// this, s (positive: the other rig's ahead).
    pub(crate) disagree: Option<f64>,
    pub(crate) absent: Option<Absent>,
    /// The two sessions' nominal rates differ: (theirs, ours).
    pub(crate) other_rate: Option<(u32, u32)>,
    /// The other rig's newest frame, while frames come.
    pub(crate) levels: Option<Box<SourceLevelsFrame>>,
}

/// The run both sample indices belong to: either side's new one starts the windows again.
type Run = (DaemonIncarnation, SessionEpoch, SessionEpoch);

/// The standing feed of one paired rig.
pub(crate) struct StandingFeed {
    long: RateWindow,
    short: RateWindow,
    run: Option<Run>,
    rate: u32,
    since: Option<Instant>,
    last_frame: Option<Instant>,
    last_mark: Option<Instant>,
    breaks: Option<(u64, u64)>,
    /// Ages of the marks over the last [`AGE_WINDOW`], s.
    ages: VecDeque<(Instant, f64)>,
    disagree: Option<f64>,
    other_rate: Option<(u32, u32)>,
    no_local: bool,
    levels: Option<Box<SourceLevelsFrame>>,
}

impl Default for StandingFeed {
    fn default() -> Self {
        Self::new()
    }
}

impl StandingFeed {
    pub(crate) fn new() -> Self {
        Self {
            long: RateWindow::new(1.0, CROSS_LONG_S, CROSS_STEP_S),
            short: RateWindow::new(1.0, CROSS_SHORT_S, CROSS_STEP_S),
            run: None,
            rate: 0,
            since: None,
            last_frame: None,
            last_mark: None,
            breaks: None,
            ages: VecDeque::new(),
            disagree: None,
            other_rate: None,
            no_local: false,
            levels: None,
        }
    }

    fn restart(&mut self, run: Run, rate: u32) {
        let r = f64::from(rate);
        self.long = RateWindow::new(r, CROSS_LONG_S, CROSS_STEP_S);
        self.short = RateWindow::new(r, CROSS_SHORT_S, CROSS_STEP_S);
        self.run = Some(run);
        self.rate = rate;
        self.since = None;
        self.breaks = None;
        self.ages.clear();
        self.disagree = None;
    }

    /// One frame of the other rig as received at `now`, stamped `stamp`; `local`: this
    /// rig's mark; `their_period`: the other rig's device period, frames, when known.
    pub(crate) fn frame(
        &mut self,
        stamp: &FrameStamp,
        f: SourceLevelsFrame,
        local: Option<LocalMark>,
        their_period: Option<u32>,
        now: Instant,
    ) {
        self.last_frame = Some(now);
        let mark = f.meta.mark;
        let theirs = f.meta.sample_rate_hz;
        self.levels = Some(Box::new(f));
        self.no_local = local.is_none();
        let Some(local) = local else {
            return;
        };
        if theirs != local.rate {
            // Rates apart are two clocks the transfer functions cannot join, whatever the
            // marks would say: nothing is fitted.
            self.other_rate = Some((theirs, local.rate));
            self.run = None;
            return;
        }
        self.other_rate = None;
        let Some(mark) = mark else {
            return;
        };
        let run = (stamp.daemon_incarnation, stamp.session_epoch, local.epoch);
        if self.run != Some(run) {
            self.restart(run, local.rate);
        }
        let rate = f64::from(local.rate);
        let at = local_at(local.reference, local.line, rate, mark.system_ns as f64);
        if at < 0.0 {
            return;
        }
        let breaks = (mark.breaks, local.breaks);
        let broke = self.breaks.is_some_and(|b| b != breaks);
        self.breaks = Some(breaks);
        let ns = (at * 1e9 / rate) as u64;
        self.long.push(mark.sample.0, ns, broke);
        self.short.push(mark.sample.0, ns, broke);
        self.since.get_or_insert(now);
        self.last_mark = Some(now);
        self.age_check(local, at, their_period, now);
    }

    /// How long before the present the mark was captured, by system time, against what
    /// transit allows: none (less this rig's own hand-off lag and a period of jitter) up to
    /// the link offset of both periods and a meter period. Outside it the two system clocks
    /// disagree by at least that much.
    fn age_check(&mut self, local: LocalMark, at: f64, their_period: Option<u32>, now: Instant) {
        let rate = f64::from(local.rate);
        let present =
            local.latest as f64 + now.saturating_duration_since(local.at).as_secs_f64() * rate;
        let age = (present - at) / rate;
        while self
            .ages
            .front()
            .is_some_and(|(t, _)| now.saturating_duration_since(*t) > AGE_WINDOW)
        {
            self.ages.pop_front();
        }
        self.ages.push_back((now, age));
        let slack = local.slack as f64 / rate;
        if age < -slack {
            self.disagree = Some(-age - slack);
            return;
        }
        let ours = f64::from(local.period) / rate;
        let theirs = their_period.map_or(ours, |p| f64::from(p) / rate);
        let bound = super::default_link_offset(ours, theirs) + meter_period_s() + slack;
        let settled = self
            .since
            .is_some_and(|t| now.saturating_duration_since(t) >= AGE_WINDOW);
        let least = self
            .ages
            .iter()
            .map(|(_, a)| *a)
            .fold(f64::INFINITY, f64::min);
        self.disagree = (settled && least > bound).then(|| -(least - bound));
    }

    /// The feed as it stands at `now`.
    pub(crate) fn report(&self, now: Instant) -> StandingReport {
        let fresh =
            |t: Option<Instant>| t.is_some_and(|t| now.saturating_duration_since(t) < LAPSED);
        let absent = if !fresh(self.last_frame) {
            Some(Absent::TheirSession)
        } else if self.no_local {
            Some(Absent::OurSession)
        } else if self.other_rate.is_some() {
            None
        } else if !fresh(self.last_mark) {
            Some(Absent::Marks)
        } else {
            None
        };
        let cross = |r: ac2_core::clock_rate::ClockRate| (r.ppm, r.span_s);
        let fed = self.run.is_some();
        StandingReport {
            long: self.long.rate().filter(|_| fed).map(cross),
            short: self.short.rate().filter(|_| fed).map(cross),
            marks_for: self
                .since
                .filter(|_| fed)
                .map(|t| now.saturating_duration_since(t)),
            disagree: self.disagree.filter(|_| absent.is_none()),
            absent,
            other_rate: self.other_rate.filter(|_| fresh(self.last_frame)),
            levels: self.levels.clone().filter(|_| fresh(self.last_frame)),
        }
    }
}

#[cfg(test)]
#[path = "standing_tests.rs"]
mod tests;
