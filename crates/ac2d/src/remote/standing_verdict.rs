//! What the standing feed may show (`docs/design/remote-auto.md` §5.1): the judge's relation,
//! held by the whole minutes of marks behind it.
//!
//! The feed runs always, on hosts whose system clocks are slewed (chrony corrects an offset
//! by running the clock a little fast or slow for a while), and a slew reads in a minute's
//! fit as a rate: 100 µs over a minute is about 2 ppm, and the same slew moves a rig's own
//! line against its system clock. A slew passes, a rate difference stays. So the two
//! verdicts are held unequally:
//!
//! - *agree* once a whole minute closes within the rule with no count running and the judge
//!   agreeing: a slew only ever makes a locked pair look apart, never an apart pair look
//!   locked for a minute running;
//! - *not locked* (the cross-rig rate or either rig's own line, each as the judge reads it
//!   at the minute's close) only once [`HOLD_MINUTES`] measured minutes in a row are all
//!   beyond the rule, on one side with one sign;
//! - ten minutes counted and not of one mind: *can't tell*, the figure not steady, with its
//!   spread; never *measuring* for ever.
//!
//! A count begins at a minute beyond [`BOUND_PPM`] and ends at two minutes in a row below
//! [`ENTER_PPM`], or at ten all within the rule: the gap between the two bounds keeps a
//! figure that sits at the rule from leaving and re-entering agreement every minute. What is
//! shown changes as minutes close, or as the evidence stops; between closes only a shown
//! figure beyond the rule takes agreement away.
//!
//! Evidence that stops says nothing (no link, no marks, a hole of a few seconds in them):
//! the count stands, and agreement waits for a minute closing after it. Evidence the judge
//! cannot tell from (marks late or in this rig's future, an unsynced system clock, another
//! nominal rate) says a system clock may have moved: the count goes, and so does the minute
//! open then, which spans the disturbance. A hole of over a minute starts the count again:
//! ten minutes in a row are ten measured minutes, not two runs hours apart.
//!
//! [`Hold::step`] is all of it, a function of [`Hold`] and one report's [`Input`].

use std::collections::VecDeque;
use std::time::Duration;

use ac2_proto::model::{CantTellReason, MinuteCount, NotLockedReason, PeerRelation};
use ac2_proto::units::{RigId, Seconds, WallNs};

use super::peer_clock::{BOUND_PPM, Evidence, judge};
use super::standing::{Minutes, StandingReport};
use super::{MAX_LINK_OFFSET_S, link_offset};

/// Measured minutes in a row a *not locked* verdict needs. chrony slews a measured offset
/// over about three of its update intervals; to read past 2 ppm through ten minutes running,
/// one correction would have to move the clock 1.2 ms or more at an update interval of 256 s
/// or longer, and chrony lengthens its interval only while the offsets it measures stay far
/// below that. A step (`maxerror` and the kernel's unsync flag) is the system clock checks'
/// to catch, not this.
pub(crate) const HOLD_MINUTES: usize = 10;
/// A count ends at two minutes in a row below this, ppm: below the rule by a margin, so a
/// figure that sits at the rule counts on rather than flipping in and out.
pub(crate) const ENTER_PPM: f64 = 1.8;
/// Marks late (or ahead) for this long before they say so: a link's queue that fills for a
/// moment is not a clock behind.
pub(crate) const LATE_IN: Duration = Duration::from_secs(3);
/// Clean for this long before a lateness stops showing: one at the bound must not flip the
/// relation every second.
pub(crate) const LATE_OUT: Duration = Duration::from_secs(3);
/// A shown lateness moves only by more than this, s.
const LATE_STEP: f64 = 0.01;
/// The shown figure is the closed minute's; between closes it moves to the live one only
/// when that is more than this away, ppm. A 60 s fit slid by a second swaps a second of
/// stamps at either end, and tens of µs of stamp scatter there moves it by several
/// hundredths of a ppm; a smaller step would move the entry every few seconds on that
/// alone. Half the rule still shows a real excursion before its minute closes.
pub(crate) const FIGURE_STEP: f64 = 1.0;
/// `marks_since` is placed again only when it is more than this off, ns.
const SINCE_STEP_NS: u64 = 2_000_000_000;
/// A closed minute's figure is a fit over this, s.
const MINUTE_S: f64 = 60.0;

/// How far past the link offset the marks arrive, s: the one in force when set (`set`),
/// else the longest any link may run, since the operator may need that much and this rig
/// cannot tell a long path from a late clock.
pub(crate) fn late(r: &StandingReport, set: Option<f64>) -> Option<f64> {
    let (ours, theirs) = r.periods;
    let offset = set.map_or(MAX_LINK_OFFSET_S, |s| link_offset(Some(s), ours, theirs));
    let by = r.least_age? - (offset + r.allow);
    (by > 0.0).then_some(by)
}

/// A closed minute as counted: the cross-rig figure it was fitted to, and how it was classed.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Minute {
    cross: f64,
    class: Class,
}

/// Which side of the rule a minute is on, and whose line put it there: one sign throughout
/// is a rate, a sign that changes is noise.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Class {
    Within,
    Cross { fast: bool },
    Own { rig: RigId, ppm: f64 },
}

impl Minute {
    /// `cross` (hundredths, as shown) with the judge's `raw` relation at its close.
    fn classed(cross: f64, raw: &Raw) -> Self {
        let class = match *raw {
            Raw::Own { rig, ppm } => Class::Own { rig, ppm },
            _ if cross.abs() > BOUND_PPM => Class::Cross { fast: cross > 0.0 },
            _ => Class::Within,
        };
        Self { cross, class }
    }

    fn within(self) -> bool {
        self.class == Class::Within
    }

    fn settled(self) -> bool {
        self.within() && self.cross.abs() < ENTER_PPM
    }

    /// One side and sign: whose line, and fast or slow.
    fn side(self) -> Option<(Option<RigId>, bool)> {
        match self.class {
            Class::Within => None,
            Class::Cross { fast } => Some((None, fast)),
            Class::Own { rig, ppm } => Some((Some(rig), ppm > 0.0)),
        }
    }
}

/// The judge's relation now, as the hold reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Raw {
    Agree,
    /// Measuring, or the cross-rig rate beyond the rule now: neither is agreement, and the
    /// minute's own figure says which side it closed on.
    NotAgree,
    /// A rig's own line beyond the rule.
    Own {
        rig: RigId,
        ppm: f64,
    },
    /// Evidence stopped: shown as it is, the count stands.
    Missing(PeerRelation),
    /// The judge cannot tell, or the nominal rates differ: shown as it is, the count and
    /// the open minute go.
    Stop(PeerRelation),
}

impl Raw {
    pub(super) fn of(r: PeerRelation) -> Self {
        match r {
            PeerRelation::Agree => Self::Agree,
            PeerRelation::Measuring => Self::NotAgree,
            PeerRelation::NotLocked {
                reason: NotLockedReason::CrossRate { .. },
            } => Self::NotAgree,
            PeerRelation::NotLocked {
                reason: NotLockedReason::OwnClock { rig, ppm },
            } => Self::Own { rig, ppm },
            PeerRelation::CantTell { reason } if only_missing(&reason) => Self::Missing(r),
            _ => Self::Stop(r),
        }
    }
}

/// What the feed's minutes did since the last report.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Event {
    None,
    /// A minute closed at this cross-rig figure (`None`: too thin to fit).
    Close(Option<f64>),
    /// A hole of a few seconds: the open minute was dropped, and a minute that closed in
    /// the same report is not counted, since which side of the hole it ends on is unknown
    /// here.
    Gap,
    /// A new series or a hole of over a minute: the minutes before are not in a row with
    /// those after. A minute that closed in the same report closed before it.
    Restart,
}

impl Event {
    /// From the minutes `seen` last (series, gaps, holes, closed) and now.
    fn of(seen: &mut Option<(u64, u64, u64, u64)>, m: Option<Minutes>) -> Self {
        let Some(m) = m else {
            return Self::None;
        };
        let now = (m.series, m.gaps, m.holes, m.closed);
        let Some(was) = seen.replace(now) else {
            return Self::Restart;
        };
        if (was.0, was.2) != (now.0, now.2) {
            Self::Restart
        } else if was.1 != now.1 {
            Self::Gap
        } else if was.3 != now.3 {
            Self::Close(m.last)
        } else {
            Self::None
        }
    }
}

/// One report as the hold reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Input {
    pub(super) raw: Raw,
    pub(super) event: Event,
    /// The live long-window figure, (ppm, span s).
    pub(super) live: Option<(f64, f64)>,
}

/// What must happen before agreement may show again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wait {
    Nothing,
    /// Evidence stopped since the last close: a minute must close after it.
    Close,
    /// The open minute spans a disturbance: it is not classed, and the one after it must
    /// close.
    Discard,
}

/// All the hold knows: the minutes behind the verdict and what is shown.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Hold {
    /// The newest closed minute since the count last started again; `None`: none, or thin.
    last: Option<Minute>,
    /// Since the count began, the newest [`HOLD_MINUTES`]; empty: not counting.
    count: VecDeque<Minute>,
    wait: Wait,
    /// What was shown, and its figure (ppm, span s).
    shown: PeerRelation,
    figure: Option<(f64, f64)>,
}

impl Default for Hold {
    fn default() -> Self {
        Self {
            last: None,
            count: VecDeque::new(),
            wait: Wait::Nothing,
            shown: PeerRelation::Measuring,
            figure: None,
        }
    }
}

impl Hold {
    /// The relation shown after `input`.
    pub(super) fn step(&mut self, input: Input) -> PeerRelation {
        let Input { raw, event, live } = input;
        if event == Event::Restart {
            self.start_again();
        }
        match raw {
            Raw::Stop(r) => {
                self.start_again();
                self.wait = Wait::Discard;
                self.shown = r;
                return r;
            }
            Raw::Missing(r) => {
                if self.wait == Wait::Nothing {
                    self.wait = Wait::Close;
                }
                self.figure = None;
                self.shown = r;
                return r;
            }
            _ => {}
        }
        let closed = match event {
            Event::Close(p) => {
                // The figure shown is the one the minute was classed by, or none.
                let p = p.map(hundredths).filter(|_| self.wait != Wait::Discard);
                if self.wait == Wait::Discard {
                    self.last = None;
                } else {
                    self.close(p, &raw);
                }
                self.wait = Wait::Nothing;
                self.figure = p.map(|p| (p, MINUTE_S));
                true
            }
            Event::Gap => {
                if self.wait == Wait::Nothing {
                    self.wait = Wait::Close;
                }
                false
            }
            Event::None | Event::Restart => false,
        };
        if !closed
            && let (Some((was, _)), Some((p, span))) = (self.figure, live)
            && (p - was).abs() > FIGURE_STEP
        {
            self.figure = Some((hundredths(p), span.round()));
        }
        let figure_within = self.figure.is_some_and(|(p, _)| p.abs() <= BOUND_PPM);
        let shown = if self.count.len() == HOLD_MINUTES {
            self.verdict()
        } else if !self.count.is_empty()
            || self.wait != Wait::Nothing
            || !self.last.is_some_and(Minute::within)
            || !figure_within
        {
            PeerRelation::Measuring
        } else if raw == Raw::Agree || (!closed && self.shown == PeerRelation::Agree) {
            // At a close the judge must agree as well; between closes agreement shown
            // stays, since a live window still filling is what the minutes wait out.
            PeerRelation::Agree
        } else {
            PeerRelation::Measuring
        };
        self.shown = shown;
        shown
    }

    fn start_again(&mut self) {
        self.last = None;
        self.count.clear();
        self.wait = Wait::Nothing;
        self.figure = None;
    }

    /// A minute closed at `cross` (`None`: thin), with the judge's `raw` relation then.
    fn close(&mut self, cross: Option<f64>, raw: &Raw) {
        let Some(cross) = cross else {
            // A minute too thin to fit breaks the run.
            self.last = None;
            self.count.clear();
            return;
        };
        let minute = Minute::classed(cross, raw);
        if minute.settled() && self.last.is_some_and(Minute::settled) {
            self.count.clear();
        } else if !minute.within() || !self.count.is_empty() {
            if self.count.len() == HOLD_MINUTES {
                self.count.pop_front();
            }
            self.count.push_back(minute);
            // Ten all within the rule, short of the margin: nothing left to count.
            if self.count.len() == HOLD_MINUTES && self.count.iter().all(|m| m.within()) {
                self.count.clear();
            }
        }
        self.last = Some(minute);
    }

    /// Ten minutes counted: of one side and sign, or not steady.
    fn verdict(&self) -> PeerRelation {
        let first = self.count.front().and_then(|m| m.side());
        if let (Some(side), Some(last)) = (first, self.count.back())
            && self.count.iter().all(|m| m.side() == Some(side))
        {
            let reason = match last.class {
                Class::Own { rig, ppm } => NotLockedReason::OwnClock { rig, ppm },
                _ => NotLockedReason::CrossRate {
                    ppm: last.cross,
                    over: Seconds(MINUTE_S),
                },
            };
            return PeerRelation::NotLocked { reason };
        }
        // One quantity per figure: the cross-rig figures, or the own lines' when no minute
        // was the cross figure's.
        let cross: Vec<f64> = self
            .count
            .iter()
            .filter(|m| !matches!(m.class, Class::Own { .. }))
            .map(|m| m.cross)
            .collect();
        let ppms = if cross.is_empty() {
            self.count
                .iter()
                .filter_map(|m| match m.class {
                    Class::Own { ppm, .. } => Some(ppm),
                    _ => None,
                })
                .collect()
        } else {
            cross
        };
        PeerRelation::CantTell {
            reason: CantTellReason::NotSteady {
                low: ppms.iter().copied().fold(f64::INFINITY, f64::min),
                high: ppms.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            },
        }
    }

    fn counting(&self) -> Option<MinuteCount> {
        (!self.count.is_empty() && self.count.len() < HOLD_MINUTES).then(|| MinuteCount {
            beyond: self.count.iter().filter(|m| !m.within()).count() as u32,
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

/// When the marks counted now began, at wall time `wall` with them paired for `streaming`;
/// the one placed before (`was`) kept unless more than [`SINCE_STEP_NS`] off, so the
/// entry does not move with the second the two clocks are read in.
fn place_since(was: Option<WallNs>, wall: WallNs, streaming: Duration) -> WallNs {
    let at = WallNs(wall.0.saturating_sub(streaming.as_nanos() as u64));
    match was {
        Some(s) if s.0.abs_diff(at.0) <= SINCE_STEP_NS => s,
        _ => at,
    }
}

/// The standing relation with one paired rig: the hold, the feed's minutes as last seen and
/// the system-time disagreements' persistence.
#[derive(Debug)]
pub(crate) struct StandingWatch {
    hold: Hold,
    seen: Option<(u64, u64, u64, u64)>,
    late: Persist,
    ahead: Persist,
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
            hold: Hold::default(),
            seen: None,
            late: Persist::default(),
            ahead: Persist::default(),
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
        let input = Input {
            raw: Raw::of(judge(&e)),
            event: Event::of(&mut self.seen, minutes),
            live: e.long,
        };
        let relation = self.hold.step(input);
        // Kept through a gap in the reports: marks that come back to the same count land on
        // the same time and do not move it.
        if let Some(d) = e.streaming_for {
            self.since = Some(place_since(self.since, wall, d));
        }
        let figure = self.hold.figure;
        Shown {
            relation: quantise(relation),
            ppm: figure.map(|f| f.0),
            span: figure.map(|f| Seconds(f.1)),
            counting: self.hold.counting(),
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
