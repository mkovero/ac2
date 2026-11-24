//! What the standing feed may show (`docs/design/remote-auto.md` §5.1): the judge's relation,
//! held by the whole minutes of marks behind it.
//!
//! The feed runs always, on hosts whose system clocks are slewed (chrony corrects an offset
//! by running the clock a little fast or slow for a while), and a slew reads in a minute's
//! fit as a rate: 100 µs over a minute is about 2 ppm, and the same slew moves a rig's own
//! line against its system clock. A slew passes, a rate difference stays. So the two
//! verdicts are held unequally:
//!
//! - *agree* once a whole minute closes within the rule with no count running, as the
//!   audio-fed relation says it: a slew only ever makes a locked pair look apart, never an
//!   apart pair look locked for a minute running;
//! - *not locked* (the cross-rig rate or either rig's own line, each as the judge reads it
//!   now, not as its watch holds it) only after [`HOLD_MINUTES`] whole minutes in a row
//!   beyond the rule, all on one side with one sign;
//! - ten minutes counted and not of one mind: *can't tell*, the figure not steady, with its
//!   spread; never *measuring* for ever.
//!
//! A count begins at a minute beyond [`BOUND_PPM`] and ends at two minutes in a row within
//! [`ENTER_PPM`]: the gap between the two keeps a figure that sits at the bound from
//! leaving and re-entering agreement every minute. What is shown changes only when a minute
//! closes (or the judge cannot tell): a live figure between closes is a fit over a window
//! still filling with the newest marks, the very thing the minute rule waits out.
//!
//! The count is cleared by anything the judge cannot tell, including the system clocks
//! disagreeing (marks in this rig's future) or the marks arriving late: either says a system
//! clock may have moved, and the minutes before rest on it. A hole longer than a minute
//! clears it too, so ten minutes in a row are ten minutes of marks, not two runs hours apart.

use std::collections::VecDeque;
use std::time::Duration;

use ac2_proto::model::{CantTellReason, MinuteCount, NotLockedReason, PeerRelation};
use ac2_proto::units::{RigId, Seconds, WallNs};

use super::peer_clock::{BOUND_PPM, Evidence, PeerClockWatch, judge};
use super::standing::{Minutes, StandingReport};
use super::{MAX_LINK_OFFSET_S, link_offset};

/// Whole minutes in a row a *not locked* verdict needs. chrony slews a measured offset over
/// about three of its update intervals; to read past 2 ppm through ten minutes running, one
/// correction would have to move the clock 1.2 ms or more at an update interval of 256 s or
/// longer, and chrony lengthens its interval only while the offsets it measures stay far
/// below that. A step (`maxerror` and the kernel's unsync flag) is the system clock checks'
/// to catch, not this.
pub(crate) const HOLD_MINUTES: usize = 10;
/// A count ends at two minutes in a row within this, ppm: below the bound by about half the
/// minute figure's scatter (0.34–0.40 ppm sd per 60 s window), so a figure at the bound
/// counts on rather than flipping in and out.
pub(crate) const ENTER_PPM: f64 = 1.8;
/// Marks late (or ahead) for this long before they say so: a link's queue that fills for a
/// moment is not a clock behind.
pub(crate) const LATE_IN: Duration = Duration::from_secs(3);
/// Clean for this long before a lateness stops showing: one at the bound must not flip the
/// relation every second.
pub(crate) const LATE_OUT: Duration = Duration::from_secs(3);
/// A shown lateness moves only by more than this, s.
const LATE_STEP: f64 = 0.01;
/// The shown figure is the one at each minute's close; between closes it moves only by more
/// than this, ppm. A 60 s fit slid by a second swaps a second of stamps at either end, and
/// tens of µs of stamp scatter there moves it by several hundredths of a ppm; a smaller step
/// would move the entry every few seconds on that alone. Half the rule still shows a real
/// excursion before its minute closes.
pub(crate) const FIGURE_STEP: f64 = 1.0;
/// `marks_since` is placed again only when it is off by more than this, s.
const SINCE_STEP_NS: u64 = 2_000_000_000;

/// How far past the link offset the marks arrive, s: the one in force when set (`set`),
/// else the longest any link may run, since the operator may need that much and this rig
/// cannot tell a long path from a late clock.
pub(crate) fn late(r: &StandingReport, set: Option<f64>) -> Option<f64> {
    let (ours, theirs) = r.periods;
    let offset = set.map_or(MAX_LINK_OFFSET_S, |s| link_offset(Some(s), ours, theirs));
    let by = r.least_age? - (offset + r.allow);
    (by > 0.0).then_some(by)
}

/// Which side of the rule a minute beyond it is on, and whose line: one sign throughout is
/// a rate, a sign that changes is noise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Cross { fast: bool },
    Own { rig: RigId, fast: bool },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Minute {
    Within(f64),
    Beyond(Side, f64),
}

impl Minute {
    fn ppm(self) -> f64 {
        match self {
            Self::Within(p) | Self::Beyond(_, p) => p,
        }
    }

    fn settled(self) -> bool {
        matches!(self, Self::Within(p) if p.abs() < ENTER_PPM)
    }
}

/// The minutes behind the verdict.
#[derive(Debug)]
struct Tally {
    /// The feed's series, long holes and minutes closed as last seen.
    seen: Option<(u64, u64, u64)>,
    last: Option<Minute>,
    /// Since the count began, the newest [`HOLD_MINUTES`]; empty: not counting.
    count: VecDeque<Minute>,
    /// What was shown last.
    shown: PeerRelation,
}

impl Default for Tally {
    fn default() -> Self {
        Self {
            seen: None,
            last: None,
            count: VecDeque::new(),
            shown: PeerRelation::Measuring,
        }
    }
}

impl Tally {
    fn clear(&mut self) {
        self.last = None;
        self.count.clear();
        self.shown = PeerRelation::Measuring;
    }

    /// The judge's relation now, unheld (`raw`), and the feed's minutes; whether a minute
    /// closed.
    fn note(&mut self, raw: &PeerRelation, minutes: Option<Minutes>) -> bool {
        match raw {
            // What the judge cannot tell, the minutes before it did not show either; what
            // is only missing (no marks, no link) leaves them standing.
            PeerRelation::CantTell { reason } if !only_missing(reason) => self.clear(),
            PeerRelation::NotLocked {
                reason: NotLockedReason::OtherRate { .. },
            } => self.clear(),
            _ => {}
        }
        let Some(m) = minutes else {
            return false;
        };
        let Some((series, holes, closed)) = self.seen.filter(|(s, ..)| *s == m.series) else {
            self.clear();
            self.seen = Some((m.series, m.holes, m.closed));
            return false;
        };
        if holes != m.holes {
            // The minutes before a long hole are not in a row with those after it; the
            // first to close after it is all after it.
            self.clear();
        }
        self.seen = Some((series, m.holes, m.closed));
        if m.closed == closed {
            return false;
        }
        let Some(p) = m.last else {
            // A minute too thin to fit breaks the run.
            self.clear();
            return true;
        };
        let minute = match raw {
            PeerRelation::NotLocked {
                reason: NotLockedReason::OwnClock { rig, ppm },
            } => Minute::Beyond(
                Side::Own {
                    rig: *rig,
                    fast: *ppm > 0.0,
                },
                *ppm,
            ),
            _ if p.abs() > BOUND_PPM => Minute::Beyond(Side::Cross { fast: p > 0.0 }, p),
            _ => Minute::Within(p),
        };
        let ends = minute.settled() && self.last.is_some_and(Minute::settled);
        if ends {
            self.count.clear();
        } else if !matches!(minute, Minute::Within(_)) || !self.count.is_empty() {
            if self.count.len() == HOLD_MINUTES {
                self.count.pop_front();
            }
            self.count.push_back(minute);
        }
        self.last = Some(minute);
        true
    }

    /// The relation shown with the watch's (held) relation `held`, and the count.
    fn shown(&mut self, held: PeerRelation) -> (PeerRelation, Option<MinuteCount>) {
        if matches!(
            held,
            PeerRelation::CantTell { .. }
                | PeerRelation::NotLocked {
                    reason: NotLockedReason::OtherRate { .. }
                }
        ) {
            return (held, None);
        }
        let shown = if self.count.len() == HOLD_MINUTES {
            self.verdict()
        } else if self.count.is_empty()
            && matches!(self.last, Some(Minute::Within(_)))
            && (held == PeerRelation::Agree || self.shown == PeerRelation::Agree)
        {
            // Agreement shown stays until a minute says otherwise; a not-locked the watch
            // still holds keeps it measuring until the watch lets go.
            PeerRelation::Agree
        } else {
            PeerRelation::Measuring
        };
        self.shown = shown;
        (shown, self.counting())
    }

    /// Ten minutes counted: of one side and sign, or not steady.
    fn verdict(&self) -> PeerRelation {
        let side = |m: &Minute| match m {
            Minute::Beyond(s, _) => Some(*s),
            Minute::Within(_) => None,
        };
        let first = self.count.front().and_then(side);
        let last = self.count.back().map_or(0.0, |m| m.ppm());
        if let Some(s) = first
            && self.count.iter().all(|m| side(m) == Some(s))
        {
            let reason = match s {
                Side::Cross { .. } => NotLockedReason::CrossRate {
                    ppm: last,
                    over: Seconds(60.0),
                },
                Side::Own { rig, .. } => NotLockedReason::OwnClock { rig, ppm: last },
            };
            return PeerRelation::NotLocked { reason };
        }
        let ppms = || self.count.iter().map(|m| m.ppm());
        PeerRelation::CantTell {
            reason: CantTellReason::NotSteady {
                low: ppms().fold(f64::INFINITY, f64::min),
                high: ppms().fold(f64::NEG_INFINITY, f64::max),
            },
        }
    }

    fn counting(&self) -> Option<MinuteCount> {
        (!self.count.is_empty() && self.count.len() < HOLD_MINUTES).then(|| MinuteCount {
            beyond: self
                .count
                .iter()
                .filter(|m| matches!(m, Minute::Beyond(..)))
                .count() as u32,
            counted: self.count.len() as u32,
            needed: HOLD_MINUTES as u32,
        })
    }
}

/// A reason that says only that evidence is missing, not that it says something.
fn only_missing(r: &CantTellReason) -> bool {
    matches!(
        r,
        CantTellReason::NoAudio
            | CantTellReason::NoMarks
            | CantTellReason::NoSession { .. }
            | CantTellReason::LinkDown
    )
}

/// A system-time disagreement (late or ahead) with a persistence in and out, its figure
/// moved only by more than [`LATE_STEP`].
#[derive(Debug, Default)]
struct Persist {
    over_since: Option<Duration>,
    clean_since: Option<Duration>,
    by: Option<f64>,
}

impl Persist {
    /// The disagreement at `now`, `by`; what is shown.
    fn update(&mut self, by: Option<f64>, now: Duration) -> Option<f64> {
        match by {
            Some(b) => {
                self.clean_since = None;
                let since = *self.over_since.get_or_insert(now);
                let moved = self.by.is_none_or(|s| (b - s).abs() > LATE_STEP);
                if moved && (self.by.is_some() || now.saturating_sub(since) >= LATE_IN) {
                    self.by = Some(b);
                }
            }
            None => {
                self.over_since = None;
                let since = *self.clean_since.get_or_insert(now);
                if now.saturating_sub(since) >= LATE_OUT {
                    self.by = None;
                }
            }
        }
        self.by
    }
}

/// The standing relation with one paired rig: the judge's watch, the minutes behind it and
/// the system-time disagreements' persistence.
#[derive(Debug)]
pub(crate) struct StandingWatch {
    pub(crate) watch: PeerClockWatch,
    tally: Tally,
    late: Persist,
    ahead: Persist,
    figure: Option<(f64, Seconds)>,
    since: Option<WallNs>,
}

/// What a standing entry shows: figures to 0.01 ppm, whole seconds and whole milliseconds,
/// each moved only by a step that means something, so a steady pair's entry stays put.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Shown {
    pub(crate) relation: PeerRelation,
    pub(crate) ppm: Option<f64>,
    pub(crate) span: Option<Seconds>,
    pub(crate) counting: Option<MinuteCount>,
    /// When the marks counted now began, this rig's wall time.
    pub(crate) marks_since: Option<WallNs>,
}

impl StandingWatch {
    pub(crate) fn new() -> Self {
        Self {
            watch: PeerClockWatch::absent(CantTellReason::LinkDown),
            tally: Tally::default(),
            late: Persist::default(),
            ahead: Persist::default(),
            figure: None,
            since: None,
        }
    }

    /// Judges `e` (its `late` and `disagree` before persistence) at `now` (time since the
    /// daemon's start) and wall time `wall`, with the feed's `minutes`.
    pub(crate) fn update(
        &mut self,
        mut e: Evidence<'_>,
        minutes: Option<Minutes>,
        now: Duration,
        wall: WallNs,
    ) -> Shown {
        e.late = self.late.update(e.late, now);
        e.disagree = self.ahead.update(e.disagree, now);
        let raw = judge(&e);
        self.watch.update(&e, now, wall);
        let closed = self.tally.note(&raw, minutes);
        let (relation, counting) = self.tally.shown(self.watch.clock.relation);
        let live = self.watch.clock.ppm.zip(self.watch.clock.span);
        self.figure = match (self.figure, live) {
            (Some((was, span)), Some((p, _))) if !closed && (p - was).abs() < FIGURE_STEP => {
                Some((was, span))
            }
            (_, live) => live.map(|(p, s)| (hundredths(p), Seconds(s.0.round()))),
        };
        // Kept through a gap in the reports: marks that come back to the same count land on
        // the same time and do not move it.
        if let Some(d) = e.streaming_for {
            let at = WallNs(wall.0.saturating_sub(d.as_nanos() as u64));
            if self
                .since
                .is_none_or(|s| s.0.abs_diff(at.0) > SINCE_STEP_NS)
            {
                self.since = Some(at);
            }
        }
        Shown {
            relation: quantise(relation),
            ppm: self.figure.map(|f| f.0),
            span: self.figure.map(|f| f.1),
            counting,
            marks_since: e.streaming_for.and(self.since),
        }
    }
}

fn hundredths(x: f64) -> f64 {
    // `+ 0.0` keeps −0.00 from reading apart from 0.00.
    (x * 100.0).round() / 100.0 + 0.0
}

fn millis(s: Seconds) -> Seconds {
    Seconds((s.0 * 1e3).round() / 1e3 + 0.0)
}

/// The relation with its figures as shown.
fn quantise(r: PeerRelation) -> PeerRelation {
    match r {
        PeerRelation::NotLocked { reason } => PeerRelation::NotLocked {
            reason: match reason {
                NotLockedReason::CrossRate { ppm, over } => NotLockedReason::CrossRate {
                    ppm: hundredths(ppm),
                    over: Seconds(over.0.round()),
                },
                NotLockedReason::OwnClock { rig, ppm } => NotLockedReason::OwnClock {
                    rig,
                    ppm: hundredths(ppm),
                },
                other => other,
            },
        },
        PeerRelation::CantTell { reason } => PeerRelation::CantTell {
            reason: match reason {
                CantTellReason::ClocksDisagree { by } => {
                    CantTellReason::ClocksDisagree { by: millis(by) }
                }
                CantTellReason::MarksLate { by } => CantTellReason::MarksLate { by: millis(by) },
                CantTellReason::NotSteady { low, high } => CantTellReason::NotSteady {
                    low: hundredths(low),
                    high: hundredths(high),
                },
                other => other,
            },
        },
        other => other,
    }
}

#[cfg(test)]
#[path = "standing_verdict_tests.rs"]
mod tests;
