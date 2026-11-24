//! What the standing feed may show (`docs/design/remote-auto.md` §5.1): the judge's relation,
//! held by the whole minutes of marks behind it.
//!
//! The feed runs always, on hosts whose system clocks are slewed (chrony corrects an offset
//! by running the clock a little fast or slow for a while), and a slew reads in a minute's
//! fit as a rate: 100 µs over a minute is about 2 ppm, and the same slew moves a rig's own
//! line against its system clock. A slew passes, a rate difference stays. So the two
//! verdicts are held unequally:
//!
//! - *agree* once the newest whole minute is within the rule, as the audio-fed relation
//!   says it: a slew only ever makes a locked pair look apart, never an apart pair look
//!   locked for a minute running;
//! - *not locked* (the cross-rig rate or either rig's own line) only after
//!   [`HOLD_MINUTES`] whole minutes in a row beyond the rule, all on one side with one sign;
//! - ten minutes counted and not of one mind: *can't tell*, the figure not steady, with its
//!   spread; never *measuring* for ever.
//!
//! A count begins at a minute beyond the rule and ends at two minutes in a row within it; a
//! single minute within between minutes beyond is the noise near the bound that the
//! not-steady verdict is for.

use std::collections::VecDeque;
use std::time::Duration;

use ac2_proto::model::{CantTellReason, MinuteCount, NotLockedReason, PeerRelation};
use ac2_proto::units::{RigId, Seconds, WallNs};

use super::peer_clock::{BOUND_PPM, Evidence, PeerClockWatch};
use super::standing::{Minutes, StandingReport};
use super::{MAX_LINK_OFFSET_S, link_offset};

/// Whole minutes in a row a *not locked* verdict needs. chrony slews a measured offset over
/// about three of its update intervals; to read past 2 ppm through ten minutes running, one
/// correction would have to move the clock 1.2 ms or more at an update interval of 256 s or
/// longer, and chrony lengthens its interval only while the offsets it measures stay far
/// below that. A step (`maxerror` and the kernel's unsync flag) is the system clock checks'
/// to catch, not this.
pub(crate) const HOLD_MINUTES: usize = 10;
/// Marks late for this long before they say so, and clean for this long before they stop:
/// a link's queue that fills for a moment is not a clock behind, and one at the bound must
/// not flip the relation every second.
pub(crate) const LATE_IN: Duration = Duration::from_secs(3);
pub(crate) const LATE_OUT: Duration = Duration::from_secs(3);

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
}

/// The minutes behind the verdict.
#[derive(Debug, Default)]
struct Tally {
    /// The feed's series and minutes closed as last seen.
    seen: Option<(u64, u64)>,
    last: Option<Minute>,
    /// Since the count began, the newest [`HOLD_MINUTES`]; empty: not counting.
    count: VecDeque<Minute>,
}

impl Tally {
    fn clear(&mut self) {
        self.last = None;
        self.count.clear();
    }

    /// The relation the watch holds now, `rel`, and the feed's minutes.
    fn note(&mut self, rel: &PeerRelation, minutes: Option<Minutes>) {
        match rel {
            // What the judge cannot tell, the minutes before it did not show either; what
            // is only missing (no marks, no link) leaves them standing.
            PeerRelation::CantTell { reason } if !only_missing(reason) => self.clear(),
            PeerRelation::NotLocked {
                reason: NotLockedReason::OtherRate { .. },
            } => self.clear(),
            _ => {}
        }
        let Some(m) = minutes else {
            return;
        };
        let Some((series, closed)) = self.seen.filter(|(s, _)| *s == m.series) else {
            self.clear();
            self.seen = Some((m.series, m.closed));
            return;
        };
        if m.closed == closed {
            return;
        }
        self.seen = Some((series, m.closed));
        let Some(p) = m.last else {
            // A minute too thin to fit breaks the run.
            self.clear();
            return;
        };
        let minute = match rel {
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
        let within = matches!(minute, Minute::Within(_));
        if within && matches!(self.last, Some(Minute::Within(_))) {
            self.count.clear();
        } else if !within || !self.count.is_empty() {
            if self.count.len() == HOLD_MINUTES {
                self.count.pop_front();
            }
            self.count.push_back(minute);
        }
        self.last = Some(minute);
    }

    /// The relation shown for the watch's `rel`, and the count toward a verdict.
    fn shown(&self, rel: PeerRelation) -> (PeerRelation, Option<MinuteCount>) {
        if matches!(
            rel,
            PeerRelation::CantTell { .. }
                | PeerRelation::NotLocked {
                    reason: NotLockedReason::OtherRate { .. }
                }
        ) {
            return (rel, None);
        }
        if self.count.len() == HOLD_MINUTES {
            let sides = self.count.iter().map(|m| match m {
                Minute::Beyond(s, _) => Some(*s),
                Minute::Within(_) => None,
            });
            let first = self.count.front().and_then(|m| match m {
                Minute::Beyond(s, _) => Some(*s),
                Minute::Within(_) => None,
            });
            let last = self.count.back().map_or(0.0, |m| m.ppm());
            if let Some(side) = first
                && sides.into_iter().all(|s| s == Some(side))
            {
                let reason = match side {
                    Side::Cross { .. } => NotLockedReason::CrossRate {
                        ppm: last,
                        over: Seconds(60.0),
                    },
                    Side::Own { rig, .. } => NotLockedReason::OwnClock { rig, ppm: last },
                };
                return (PeerRelation::NotLocked { reason }, None);
            }
            let ppms = || self.count.iter().map(|m| m.ppm());
            let reason = CantTellReason::NotSteady {
                low: ppms().fold(f64::INFINITY, f64::min),
                high: ppms().fold(f64::NEG_INFINITY, f64::max),
            };
            return (PeerRelation::CantTell { reason }, None);
        }
        let within = matches!(self.last, Some(Minute::Within(_)));
        if within && rel == PeerRelation::Agree {
            return (PeerRelation::Agree, None);
        }
        let counting = (!within && !self.count.is_empty()).then_some(MinuteCount {
            minutes: self.count.len() as u32,
            of: HOLD_MINUTES as u32,
        });
        (PeerRelation::Measuring, counting)
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

/// Late marks with a persistence in and out.
#[derive(Debug, Default)]
struct Late {
    over_since: Option<Duration>,
    clean_since: Option<Duration>,
    by: Option<f64>,
}

impl Late {
    /// The lateness at `now`, `by`; what is shown.
    fn update(&mut self, by: Option<f64>, now: Duration) -> Option<f64> {
        match by {
            Some(b) => {
                self.clean_since = None;
                let since = *self.over_since.get_or_insert(now);
                if self.by.is_some() || now.saturating_sub(since) >= LATE_IN {
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
/// the late marks' persistence.
#[derive(Debug)]
pub(crate) struct StandingWatch {
    pub(crate) watch: PeerClockWatch,
    tally: Tally,
    late: Late,
}

/// What a standing entry shows: figures to 0.01 ppm, whole seconds and whole milliseconds,
/// so it changes only when a shown digit does.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Shown {
    pub(crate) relation: PeerRelation,
    pub(crate) ppm: Option<f64>,
    pub(crate) span: Option<Seconds>,
    pub(crate) counting: Option<MinuteCount>,
}

impl StandingWatch {
    pub(crate) fn new() -> Self {
        Self {
            watch: PeerClockWatch::absent(CantTellReason::LinkDown),
            tally: Tally::default(),
            late: Late::default(),
        }
    }

    /// Judges `e` (its `late` before persistence) at `now` (time since the daemon's start)
    /// and wall time `wall`, with the feed's `minutes`.
    pub(crate) fn update(
        &mut self,
        mut e: Evidence<'_>,
        minutes: Option<Minutes>,
        now: Duration,
        wall: WallNs,
    ) -> Shown {
        e.late = self.late.update(e.late, now);
        self.watch.update(&e, now, wall);
        let rel = self.watch.clock.relation;
        self.tally.note(&rel, minutes);
        let (relation, counting) = self.tally.shown(rel);
        Shown {
            relation: quantise(relation),
            ppm: self.watch.clock.ppm.map(hundredths),
            span: self.watch.clock.span.map(|s| Seconds(s.0.round())),
            counting,
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
