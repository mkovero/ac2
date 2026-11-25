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
//! - *not locked* only once [`HOLD_MINUTES`] measured minutes in a row are all beyond the
//!   rule: the cross-rig figure on one sign, else one rig's own line on one sign, else some
//!   line in every one of them;
//! - ten minutes counted and not of one mind: *can't tell*, the figure not steady, with its
//!   spread; never *measuring* for ever.
//!
//! Each minute keeps both of its facts: the cross-rig figure it closed at, and the worst
//! either rig's own line read while it was open, of the lines stated a whole window
//! ([`LINE_SETTLED`]). It is beyond when either is beyond
//! [`BOUND_PPM`]; a count begins at a minute beyond and ends at two minutes in a row with
//! both below [`ENTER_PPM`], or at ten all within the rule: the gap between the two bounds
//! keeps a figure that sits at the rule from leaving and re-entering agreement every
//! minute. Counts that keep starting ([`FLAP_COUNTS`] within [`FLAP_WINDOW`]) are a figure
//! wandering across the rule: *not steady* in place of agreement until a quiet window.
//!
//! What is shown changes as minutes close, or as the evidence stops; between closes only a
//! shown figure beyond the rule takes agreement away, and only [`AGREE_REPORTS`] agreeing
//! reports in a row give it.
//!
//! Evidence that stops says nothing (no link, no marks, a hole of a few seconds in them):
//! the count stands, and both agreement and a verdict wait for a minute closing after it.
//! Holes that keep every minute from closing for [`BREAKING`] say so. Evidence the judge
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

use super::peer_clock::{BOUND_PPM, Evidence, judge, own_line, same_kind};
use super::standing::{Minutes, StandingReport};
use super::{MAX_LINK_OFFSET_S, link_offset};
use crate::capture_clock::WINDOW_S;
use crate::fanout::CLOCK_EVERY_S;

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
/// Agreeing reports in a row that give agreement between closes: one report's windows can
/// agree for a moment while sliding past a minute that did not.
pub(crate) const AGREE_REPORTS: u32 = 3;
/// Counts started within [`FLAP_WINDOW`] that read as a figure wandering across the rule.
/// A slew starts one count and ends it; three in half an hour is more than chrony's
/// corrections at the intervals [`HOLD_MINUTES`] assumes.
pub(crate) const FLAP_COUNTS: usize = 3;
pub(crate) const FLAP_WINDOW: Duration = Duration::from_secs(30 * 60);
/// No minute closed this long, with holes in the marks between: the marks keep breaking.
/// Five minutes of marks would have closed four or five minutes.
pub(crate) const BREAKING: Duration = Duration::from_secs(5 * 60);
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
/// A rig's own line classes a minute only once it has been stated this long without a
/// break: its window, a statement period (the reading in state may be the one before, whose
/// window still reaches back that far) and [`LINE_SLACK_S`]. Until then every reading is a
/// fit that still spans the stream's first seconds, whatever the device and its stamps did
/// while starting, so a reading beyond the rule then may be the start, not a rate: the minute
/// it falls in is left unclassed (see [`Hold::step`]), and the judge withholds agreement.
pub(crate) const LINE_SETTLED: Duration =
    Duration::from_secs((WINDOW_S + CLOCK_EVERY_S + LINE_SLACK_S) as u64);
/// The other rig's statement reaches this one in its state, sent on change over the link
/// and read at each report a second apart, s.
const LINE_SLACK_S: f64 = 2.0;

/// How far past the link offset the marks arrive, s: the one in force when set (`set`),
/// else the longest any link may run, since the operator may need that much and this rig
/// cannot tell a long path from a late clock.
pub(crate) fn late(r: &StandingReport, set: Option<f64>) -> Option<f64> {
    let (ours, theirs) = r.periods;
    let offset = set.map_or(MAX_LINK_OFFSET_S, |s| link_offset(Some(s), ours, theirs));
    let by = r.least_age? - (offset + r.allow);
    (by > 0.0).then_some(by)
}

/// A rig's own line: its sample clock against its system clock, ppm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Line {
    pub(crate) rig: RigId,
    pub(crate) ppm: f64,
}

/// A closed minute: the cross-rig figure it closed at (hundredths, as shown), and the worst
/// own line read while it was open.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Minute {
    cross: f64,
    line: Option<Line>,
}

impl Minute {
    fn cross_beyond(self) -> bool {
        self.cross.abs() > BOUND_PPM
    }

    fn line_beyond(self) -> Option<Line> {
        self.line.filter(|l| l.ppm.abs() > BOUND_PPM)
    }

    fn within(self) -> bool {
        !self.cross_beyond() && self.line_beyond().is_none()
    }

    fn settled(self) -> bool {
        self.cross.abs() < ENTER_PPM && self.line.is_none_or(|l| l.ppm.abs() < ENTER_PPM)
    }

    /// Why a minute beyond the rule is: the cross-rig figure first, the two clocks' own
    /// relation, else the own line.
    fn reason(self) -> NotLockedReason {
        match self.line_beyond().filter(|_| !self.cross_beyond()) {
            Some(Line { rig, ppm }) => NotLockedReason::OwnClock { rig, ppm },
            None => NotLockedReason::CrossRate {
                ppm: self.cross,
                over: Seconds(MINUTE_S),
            },
        }
    }
}

/// Whether `values` hold no more than one value.
fn one_value<T: PartialEq>(mut values: impl Iterator<Item = T>) -> bool {
    let first = values.next();
    values.all(|v| Some(v) == first)
}

/// The spread of `minutes`: of the cross-rig figure, or, where that stayed within the rule
/// and only an own line went beyond it, of that line.
fn spread(minutes: &[Minute]) -> CantTellReason {
    let line = minutes
        .iter()
        .rev()
        .find_map(|m| m.line_beyond())
        .filter(|_| minutes.iter().all(|m| !m.cross_beyond()));
    let ppms: Vec<f64> = match line {
        Some(l) => minutes
            .iter()
            .filter_map(|m| m.line.filter(|x| x.rig == l.rig).map(|x| x.ppm))
            .collect(),
        None => minutes.iter().map(|m| m.cross).collect(),
    };
    CantTellReason::NotSteady {
        low: ppms.iter().copied().fold(f64::INFINITY, f64::min),
        high: ppms.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        line: line.map(|l| l.rig),
    }
}

/// The judge's relation now, as the hold reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Raw {
    Agree,
    /// Measuring, or a rate beyond the rule now: neither is agreement, and the minute's own
    /// figures say which side it closed on.
    NotAgree,
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
                reason: NotLockedReason::CrossRate { .. } | NotLockedReason::OwnClock { .. },
            } => Self::NotAgree,
            PeerRelation::CantTell { reason } if only_missing(&reason) => Self::Missing(r),
            _ => Self::Stop(r),
        }
    }
}

/// What the feed's minutes did since the last report.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Event {
    None,
    /// A minute closed at this cross-rig figure (`None`: too thin to fit, or more than one
    /// closed since the last report, which leaves a minute unread).
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
        } else if now.3 == was.3 + 1 {
            Self::Close(m.last)
        } else if now.3 != was.3 {
            Self::Close(None)
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
    /// The worse of the two rigs' own lines now, of those stated [`LINE_SETTLED`].
    pub(super) line: Option<Line>,
    /// A line stated for less than that reads beyond the rule now.
    pub(super) young_beyond: bool,
    /// Time since the daemon's start.
    pub(super) now: Duration,
}

/// What must happen before agreement or a verdict may show again.
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
    /// The worst own line while the open minute was open.
    worst: Option<Line>,
    /// A young own line read beyond the rule while the open minute was open.
    young: bool,
    /// Whether the minute that closed at the last report was classed; `None`: none closed.
    classed: Option<bool>,
    /// Agreeing reports in a row.
    agrees: u32,
    /// Since when no minute closed, and whether a hole kept one from closing since.
    open_from: Option<Duration>,
    gapped: bool,
    /// Counts started and minutes closed within [`FLAP_WINDOW`], and whether counts kept
    /// starting.
    starts: VecDeque<Duration>,
    recent: VecDeque<(Duration, Minute)>,
    wandering: bool,
}

impl Default for Hold {
    fn default() -> Self {
        Self {
            last: None,
            count: VecDeque::new(),
            wait: Wait::Nothing,
            shown: PeerRelation::Measuring,
            figure: None,
            worst: None,
            young: false,
            classed: None,
            agrees: 0,
            open_from: None,
            gapped: false,
            starts: VecDeque::new(),
            recent: VecDeque::new(),
            wandering: false,
        }
    }
}

impl Hold {
    /// The relation shown after `input`.
    pub(super) fn step(&mut self, input: Input) -> PeerRelation {
        let Input {
            raw,
            event,
            live,
            line,
            young_beyond,
            now,
        } = input;
        self.forget(now);
        // A minute that closes in a report the judge cannot use is not classed either.
        self.classed = matches!(event, Event::Close(_)).then_some(false);
        if event == Event::Restart {
            self.start_again();
            self.starts.clear();
            self.recent.clear();
            self.wandering = false;
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
                self.agrees = 0;
                self.shown = r;
                return r;
            }
            Raw::Agree | Raw::NotAgree => {}
        }
        self.agrees = if raw == Raw::Agree {
            self.agrees + 1
        } else {
            0
        };
        self.young |= young_beyond;
        if let Some(l) = line
            && self.worst.is_none_or(|w| l.ppm.abs() > w.ppm.abs())
        {
            self.worst = Some(Line {
                rig: l.rig,
                ppm: hundredths(l.ppm),
            });
        }
        let closed = match event {
            Event::Close(p) => {
                // The figure shown is the one the minute was classed by, or none.
                let p = p.map(hundredths).filter(|_| self.wait != Wait::Discard);
                // A young line beyond the rule may be its stream's start or a rate: a minute
                // within on all else is neither within nor beyond. One beyond on its cross
                // figure or a settled line is beyond whatever the young line read.
                let unclassed = self.young
                    && p.is_some_and(|cross| {
                        Minute {
                            cross,
                            line: self.worst,
                        }
                        .within()
                    });
                if self.wait == Wait::Discard || unclassed {
                    self.last = None;
                } else {
                    self.close(p, now);
                }
                self.classed = Some(self.wait != Wait::Discard && p.is_some() && !unclassed);
                self.wait = Wait::Nothing;
                self.figure = p.map(|p| (p, MINUTE_S));
                self.worst = None;
                self.young = false;
                self.open_from = None;
                self.gapped = false;
                true
            }
            Event::Gap => {
                if self.wait == Wait::Nothing {
                    self.wait = Wait::Close;
                }
                self.figure = None;
                self.worst = None;
                self.young = false;
                self.gapped = true;
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
        let open = *self.open_from.get_or_insert(now);
        let figure_within = self.figure.is_some_and(|(p, _)| p.abs() <= BOUND_PPM);
        let shown = if self.gapped && now.saturating_sub(open) >= BREAKING {
            PeerRelation::CantTell {
                reason: CantTellReason::MarksKeepBreaking,
            }
        } else if self.wait != Wait::Nothing {
            PeerRelation::Measuring
        } else if self.count.len() == HOLD_MINUTES {
            self.verdict()
        } else if !self.count.is_empty() || !self.last.is_some_and(Minute::within) || !figure_within
        {
            PeerRelation::Measuring
        } else if self.wandering {
            let recent: Vec<Minute> = self.recent.iter().map(|(_, m)| *m).collect();
            PeerRelation::CantTell {
                reason: spread(&recent),
            }
        } else if (closed && raw == Raw::Agree)
            || (!closed && (self.shown == PeerRelation::Agree || self.agrees >= AGREE_REPORTS))
        {
            // At a close the judge must agree as well; between closes agreement shown
            // stays, since a live window still filling is what the minutes wait out.
            PeerRelation::Agree
        } else {
            PeerRelation::Measuring
        };
        self.shown = shown;
        shown
    }

    /// Counts and minutes older than [`FLAP_WINDOW`] go; a window with no count started
    /// ends the wandering.
    fn forget(&mut self, now: Duration) {
        let old = |t: Duration| now.saturating_sub(t) > FLAP_WINDOW;
        while self.starts.front().is_some_and(|t| old(*t)) {
            self.starts.pop_front();
        }
        while self.recent.front().is_some_and(|(t, _)| old(*t)) {
            self.recent.pop_front();
        }
        if self.starts.is_empty() {
            self.wandering = false;
        }
    }

    fn start_again(&mut self) {
        self.last = None;
        self.count.clear();
        self.wait = Wait::Nothing;
        self.figure = None;
        self.worst = None;
        self.young = false;
        self.agrees = 0;
        self.open_from = None;
        self.gapped = false;
    }

    /// A minute closed at `cross` (`None`: thin) at `now`, with the worst own line while it
    /// was open.
    fn close(&mut self, cross: Option<f64>, now: Duration) {
        let Some(cross) = cross else {
            // A minute too thin to fit breaks the run.
            self.last = None;
            self.count.clear();
            return;
        };
        let minute = Minute {
            cross,
            line: self.worst,
        };
        if minute.settled() && self.last.is_some_and(Minute::settled) {
            self.count.clear();
        } else if !minute.within() || !self.count.is_empty() {
            if self.count.is_empty() {
                self.starts.push_back(now);
                self.wandering |= self.starts.len() >= FLAP_COUNTS;
            }
            if self.count.len() == HOLD_MINUTES {
                self.count.pop_front();
            }
            self.count.push_back(minute);
            // Ten all within the rule, short of the margin: nothing left to count.
            if self.count.len() == HOLD_MINUTES && self.count.iter().all(|m| m.within()) {
                self.count.clear();
            }
        }
        self.recent.push_back((now, minute));
        self.last = Some(minute);
    }

    /// Ten minutes counted: the cross-rig figure beyond on one sign, else one rig's own line
    /// on one sign, else some line beyond in every one; or not steady.
    fn verdict(&self) -> PeerRelation {
        let minutes: Vec<Minute> = self.count.iter().copied().collect();
        let newest = minutes[minutes.len() - 1];
        let fast = newest.cross > 0.0;
        if minutes
            .iter()
            .all(|m| m.cross_beyond() && (m.cross > 0.0) == fast)
        {
            return PeerRelation::NotLocked {
                reason: NotLockedReason::CrossRate {
                    ppm: newest.cross,
                    over: Seconds(MINUTE_S),
                },
            };
        }
        if let Some(l) = newest.line_beyond()
            && minutes.iter().all(|m| {
                m.line_beyond()
                    .is_some_and(|x| x.rig == l.rig && (x.ppm > 0.0) == (l.ppm > 0.0))
            })
        {
            return PeerRelation::NotLocked {
                reason: NotLockedReason::OwnClock {
                    rig: l.rig,
                    ppm: l.ppm,
                },
            };
        }
        // Beyond every minute, on one line or the other, each line keeping its sign: a
        // free interface whose own line reads about the cross-rig rate, either side of the
        // rule in turn. A sign that turns is noise, not a rate.
        let cross_sign = minutes
            .iter()
            .filter(|m| m.cross_beyond())
            .map(|m| m.cross > 0.0);
        let line_side = minutes
            .iter()
            .filter_map(|m| m.line_beyond())
            .map(|l| (l.rig, l.ppm > 0.0));
        if minutes.iter().all(|m| !m.within()) && one_value(cross_sign) && one_value(line_side) {
            return PeerRelation::NotLocked {
                reason: newest.reason(),
            };
        }
        PeerRelation::CantTell {
            reason: spread(&minutes),
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
            | CantTellReason::NoStream { .. }
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

/// The judge's relation changes are logged at most once in this long per rig: a figure at
/// the rule can turn it every report.
const RAW_LOG_EVERY: Duration = Duration::from_secs(10);

/// The standing relation with one paired rig: the hold, the feed's minutes as last seen and
/// the system-time disagreements' persistence.
#[derive(Debug)]
pub(crate) struct StandingWatch {
    hold: Hold,
    seen: Option<(u64, u64, u64, u64)>,
    late: Persist,
    ahead: Persist,
    since: Option<WallNs>,
    /// Since when each rig's own line has been stated measured, time since the daemon's
    /// start.
    stated: Vec<(RigId, Duration)>,
    /// Each rig's worst own reading while the open minute was open, settled or not.
    readings: Vec<Reading>,
    /// The last report the judge could not tell from: the marks counted begin after it.
    stopped: Option<Duration>,
    /// The judge's relation as last logged, when, and the changes not logged since.
    raw: Option<PeerRelation>,
    raw_logged: Option<Duration>,
    raw_unlogged: u32,
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
    /// The judge's relation, before any hold, to log now, with the changes of it not
    /// logged since the last.
    pub(crate) raw_changed: Option<(PeerRelation, u32)>,
    /// A minute that closed at this report.
    pub(crate) closed: Option<Closed>,
}

/// A rig's own line as read, ppm, and whether it was stated for less than
/// [`LINE_SETTLED`] then.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Reading {
    pub(crate) rig: RigId,
    pub(crate) ppm: f64,
    pub(crate) young: bool,
}

/// A minute as it closed, for the log: its cross-rig figure (`None`: thin), each rig's worst
/// own reading over it, whether the hold classed it and the judge's relation then.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Closed {
    pub(crate) cross: Option<f64>,
    pub(crate) lines: Vec<Reading>,
    pub(crate) classed: bool,
    pub(crate) raw: PeerRelation,
}

/// Whether two judge relations are the same for the log: of a kind, and a not-locked one
/// for the same reason.
fn same_raw(a: &PeerRelation, b: &PeerRelation) -> bool {
    same_kind(a, b)
        && match (a, b) {
            (PeerRelation::NotLocked { reason: x }, PeerRelation::NotLocked { reason: y }) => {
                std::mem::discriminant(x) == std::mem::discriminant(y)
            }
            _ => true,
        }
}

impl StandingWatch {
    pub(crate) fn new() -> Self {
        Self {
            hold: Hold::default(),
            seen: None,
            late: Persist::default(),
            ahead: Persist::default(),
            since: None,
            stated: Vec::new(),
            readings: Vec::new(),
            stopped: None,
            raw: None,
            raw_logged: None,
            raw_unlogged: 0,
        }
    }

    /// Each rig's own line now, with whether it is young; `restart`: the minutes start a new
    /// series, and every line's time starts with it.
    fn lines(&mut self, e: &Evidence<'_>, restart: bool, now: Duration) -> Vec<Reading> {
        if restart {
            self.stated.clear();
        }
        let mut out = Vec::new();
        // A rig whose clocks are not heard keeps its time: nothing says its line broke.
        for (rig, c) in [Some(e.ours), e.theirs.1.map(|c| (e.theirs.0, c))]
            .into_iter()
            .flatten()
        {
            let Ok(Some(ppm)) = own_line(rig, c) else {
                self.stated.retain(|(r, _)| *r != rig);
                continue;
            };
            let since = match self.stated.iter().find(|(r, _)| *r == rig) {
                Some((_, t)) => *t,
                None => {
                    self.stated.push((rig, now));
                    now
                }
            };
            out.push(Reading {
                rig,
                ppm,
                young: now.saturating_sub(since) < LINE_SETTLED,
            });
        }
        out
    }

    /// Keeps each rig's worst reading over the open minute.
    fn keep(&mut self, now: &[Reading]) {
        for r in now {
            let r = Reading {
                ppm: hundredths(r.ppm),
                ..*r
            };
            match self.readings.iter_mut().find(|x| x.rig == r.rig) {
                Some(x) if r.ppm.abs() > x.ppm.abs() => *x = r,
                Some(_) => {}
                None => self.readings.push(r),
            }
        }
    }

    /// The judge's relation to log at `now`: a change, at most once per [`RAW_LOG_EVERY`];
    /// changes held back are logged with their count once that has passed.
    fn raw_log(&mut self, judged: PeerRelation, now: Duration) -> Option<(PeerRelation, u32)> {
        let changed = self.raw.is_none_or(|was| !same_raw(&was, &judged));
        if !changed && self.raw_unlogged == 0 {
            return None;
        }
        let due = self
            .raw_logged
            .is_none_or(|t| now.saturating_sub(t) >= RAW_LOG_EVERY);
        if !due {
            self.raw_unlogged += u32::from(changed);
            if changed {
                self.raw = Some(judged);
            }
            return None;
        }
        let held = std::mem::take(&mut self.raw_unlogged);
        self.raw = Some(judged);
        self.raw_logged = Some(now);
        Some((judged, held))
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
        let event = Event::of(&mut self.seen, minutes);
        let lines = self.lines(&e, event == Event::Restart, now);
        let line = lines
            .iter()
            .filter(|r| !r.young)
            .map(|r| Line {
                rig: r.rig,
                ppm: r.ppm,
            })
            .max_by(|a, b| a.ppm.abs().total_cmp(&b.ppm.abs()));
        let young_beyond = lines.iter().any(|r| r.young && r.ppm.abs() > BOUND_PPM);
        let judged = quantise(judge(&e));
        let raw = Raw::of(judged);
        let raw_changed = self.raw_log(judged, now);
        if matches!(raw, Raw::Agree | Raw::NotAgree) {
            self.keep(&lines);
        }
        let input = Input {
            raw,
            event,
            live: e.long,
            line,
            young_beyond,
            now,
        };
        let relation = self.hold.step(input);
        let closed = match event {
            Event::Close(cross) => Some(Closed {
                cross: cross.map(hundredths),
                lines: std::mem::take(&mut self.readings),
                classed: self.hold.classed == Some(true),
                raw: judged,
            }),
            Event::Gap | Event::Restart => {
                self.readings.clear();
                None
            }
            Event::None => None,
        };
        // What the judge cannot tell from drops the count and the open minute: the marks
        // counted begin after it, though the windows kept them.
        if matches!(raw, Raw::Stop(_)) {
            self.stopped = Some(now);
            self.readings.clear();
        }
        let counted = e
            .streaming_for
            .filter(|_| !matches!(raw, Raw::Stop(_)))
            .map(|d| match self.stopped {
                Some(t) => d.min(now.saturating_sub(t)),
                None => d,
            });
        // Kept through a gap in the reports: marks that come back to the same count land on
        // the same time and do not move it.
        if let Some(d) = counted {
            self.since = Some(place_since(self.since, wall, d));
        }
        let figure = self.hold.figure;
        Shown {
            relation: quantise(relation),
            ppm: figure.map(|f| f.0),
            span: figure.map(|f| Seconds(f.1)),
            counting: self.hold.counting(),
            marks_since: counted.and(self.since),
            raw_changed,
            closed,
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
                CantTellReason::NotSteady { low, high, line } => CantTellReason::NotSteady {
                    low: hundredths(low),
                    high: hundredths(high),
                    line,
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
