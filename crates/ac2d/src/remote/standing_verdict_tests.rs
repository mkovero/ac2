#![allow(clippy::unwrap_used)]

use super::super::standing::Minutes;
use super::*;

/// A report whose marks are `least_age` s old, devices on 64-frame periods at 48 kHz.
fn aged(least_age: f64) -> StandingReport {
    StandingReport {
        long: None,
        short: None,
        marks_for: None,
        marks_since: None,
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
    let mut l = Late::default();
    let s = Duration::from_secs;
    assert_eq!(l.update(Some(0.2), s(10)), None);
    assert_eq!(l.update(Some(0.2), s(12)), None);
    assert_eq!(l.update(Some(0.25), s(13)), Some(0.25));
    // Clean for a moment: still shown, with the last figure.
    assert_eq!(l.update(None, s(14)), Some(0.25));
    assert_eq!(l.update(Some(0.21), s(15)), Some(0.21));
    assert_eq!(l.update(None, s(16)), Some(0.21));
    assert_eq!(l.update(None, s(18)), Some(0.21));
    assert_eq!(l.update(None, s(19)), None);
    // At the bound, flickering: never in.
    for k in 20..40 {
        let by = (k % 2 == 0).then_some(0.001);
        assert_eq!(l.update(by, s(k)), None);
    }
}

fn minutes(closed: u64, last: Option<f64>) -> Option<Minutes> {
    Some(Minutes {
        series: 1,
        closed,
        last,
    })
}

/// A tally fed minutes of `ppm` (judged as `rel`), one per call, from minute `from`.
fn feed(t: &mut Tally, rel: PeerRelation, from: u64, ppm: &[f64]) {
    for (k, p) in ppm.iter().enumerate() {
        t.note(&rel, minutes(from + k as u64, Some(*p)));
    }
}

fn started() -> Tally {
    let mut t = Tally::default();
    t.note(&PeerRelation::Measuring, minutes(0, None));
    t
}

#[test]
fn a_minute_within_agrees_and_one_beyond_counts() {
    let mut t = started();
    assert_eq!(t.shown(PeerRelation::Agree).0, PeerRelation::Measuring);
    feed(&mut t, PeerRelation::Agree, 1, &[0.3]);
    assert_eq!(t.shown(PeerRelation::Agree), (PeerRelation::Agree, None));
    feed(&mut t, PeerRelation::Agree, 2, &[2.6]);
    assert_eq!(
        t.shown(PeerRelation::Agree),
        (
            PeerRelation::Measuring,
            Some(MinuteCount { minutes: 1, of: 10 })
        )
    );
}

#[test]
fn ten_minutes_of_one_sign_are_not_locked_mixed_ones_not_steady() {
    let apart = |ppm: f64| PeerRelation::NotLocked {
        reason: NotLockedReason::CrossRate {
            ppm,
            over: Seconds(60.0),
        },
    };
    let mut t = started();
    feed(&mut t, apart(3.0), 1, &[3.0; 9]);
    assert_eq!(t.shown(apart(3.0)).1.unwrap().minutes, 9);
    feed(&mut t, apart(3.1), 10, &[3.1]);
    assert_eq!(t.shown(apart(3.1)), (apart(3.1), None));

    // One sign changed among them: noise about zero, not a rate.
    let mut t = started();
    feed(
        &mut t,
        apart(3.0),
        1,
        &[3.0, -2.5, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0],
    );
    let not_steady = |low, high| PeerRelation::CantTell {
        reason: CantTellReason::NotSteady { low, high },
    };
    assert_eq!(t.shown(apart(3.0)).0, not_steady(-2.5, 3.0));

    // Beyond and within in turn: counted to the end, then not steady with the spread.
    let mut t = started();
    feed(&mut t, PeerRelation::Measuring, 1, &[3.1, 0.4].repeat(5));
    assert_eq!(t.shown(PeerRelation::Agree).0, not_steady(0.4, 3.1));
    // Two minutes within in a row end the count: agreeing again.
    feed(&mut t, PeerRelation::Agree, 11, &[0.2, 0.3]);
    assert_eq!(t.shown(PeerRelation::Agree), (PeerRelation::Agree, None));
}

#[test]
fn a_rigs_own_line_beyond_the_rule_is_held_like_the_rate() {
    let own = |ppm| PeerRelation::NotLocked {
        reason: NotLockedReason::OwnClock { rig: RigId(2), ppm },
    };
    let mut t = started();
    feed(&mut t, own(4.0), 1, &[0.1; 9]);
    assert_eq!(
        t.shown(own(4.0)),
        (
            PeerRelation::Measuring,
            Some(MinuteCount { minutes: 9, of: 10 })
        )
    );
    feed(&mut t, own(4.0), 10, &[0.1]);
    assert_eq!(t.shown(own(4.0)).0, own(4.0));
}

#[test]
fn what_the_judge_cannot_tell_or_a_thin_minute_ends_the_count() {
    let mut t = started();
    feed(&mut t, PeerRelation::Measuring, 1, &[3.0; 5]);
    t.note(
        &PeerRelation::CantTell {
            reason: CantTellReason::SystemClockUnsynced { rig: RigId(2) },
        },
        minutes(5, Some(3.0)),
    );
    assert_eq!(
        t.shown(PeerRelation::Measuring),
        (PeerRelation::Measuring, None)
    );

    let mut t = started();
    feed(&mut t, PeerRelation::Measuring, 1, &[3.0; 5]);
    t.note(&PeerRelation::Measuring, minutes(6, None));
    assert_eq!(
        t.shown(PeerRelation::Measuring),
        (PeerRelation::Measuring, None)
    );

    // Only missing marks: the count stands.
    let mut t = started();
    feed(&mut t, PeerRelation::Measuring, 1, &[3.0; 5]);
    t.note(
        &PeerRelation::CantTell {
            reason: CantTellReason::NoMarks,
        },
        None,
    );
    assert_eq!(t.shown(PeerRelation::Measuring).1.unwrap().minutes, 5);
    // A new series is a new count.
    t.note(
        &PeerRelation::Measuring,
        Some(Minutes {
            series: 2,
            closed: 0,
            last: None,
        }),
    );
    assert_eq!(
        t.shown(PeerRelation::Measuring),
        (PeerRelation::Measuring, None)
    );
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
