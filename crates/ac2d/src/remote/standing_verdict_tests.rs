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

#[test]
fn marks_since_is_placed_again_only_when_more_than_two_seconds_off() {
    // 2026-11-24 around 03:00 UTC, as a host reads it.
    const T: u64 = 1_795_489_200_000_000_000;
    const S: u64 = 1_000_000_000;
    let d = Duration::from_secs_f64;
    let first = place_since(None, WallNs(T), d(300.0));
    assert_eq!(first, WallNs(T - 300 * S));
    // A second later, the pairing read 400 ms long: the same time.
    assert_eq!(place_since(Some(first), WallNs(T + S), d(301.4)), first);
    // The wall clock stepped 1.5 s, either way: kept.
    assert_eq!(
        place_since(Some(first), WallNs(T + 3 * S / 2), d(300.0)),
        first
    );
    assert_eq!(
        place_since(Some(first), WallNs(T - 3 * S / 2), d(300.0)),
        first
    );
    // Two 1.5 s steps the same way: kept after the first, placed again after the second.
    let once = place_since(Some(first), WallNs(T + 3 * S / 2), d(300.0));
    assert_eq!(once, first);
    assert_eq!(
        place_since(Some(once), WallNs(T + 3 * S), d(300.0)),
        WallNs(T + 3 * S - 300 * S)
    );
    // Exactly 2 s off is not more than 2 s; 2 s and a ns is.
    assert_eq!(place_since(Some(first), WallNs(T + 2 * S), d(300.0)), first);
    assert_eq!(
        place_since(Some(first), WallNs(T - 2 * S - 1), d(300.0)),
        WallNs(T - 2 * S - 1 - 300 * S)
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

#[test]
fn minutes_are_read_as_one_event_a_report() {
    let m = |gaps, holes, closed| {
        Some(Minutes {
            series: 1,
            gaps,
            holes,
            closed,
            last: Some(0.5),
        })
    };
    let mut seen = None;
    assert_eq!(Event::of(&mut seen, m(0, 0, 0)), Event::Restart);
    assert_eq!(Event::of(&mut seen, m(0, 0, 0)), Event::None);
    assert_eq!(Event::of(&mut seen, None), Event::None);
    assert_eq!(Event::of(&mut seen, m(0, 0, 1)), Event::Close(Some(0.5)));
    assert_eq!(Event::of(&mut seen, m(1, 0, 1)), Event::Gap);
    // A gap and a close together: the gap.
    assert_eq!(Event::of(&mut seen, m(2, 0, 2)), Event::Gap);
    // A long hole and a close together: the minute closed before the hole is not counted.
    assert_eq!(Event::of(&mut seen, m(3, 1, 3)), Event::Restart);
    assert_eq!(Event::of(&mut seen, m(3, 1, 4)), Event::Close(Some(0.5)));
}

// The hold itself, a report at a time.

const LIVE: Option<(f64, f64)> = Some((0.5, 60.0));

fn tick(raw: Raw, live: Option<(f64, f64)>) -> Input {
    Input {
        raw,
        event: Event::None,
        live,
    }
}

fn close(raw: Raw, p: f64) -> Input {
    Input {
        raw,
        event: Event::Close(Some(p)),
        live: Some((p, 60.0)),
    }
}

fn agree(p: f64) -> Input {
    close(Raw::Agree, p)
}

fn apart(p: f64) -> Input {
    close(Raw::NotAgree, p)
}

fn own(ppm: f64, cross: f64) -> Input {
    close(Raw::Own { rig: RigId(2), ppm }, cross)
}

fn thin() -> Input {
    Input {
        raw: Raw::NotAgree,
        event: Event::Close(None),
        live: None,
    }
}

fn link_down() -> Raw {
    Raw::Missing(PeerRelation::CantTell {
        reason: CantTellReason::LinkDown,
    })
}

fn marks_late() -> Raw {
    Raw::Stop(PeerRelation::CantTell {
        reason: CantTellReason::MarksLate { by: Seconds(0.25) },
    })
}

fn event(raw: Raw, event: Event) -> Input {
    Input {
        raw,
        event,
        live: LIVE,
    }
}

/// Short text for what is shown: `A`, `M` (with its count `M b/c`), `NL ppm`, `NLo ppm`,
/// `NS low..high`, or the reason.
fn code(h: &Hold, r: PeerRelation) -> String {
    match r {
        PeerRelation::Agree => "A".into(),
        PeerRelation::Measuring => match h.counting() {
            Some(c) => format!("M {}/{}", c.beyond, c.counted),
            None => "M".into(),
        },
        PeerRelation::NotLocked {
            reason: NotLockedReason::CrossRate { ppm, .. },
        } => format!("NL {ppm:.2}"),
        PeerRelation::NotLocked {
            reason: NotLockedReason::OwnClock { ppm, .. },
        } => format!("NLo {ppm:.2}"),
        PeerRelation::CantTell {
            reason: CantTellReason::NotSteady { low, high },
        } => format!("NS {low:.2}..{high:.2}"),
        PeerRelation::CantTell {
            reason: CantTellReason::LinkDown,
        } => "LD".into(),
        PeerRelation::CantTell {
            reason: CantTellReason::MarksLate { .. },
        } => "LATE".into(),
        other => format!("{other:?}"),
    }
}

/// The codes shown after each of `inputs`, from `h`.
fn walk(h: &mut Hold, inputs: &[Input]) -> Vec<String> {
    inputs
        .iter()
        .map(|i| {
            let r = h.step(*i);
            code(h, r)
        })
        .collect()
}

fn from(inputs: &[Input]) -> Hold {
    let mut h = Hold::default();
    walk(&mut h, inputs);
    h
}

fn last(inputs: &[Input]) -> String {
    walk(&mut Hold::default(), inputs).pop().unwrap()
}

/// Every state the hold can be in, against every kind of report: what is shown.
#[test]
fn every_state_meets_every_input() {
    let nine_within = {
        let mut v = vec![apart(2.5)];
        v.extend([agree(1.9); 8]);
        v
    };
    let states: [(&str, Vec<Input>); 8] = [
        ("fresh", vec![]),
        ("agreeing", vec![agree(0.5)]),
        ("within, judge not yet", vec![apart(0.5)]),
        ("counting", vec![agree(0.5), apart(3.0)]),
        ("not locked", vec![apart(3.0); 10]),
        (
            "waiting a close",
            vec![agree(0.5), tick(link_down(), None), tick(Raw::Agree, LIVE)],
        ),
        ("discarding", vec![agree(0.5), tick(marks_late(), None)]),
        ("nine within after one beyond", nine_within),
    ];
    let inputs: [(&str, Input); 13] = [
        ("tick, judge agrees", tick(Raw::Agree, LIVE)),
        ("tick, live 2.6", tick(Raw::Agree, Some((2.6, 60.0)))),
        ("tick, judge does not agree", tick(Raw::NotAgree, LIVE)),
        ("close 0.5, judge agrees", agree(0.5)),
        ("close 1.9, judge agrees", agree(1.9)),
        ("close 3.0", apart(3.0)),
        ("close 0.5, judge does not agree", apart(0.5)),
        ("close 0.5, own line 4.0", own(4.0, 0.5)),
        ("close thin", thin()),
        ("gap", event(Raw::Agree, Event::Gap)),
        ("restart", event(Raw::Agree, Event::Restart)),
        ("link down", tick(link_down(), None)),
        ("marks late", tick(marks_late(), None)),
    ];
    #[rustfmt::skip]
    let expected: [[&str; 13]; 8] = [
        ["M", "M", "M", "A", "A", "M 1/1", "M", "M 1/1", "M", "M", "M", "LD", "LATE"],
        ["A", "M", "A", "A", "A", "M 1/1", "M", "M 1/1", "M", "M", "M", "LD", "LATE"],
        ["A", "M", "M", "A", "A", "M 1/1", "M", "M 1/1", "M", "M", "M", "LD", "LATE"],
        ["M 1/1", "M 1/1", "M 1/1", "M 1/2", "M 1/2", "M 2/2", "M 1/2", "M 2/2", "M",
         "M 1/1", "M", "LD", "LATE"],
        ["NL 3.00", "NL 3.00", "NL 3.00", "NS 0.50..3.00", "NS 1.90..3.00", "NL 3.00",
         "NS 0.50..3.00", "NS 3.00..3.00", "M", "NL 3.00", "M", "LD", "LATE"],
        ["M", "M", "M", "A", "A", "M 1/1", "M", "M 1/1", "M", "M", "M", "LD", "LATE"],
        ["M", "M", "M", "M", "M", "M", "M", "M", "M", "M", "M", "LD", "LATE"],
        ["M 1/9", "M 1/9", "M 1/9", "NS 0.50..2.50", "NS 1.90..2.50", "NS 1.90..3.00",
         "NS 0.50..2.50", "NS 1.90..2.50", "M", "M 1/9", "M", "LD", "LATE"],
    ];
    let mut wrong = Vec::new();
    for ((state, path), row) in states.iter().zip(expected) {
        for ((input, i), want) in inputs.iter().zip(row) {
            let mut h = from(path);
            let r = h.step(*i);
            let got = code(&h, r);
            if got != want {
                wrong.push(format!("{state} × {input}: {got}, not {want}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn a_count_says_its_minutes_and_agrees_only_once_it_ends() {
    // B, W: counting, one of two beyond; B, W, B: two of three.
    assert_eq!(last(&[agree(0.3), apart(2.6), agree(0.4)]), "M 1/2");
    assert_eq!(
        last(&[agree(0.3), apart(2.6), agree(0.4), apart(2.7)]),
        "M 2/3"
    );
    // Two in a row below the margin end it.
    assert_eq!(
        walk(
            &mut Hold::default(),
            &[agree(0.3), apart(2.6), agree(0.4), agree(0.2)]
        ),
        ["A", "M 1/1", "M 1/2", "A"]
    );
}

#[test]
fn the_rule_and_the_margin_are_where_they_say() {
    // 2.00 is within the rule, 2.01 beyond it.
    assert_eq!(last(&[agree(0.3), agree(2.0)]), "A");
    assert_eq!(last(&[agree(0.3), apart(2.01)]), "M 1/1");
    // 1.79 is below the margin, 1.81 not.
    assert_eq!(last(&[apart(2.5), agree(1.79), agree(1.79)]), "A");
    assert_eq!(last(&[apart(2.5), agree(1.81), agree(1.81)]), "M 1/3");
    assert_eq!(last(&[apart(2.5), agree(1.79), agree(1.81)]), "M 1/3");
}

#[test]
fn ten_within_the_rule_end_a_count() {
    let mut inputs = vec![agree(0.3), apart(2.5)];
    inputs.extend([agree(1.9); 12]);
    let shown = walk(&mut Hold::default(), &inputs);
    // Not steady while 2.5 is among the ten, then agreement for good.
    assert_eq!(shown[9], "M 1/9");
    assert_eq!(shown[10], "NS 1.90..2.50");
    assert!(shown[11..].iter().all(|s| s == "A"), "{shown:?}");
}

#[test]
fn ten_beyond_of_one_sign_are_not_locked_and_leave_through_not_steady() {
    let mut h = Hold::default();
    let shown = walk(&mut h, &[apart(3.0); 10]);
    assert_eq!(shown[8], "M 9/9");
    assert_eq!(shown[9], "NL 3.00");
    assert_eq!(
        walk(&mut h, &[agree(0.5), agree(0.4)]),
        ["NS 0.50..3.00", "A"]
    );
    // Both signs: not of one mind.
    let mut inputs = vec![apart(3.0), apart(-2.5)];
    inputs.extend([apart(3.0); 8]);
    assert_eq!(last(&inputs), "NS -2.50..3.00");
}

#[test]
fn an_own_line_counts_as_beyond_and_keeps_out_of_the_spread() {
    assert_eq!(last(&[own(4.0, 0.2); 10]), "NLo 4.00");
    let mut inputs = vec![own(4.0, 0.2)];
    inputs.extend([apart(3.0), apart(2.6)].repeat(4));
    inputs.push(apart(3.1));
    // Nine cross minutes and one own one: the spread is of the cross figures.
    assert_eq!(last(&inputs), "NS 2.60..3.10");
}

#[test]
fn agreement_needs_the_judge_at_the_close() {
    // A close with the judge measuring (an own line missing) takes agreement away.
    let mut h = from(&[agree(0.3)]);
    assert_eq!(walk(&mut h, &[apart(0.3)]), ["M"]);
    assert_eq!(walk(&mut h, &[tick(Raw::NotAgree, LIVE); 5]), ["M"; 5]);
    // Between closes, once the judge agrees.
    assert_eq!(walk(&mut h, &[tick(Raw::Agree, LIVE)]), ["A"]);
}

#[test]
fn between_closes_only_a_figure_beyond_the_rule_moves_what_is_shown() {
    let mut h = from(&[agree(0.2)]);
    // The judge's short window beyond, the live figure under a step away: still agreeing.
    assert_eq!(
        walk(&mut h, &[tick(Raw::NotAgree, Some((1.1, 60.0)))]),
        ["A"]
    );
    assert_eq!(h.figure, Some((0.2, 60.0)));
    // An interface fallen to a free crystal: the figure beyond the rule, not agreement.
    assert_eq!(
        walk(&mut h, &[tick(Raw::NotAgree, Some((12.4, 60.0)))]),
        ["M"]
    );
    assert_eq!(h.figure, Some((12.4, 60.0)));
    // A step exactly is not more than a step.
    let mut h = from(&[agree(0.2)]);
    h.step(tick(Raw::Agree, Some((1.2, 60.0))));
    assert_eq!(h.figure, Some((0.2, 60.0)));
    h.step(tick(Raw::Agree, Some((1.21, 60.0))));
    assert_eq!(h.figure, Some((1.21, 60.0)));
}

#[test]
fn no_figure_until_a_minute_closes_then_the_minutes_own() {
    let mut h = Hold::default();
    h.step(tick(Raw::NotAgree, Some((0.9, 12.0))));
    assert_eq!(h.figure, None);
    h.step(Input {
        raw: Raw::Agree,
        event: Event::Close(Some(1.984)),
        live: Some((2.3, 60.0)),
    });
    assert_eq!(h.figure, Some((1.98, 60.0)));
}

#[test]
fn evidence_that_stops_keeps_the_count_and_agreement_waits_a_close() {
    let mut h = from(&[agree(0.3), apart(2.6)]);
    assert_eq!(
        walk(&mut h, &[tick(link_down(), None), tick(Raw::Agree, LIVE)]),
        ["LD", "M 1/1"]
    );
    // From agreeing: back as measuring, agreeing again at the next close.
    let mut h = from(&[agree(0.3)]);
    let back = [
        tick(link_down(), None),
        tick(Raw::Agree, LIVE),
        tick(Raw::Agree, LIVE),
        agree(0.3),
    ];
    assert_eq!(walk(&mut h, &back), ["LD", "M", "M", "A"]);
    // A gap in the marks alone: the same.
    let mut h = from(&[agree(0.3), apart(2.6)]);
    let gap = [event(Raw::Agree, Event::Gap), tick(Raw::Agree, LIVE)];
    assert_eq!(walk(&mut h, &gap), ["M 1/1", "M 1/1"]);
    let mut h = from(&[agree(0.3)]);
    let gap = [
        event(Raw::Agree, Event::Gap),
        tick(Raw::Agree, LIVE),
        agree(0.3),
    ];
    assert_eq!(walk(&mut h, &gap), ["M", "M", "A"]);
}

#[test]
fn what_the_judge_cannot_tell_ends_the_count_and_the_minute_open_then() {
    let mut h = from(&[apart(3.0); 5]);
    let late = Input {
        raw: marks_late(),
        event: Event::Close(Some(3.0)),
        live: LIVE,
    };
    // The close on the same report as the lateness is not counted, nor the next.
    let after = [late, tick(Raw::Agree, LIVE), agree(0.3), agree(0.3)];
    assert_eq!(walk(&mut h, &after), ["LATE", "M", "M", "A"]);
    // Another nominal rate the same way.
    let other = PeerRelation::NotLocked {
        reason: NotLockedReason::OtherRate {
            theirs: 48_000,
            ours: 96_000,
        },
    };
    let mut h = from(&[apart(3.0); 5]);
    h.step(tick(Raw::of(other), None));
    assert_eq!(walk(&mut h, &[apart(3.0)]), ["M"]);
    assert_eq!(h.counting(), None);
}

#[test]
fn a_long_hole_starts_the_count_and_the_agreement_again() {
    let mut inputs = vec![apart(3.0); 5];
    inputs.push(event(Raw::NotAgree, Event::Restart));
    inputs.extend([apart(3.0); 5]);
    assert_eq!(last(&inputs), "M 5/5");
    let mut h = from(&[agree(0.2)]);
    let after = [
        event(Raw::Agree, Event::Restart),
        tick(Raw::Agree, LIVE),
        agree(0.1),
    ];
    assert_eq!(walk(&mut h, &after), ["M", "M", "A"]);
}

#[test]
fn thin_minutes_break_the_run() {
    let mut inputs = vec![apart(3.0); 9];
    inputs.push(thin());
    inputs.push(apart(3.0));
    assert_eq!(last(&inputs), "M 1/1");
}
