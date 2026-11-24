#![allow(clippy::unwrap_used)]

use super::super::standing::Minutes;
use super::*;

/// A report whose marks are `least_age` s old, devices on 64-frame periods at 48 kHz.
fn aged(least_age: f64) -> StandingReport {
    StandingReport {
        long: None,
        short: None,
        marks_for: None,
        ahead: None,
        least_age: Some(least_age),
        allow: 0.004,
        periods: (64.0 / 48_000.0, 64.0 / 48_000.0),
        minutes: None,
        absent: None,
        other_rate: None,
        levels: None,
    }
}

#[test]
fn late_is_measured_past_the_offset_in_force_else_the_longest() {
    // Set to 100 ms: 300 ms old is 196 ms past it and the allowance.
    let by = late(&aged(0.300), Some(0.100)).unwrap();
    assert!((by - 0.196).abs() < 1e-9, "{by}");
    assert_eq!(late(&aged(0.090), Some(0.100)), None);
    // None set: only past the longest offset a link may run.
    assert_eq!(late(&aged(0.300), None), None);
    let by = late(&aged(0.700), None).unwrap();
    assert!((by - 0.196).abs() < 1e-9, "{by}");
}

#[test]
fn late_marks_say_so_only_after_a_while_and_stop_only_after_a_while_clean() {
    let mut l = Persist::default();
    let s = Duration::from_secs;
    assert_eq!(l.update(Some(0.2), s(10)), None);
    assert_eq!(l.update(Some(0.2), s(12)), None);
    assert_eq!(l.update(Some(0.25), s(13)), Some(0.25));
    // A few ms either way is the same lateness.
    assert_eq!(l.update(Some(0.254), s(14)), Some(0.25));
    // Clean for a moment: still shown, with the last figure.
    assert_eq!(l.update(None, s(15)), Some(0.25));
    assert_eq!(l.update(Some(0.21), s(16)), Some(0.21));
    assert_eq!(l.update(None, s(17)), Some(0.21));
    assert_eq!(l.update(None, s(19)), Some(0.21));
    assert_eq!(l.update(None, s(20)), None);
    // At the bound, flickering: never in.
    for k in 21..40 {
        let by = (k % 2 == 0).then_some(0.001);
        assert_eq!(l.update(by, s(k)), None);
    }
}

/// A tally fed whole minutes as the feed closes them.
struct Run {
    t: Tally,
    holes: u64,
    closed: u64,
}

const AGREE: PeerRelation = PeerRelation::Agree;
const MEASURING: PeerRelation = PeerRelation::Measuring;

fn apart(ppm: f64) -> PeerRelation {
    PeerRelation::NotLocked {
        reason: NotLockedReason::CrossRate {
            ppm,
            over: Seconds(60.0),
        },
    }
}

fn not_steady(low: f64, high: f64) -> PeerRelation {
    PeerRelation::CantTell {
        reason: CantTellReason::NotSteady { low, high },
    }
}

fn count(beyond: u32, counted: u32) -> Option<MinuteCount> {
    Some(MinuteCount {
        beyond,
        counted,
        needed: HOLD_MINUTES as u32,
    })
}

impl Run {
    fn new() -> Self {
        let mut t = Tally::default();
        t.note(&MEASURING, Some(Self::at(0, 0, None)));
        Self {
            t,
            holes: 0,
            closed: 0,
        }
    }

    fn at(holes: u64, closed: u64, last: Option<f64>) -> Minutes {
        Minutes {
            series: 1,
            holes,
            closed,
            last,
        }
    }

    /// A minute closes at `ppm`, the judge reading `raw` and the watch holding `held`.
    fn close_as(
        &mut self,
        raw: PeerRelation,
        held: PeerRelation,
        ppm: f64,
    ) -> (PeerRelation, Option<MinuteCount>) {
        self.closed += 1;
        assert!(
            self.t
                .note(&raw, Some(Self::at(self.holes, self.closed, Some(ppm))))
        );
        self.t.shown(held)
    }

    /// A minute closes at `ppm` with the judge and its watch agreeing it is so.
    fn close(&mut self, ppm: f64) -> (PeerRelation, Option<MinuteCount>) {
        let rel = if ppm.abs() > BOUND_PPM {
            apart(ppm)
        } else {
            AGREE
        };
        self.close_as(rel, rel, ppm)
    }

    fn closes(&mut self, ppm: &[f64]) -> (PeerRelation, Option<MinuteCount>) {
        ppm.iter().map(|p| self.close(*p)).last().unwrap()
    }

    /// A second between closes, the watch holding `held`.
    fn between(&mut self, held: PeerRelation) -> (PeerRelation, Option<MinuteCount>) {
        self.t
            .note(&held, Some(Self::at(self.holes, self.closed, Some(0.0))));
        self.t.shown(held)
    }
}

#[test]
fn the_first_minute_within_agrees_one_beyond_starts_a_count() {
    let mut r = Run::new();
    assert_eq!(r.between(AGREE), (MEASURING, None));
    assert_eq!(r.close(0.3), (AGREE, None));
    assert_eq!(r.close(2.6), (MEASURING, count(1, 1)));
}

#[test]
fn a_count_says_its_minutes_truthfully_and_agrees_only_once_it_ends() {
    let mut r = Run::new();
    r.close(0.3);
    assert_eq!(r.closes(&[2.6, 0.4]), (MEASURING, count(1, 2)));
    assert_eq!(r.close(2.7), (MEASURING, count(2, 3)));
    // Two minutes in a row within: the count ends, agreement again.
    assert_eq!(r.close(0.4), (MEASURING, count(2, 4)));
    assert_eq!(r.close(0.2), (AGREE, None));
}

#[test]
fn agreement_leaves_above_the_bound_and_comes_back_below_a_margin() {
    let mut r = Run::new();
    r.close(0.3);
    // Within the rule, near it: still agreeing.
    assert_eq!(r.close(1.9), (AGREE, None));
    // Counting: minutes between the margin and the bound do not end it.
    r.close(2.5);
    assert_eq!(r.closes(&[1.9, 1.9]), (MEASURING, count(1, 3)));
    assert_eq!(r.closes(&[1.0, 1.0]), (AGREE, None));
}

#[test]
fn ten_minutes_of_one_sign_are_not_locked_and_leave_through_not_steady() {
    let mut r = Run::new();
    assert_eq!(r.closes(&[3.0; 9]), (MEASURING, count(9, 9)));
    assert_eq!(r.close(3.1), (apart(3.1), None));
    // One minute within: the ten are no longer of one mind.
    assert_eq!(r.close(0.5).0, not_steady(0.5, 3.1));
    // A second: the count ends; the watch still holds not locked, so measuring until it
    // lets go.
    assert_eq!(r.close_as(AGREE, apart(3.0), 0.4), (MEASURING, None));
    assert_eq!(r.between(apart(3.0)), (MEASURING, None));
    assert_eq!(r.between(AGREE), (AGREE, None));
}

#[test]
fn minutes_beyond_but_of_both_signs_or_sides_are_not_steady() {
    let mut r = Run::new();
    r.closes(&[3.0, -2.5, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0]);
    assert_eq!(r.close(3.0).0, not_steady(-2.5, 3.0));

    let mut r = Run::new();
    r.close(0.2);
    assert_eq!(r.closes(&[3.1, 0.4].repeat(5)).0, not_steady(0.4, 3.1));
}

#[test]
fn between_closes_nothing_shown_moves_with_the_live_figure() {
    let mut r = Run::new();
    r.close(0.2);
    // The short window reads 3.5 ppm and the watch says not locked: still agreeing.
    assert_eq!(r.between(apart(3.5)), (AGREE, None));
    r.close(2.6);
    assert_eq!(r.between(AGREE), (MEASURING, count(1, 1)));
}

#[test]
fn a_long_hole_starts_the_count_and_the_agreement_again() {
    let mut r = Run::new();
    r.closes(&[3.0; 5]);
    r.holes += 1;
    assert_eq!(r.closes(&[3.0; 5]), (MEASURING, count(5, 5)));

    let mut r = Run::new();
    r.close(0.2);
    r.holes += 1;
    assert_eq!(r.between(AGREE), (MEASURING, None));
    // The minute closing after the hole is the first of the new count: it is what agrees.
    r.closed += 1;
    r.t.note(&AGREE, Some(Run::at(r.holes, r.closed, Some(0.1))));
    assert_eq!(r.t.shown(AGREE), (AGREE, None));
}

#[test]
fn an_own_line_held_by_the_watch_counts_as_the_judge_reads_it() {
    let own = |ppm| PeerRelation::NotLocked {
        reason: NotLockedReason::OwnClock { rig: RigId(2), ppm },
    };
    let mut r = Run::new();
    for _ in 0..9 {
        r.close_as(own(4.0), own(4.0), 0.1);
    }
    assert_eq!(r.t.shown(own(4.0)), (MEASURING, count(9, 9)));
    // The line is back within; the watch still holds it beyond for its minute.
    assert_eq!(r.close_as(AGREE, own(4.0), 0.1).0, not_steady(0.1, 4.0));
    let mut r = Run::new();
    for _ in 0..10 {
        r.close_as(own(4.0), own(4.0), 0.1);
    }
    assert_eq!(r.t.shown(own(4.0)).0, own(4.0));
}

#[test]
fn what_the_judge_cannot_tell_or_a_thin_minute_ends_the_count() {
    let mut r = Run::new();
    r.closes(&[3.0; 5]);
    r.t.note(
        &PeerRelation::CantTell {
            reason: CantTellReason::ClocksDisagree { by: Seconds(0.05) },
        },
        None,
    );
    assert_eq!(r.t.shown(MEASURING), (MEASURING, None));

    let mut r = Run::new();
    r.closes(&[3.0; 5]);
    r.closed += 1;
    r.t.note(&MEASURING, Some(Run::at(0, r.closed, None)));
    assert_eq!(r.t.shown(MEASURING), (MEASURING, None));

    // Only missing marks: the count stands.
    let mut r = Run::new();
    r.closes(&[3.0; 5]);
    r.t.note(
        &PeerRelation::CantTell {
            reason: CantTellReason::NoMarks,
        },
        None,
    );
    assert_eq!(r.t.shown(MEASURING), (MEASURING, count(5, 5)));
}

#[test]
fn shown_figures_change_only_with_a_shown_digit() {
    assert_eq!(hundredths(0.123_456), 0.12);
    assert_eq!(hundredths(-0.001).to_string(), "0");
    assert_eq!(
        quantise(PeerRelation::CantTell {
            reason: CantTellReason::MarksLate {
                by: Seconds(0.250_4)
            }
        }),
        PeerRelation::CantTell {
            reason: CantTellReason::MarksLate { by: Seconds(0.25) }
        }
    );
}
