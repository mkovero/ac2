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

#[test]
fn two_closes_in_one_report_are_a_broken_run() {
    let m = |closed| {
        Some(Minutes {
            series: 1,
            gaps: 0,
            holes: 0,
            closed,
            last: Some(0.5),
        })
    };
    let mut seen = None;
    Event::of(&mut seen, m(0));
    assert_eq!(Event::of(&mut seen, m(2)), Event::Close(None));
    assert_eq!(Event::of(&mut seen, m(3)), Event::Close(Some(0.5)));
}

// The hold itself, a report at a time.

const LIVE: Option<(f64, f64)> = Some((0.5, 60.0));
const RIG: RigId = RigId(2);

fn input(raw: Raw, event: Event, live: Option<(f64, f64)>) -> Input {
    Input {
        raw,
        event,
        live,
        line: None,
        young_beyond: false,
        now: Duration::ZERO,
    }
}

fn tick(raw: Raw, live: Option<(f64, f64)>) -> Input {
    input(raw, Event::None, live)
}

fn close(raw: Raw, p: f64) -> Input {
    input(raw, Event::Close(Some(p)), Some((p, 60.0)))
}

fn agree(p: f64) -> Input {
    close(Raw::Agree, p)
}

fn apart(p: f64) -> Input {
    close(Raw::NotAgree, p)
}

/// A minute closing at `cross` with rig 2's own line at `ppm`.
fn own(ppm: f64, cross: f64) -> Input {
    Input {
        line: Some(Line { rig: RIG, ppm }),
        ..apart(cross)
    }
}

fn thin() -> Input {
    input(Raw::NotAgree, Event::Close(None), None)
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
    input(raw, event, LIVE)
}

/// A hold fed reports a second apart, a minute apart where one closes.
struct Run {
    h: Hold,
    now: Duration,
}

impl Run {
    fn new() -> Self {
        Self {
            h: Hold::default(),
            now: Duration::ZERO,
        }
    }

    fn from(inputs: &[Input]) -> Self {
        let mut r = Self::new();
        r.walk(inputs);
        r
    }

    fn step(&mut self, i: Input) -> String {
        self.now += match i.event {
            Event::Close(_) => Duration::from_secs(60),
            _ => Duration::from_secs(1),
        };
        let r = self.h.step(Input { now: self.now, ..i });
        code(&self.h, r)
    }

    /// The codes shown after each of `inputs`.
    fn walk(&mut self, inputs: &[Input]) -> Vec<String> {
        inputs.iter().map(|i| self.step(*i)).collect()
    }

    /// The full view after each of `inputs`.
    fn view(&mut self, i: Input) -> String {
        let shown = self.step(i);
        view(&self.h, &shown)
    }
}

fn last(inputs: &[Input]) -> String {
    Run::new().walk(inputs).pop().unwrap()
}

/// Short text for what is shown: `A`, `M` (with its count `M b/c`), `NL ppm`, `NLo ppm`,
/// `NS low..high` (`NSo` for an own line's), `BRK`, or the reason.
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
            reason: CantTellReason::NotSteady { low, high, line },
        } => format!(
            "NS{} {low:.2}..{high:.2}",
            if line.is_some() { "o" } else { "" }
        ),
        PeerRelation::CantTell {
            reason: CantTellReason::MarksKeepBreaking,
        } => "BRK".into(),
        PeerRelation::CantTell {
            reason: CantTellReason::LinkDown,
        } => "LD".into(),
        PeerRelation::CantTell {
            reason: CantTellReason::MarksLate { .. },
        } => "LATE".into(),
        other => format!("{other:?}"),
    }
}

/// What is shown with the state behind it: the figure, what is waited for (`-`, `C`lose,
/// `D`iscard) and the last minute (`o`: its own line beyond).
fn view(h: &Hold, shown: &str) -> String {
    let figure = h.figure.map_or("-".into(), |(p, _)| format!("{p:.2}"));
    let wait = match h.wait {
        Wait::Nothing => "-",
        Wait::Close => "C",
        Wait::Discard => "D",
    };
    let last = h.last.map_or("-".into(), |m| {
        format!(
            "{:.2}{}",
            m.cross,
            if m.line_beyond().is_some() { "o" } else { "" }
        )
    });
    format!("{shown} f{figure} w{wait} l{last}")
}

/// Every state the hold can be in, against every kind of report: what is shown, with the
/// figure, what is waited for and the last minute.
#[test]
fn every_state_meets_every_input() {
    let wander: Vec<Input> = [apart(2.1), agree(1.7), agree(1.7)].repeat(3);
    let states: Vec<(&str, Vec<Input>)> = vec![
        ("fresh", vec![]),
        ("agreeing", vec![agree(0.5)]),
        ("within, judge not yet", vec![apart(0.5)]),
        (
            "agreeing, figure beyond",
            vec![agree(0.5), tick(Raw::Agree, Some((2.6, 60.0)))],
        ),
        ("counting", vec![agree(0.5), apart(3.0)]),
        (
            "counting, waiting",
            vec![
                agree(0.5),
                apart(3.0),
                tick(link_down(), None),
                tick(Raw::Agree, LIVE),
            ],
        ),
        ("counting own", vec![agree(0.5), own(4.0, 0.2)]),
        ("not locked", vec![apart(3.0); 10]),
        (
            "not steady",
            [vec![apart(3.0); 9], vec![agree(0.5)]].concat(),
        ),
        (
            "not locked, link down",
            [vec![apart(3.0); 10], vec![tick(link_down(), None)]].concat(),
        ),
        (
            "not locked, back",
            [
                vec![apart(3.0); 10],
                vec![tick(link_down(), None), tick(Raw::NotAgree, LIVE)],
            ]
            .concat(),
        ),
        (
            "not locked, gap",
            [vec![apart(3.0); 10], vec![event(Raw::NotAgree, Event::Gap)]].concat(),
        ),
        (
            "waiting a close",
            vec![agree(0.5), tick(link_down(), None), tick(Raw::Agree, LIVE)],
        ),
        ("discarding", vec![agree(0.5), tick(marks_late(), None)]),
        (
            "discarding, link down",
            vec![
                agree(0.5),
                tick(marks_late(), None),
                tick(link_down(), None),
            ],
        ),
        (
            "nine within after one beyond",
            [vec![apart(2.5)], vec![agree(1.9); 8]].concat(),
        ),
        ("wandering", wander),
    ];
    let inputs: [(&str, Input); 13] = [
        ("tick, judge agrees", tick(Raw::Agree, LIVE)),
        ("tick, live 2.6", tick(Raw::Agree, Some((2.6, 60.0)))),
        ("tick, judge does not agree", tick(Raw::NotAgree, LIVE)),
        ("close 0.5, judge agrees", agree(0.5)),
        ("close 1.9, judge agrees", agree(1.9)),
        ("close 3.0", apart(3.0)),
        ("close 0.5, judge does not agree", apart(0.5)),
        ("close 0.2, own line 4.0", own(4.0, 0.2)),
        ("close thin", thin()),
        ("gap", event(Raw::Agree, Event::Gap)),
        ("restart", event(Raw::Agree, Event::Restart)),
        ("link down", tick(link_down(), None)),
        ("marks late", tick(marks_late(), None)),
    ];
    let mut out = String::new();
    for (state, path) in &states {
        out.push_str(&format!("{state}\n"));
        for (name, i) in &inputs {
            let mut r = Run::from(path);
            out.push_str(&format!("  {name}: {}\n", r.view(*i)));
        }
    }
    assert!(out == TABLE, "{out}");
}

/// [`every_state_meets_every_input`]: for each state, each input and what follows it.
const TABLE: &str = "\
fresh
  tick, judge agrees: M f- w- l-
  tick, live 2.6: M f- w- l-
  tick, judge does not agree: M f- w- l-
  close 0.5, judge agrees: A f0.50 w- l0.50
  close 1.9, judge agrees: A f1.90 w- l1.90
  close 3.0: M 1/1 f3.00 w- l3.00
  close 0.5, judge does not agree: M f0.50 w- l0.50
  close 0.2, own line 4.0: M 1/1 f0.20 w- l0.20o
  close thin: M f- w- l-
  gap: M f- wC l-
  restart: M f- w- l-
  link down: LD f- wC l-
  marks late: LATE f- wD l-
agreeing
  tick, judge agrees: A f0.50 w- l0.50
  tick, live 2.6: M f2.60 w- l0.50
  tick, judge does not agree: A f0.50 w- l0.50
  close 0.5, judge agrees: A f0.50 w- l0.50
  close 1.9, judge agrees: A f1.90 w- l1.90
  close 3.0: M 1/1 f3.00 w- l3.00
  close 0.5, judge does not agree: M f0.50 w- l0.50
  close 0.2, own line 4.0: M 1/1 f0.20 w- l0.20o
  close thin: M f- w- l-
  gap: M f- wC l0.50
  restart: M f- w- l-
  link down: LD f- wC l0.50
  marks late: LATE f- wD l-
within, judge not yet
  tick, judge agrees: M f0.50 w- l0.50
  tick, live 2.6: M f2.60 w- l0.50
  tick, judge does not agree: M f0.50 w- l0.50
  close 0.5, judge agrees: A f0.50 w- l0.50
  close 1.9, judge agrees: A f1.90 w- l1.90
  close 3.0: M 1/1 f3.00 w- l3.00
  close 0.5, judge does not agree: M f0.50 w- l0.50
  close 0.2, own line 4.0: M 1/1 f0.20 w- l0.20o
  close thin: M f- w- l-
  gap: M f- wC l0.50
  restart: M f- w- l-
  link down: LD f- wC l0.50
  marks late: LATE f- wD l-
agreeing, figure beyond
  tick, judge agrees: A f0.50 w- l0.50
  tick, live 2.6: M f2.60 w- l0.50
  tick, judge does not agree: M f0.50 w- l0.50
  close 0.5, judge agrees: A f0.50 w- l0.50
  close 1.9, judge agrees: A f1.90 w- l1.90
  close 3.0: M 1/1 f3.00 w- l3.00
  close 0.5, judge does not agree: M f0.50 w- l0.50
  close 0.2, own line 4.0: M 1/1 f0.20 w- l0.20o
  close thin: M f- w- l-
  gap: M f- wC l0.50
  restart: M f- w- l-
  link down: LD f- wC l0.50
  marks late: LATE f- wD l-
counting
  tick, judge agrees: M 1/1 f0.50 w- l3.00
  tick, live 2.6: M 1/1 f3.00 w- l3.00
  tick, judge does not agree: M 1/1 f0.50 w- l3.00
  close 0.5, judge agrees: M 1/2 f0.50 w- l0.50
  close 1.9, judge agrees: M 1/2 f1.90 w- l1.90
  close 3.0: M 2/2 f3.00 w- l3.00
  close 0.5, judge does not agree: M 1/2 f0.50 w- l0.50
  close 0.2, own line 4.0: M 2/2 f0.20 w- l0.20o
  close thin: M f- w- l-
  gap: M 1/1 f- wC l3.00
  restart: M f- w- l-
  link down: LD f- wC l3.00
  marks late: LATE f- wD l-
counting, waiting
  tick, judge agrees: M 1/1 f- wC l3.00
  tick, live 2.6: M 1/1 f- wC l3.00
  tick, judge does not agree: M 1/1 f- wC l3.00
  close 0.5, judge agrees: M 1/2 f0.50 w- l0.50
  close 1.9, judge agrees: M 1/2 f1.90 w- l1.90
  close 3.0: M 2/2 f3.00 w- l3.00
  close 0.5, judge does not agree: M 1/2 f0.50 w- l0.50
  close 0.2, own line 4.0: M 2/2 f0.20 w- l0.20o
  close thin: M f- w- l-
  gap: M 1/1 f- wC l3.00
  restart: M f- w- l-
  link down: LD f- wC l3.00
  marks late: LATE f- wD l-
counting own
  tick, judge agrees: M 1/1 f0.20 w- l0.20o
  tick, live 2.6: M 1/1 f2.60 w- l0.20o
  tick, judge does not agree: M 1/1 f0.20 w- l0.20o
  close 0.5, judge agrees: M 1/2 f0.50 w- l0.50
  close 1.9, judge agrees: M 1/2 f1.90 w- l1.90
  close 3.0: M 2/2 f3.00 w- l3.00
  close 0.5, judge does not agree: M 1/2 f0.50 w- l0.50
  close 0.2, own line 4.0: M 2/2 f0.20 w- l0.20o
  close thin: M f- w- l-
  gap: M 1/1 f- wC l0.20o
  restart: M f- w- l-
  link down: LD f- wC l0.20o
  marks late: LATE f- wD l-
not locked
  tick, judge agrees: NL 3.00 f0.50 w- l3.00
  tick, live 2.6: NL 3.00 f3.00 w- l3.00
  tick, judge does not agree: NL 3.00 f0.50 w- l3.00
  close 0.5, judge agrees: NS 0.50..3.00 f0.50 w- l0.50
  close 1.9, judge agrees: NS 1.90..3.00 f1.90 w- l1.90
  close 3.0: NL 3.00 f3.00 w- l3.00
  close 0.5, judge does not agree: NS 0.50..3.00 f0.50 w- l0.50
  close 0.2, own line 4.0: NLo 4.00 f0.20 w- l0.20o
  close thin: M f- w- l-
  gap: M f- wC l3.00
  restart: M f- w- l-
  link down: LD f- wC l3.00
  marks late: LATE f- wD l-
not steady
  tick, judge agrees: NS 0.50..3.00 f0.50 w- l0.50
  tick, live 2.6: NS 0.50..3.00 f2.60 w- l0.50
  tick, judge does not agree: NS 0.50..3.00 f0.50 w- l0.50
  close 0.5, judge agrees: A f0.50 w- l0.50
  close 1.9, judge agrees: NS 0.50..3.00 f1.90 w- l1.90
  close 3.0: NS 0.50..3.00 f3.00 w- l3.00
  close 0.5, judge does not agree: M f0.50 w- l0.50
  close 0.2, own line 4.0: NS 0.20..3.00 f0.20 w- l0.20o
  close thin: M f- w- l-
  gap: M f- wC l0.50
  restart: M f- w- l-
  link down: LD f- wC l0.50
  marks late: LATE f- wD l-
not locked, link down
  tick, judge agrees: M f- wC l3.00
  tick, live 2.6: M f- wC l3.00
  tick, judge does not agree: M f- wC l3.00
  close 0.5, judge agrees: NS 0.50..3.00 f0.50 w- l0.50
  close 1.9, judge agrees: NS 1.90..3.00 f1.90 w- l1.90
  close 3.0: NL 3.00 f3.00 w- l3.00
  close 0.5, judge does not agree: NS 0.50..3.00 f0.50 w- l0.50
  close 0.2, own line 4.0: NLo 4.00 f0.20 w- l0.20o
  close thin: M f- w- l-
  gap: M f- wC l3.00
  restart: M f- w- l-
  link down: LD f- wC l3.00
  marks late: LATE f- wD l-
not locked, back
  tick, judge agrees: M f- wC l3.00
  tick, live 2.6: M f- wC l3.00
  tick, judge does not agree: M f- wC l3.00
  close 0.5, judge agrees: NS 0.50..3.00 f0.50 w- l0.50
  close 1.9, judge agrees: NS 1.90..3.00 f1.90 w- l1.90
  close 3.0: NL 3.00 f3.00 w- l3.00
  close 0.5, judge does not agree: NS 0.50..3.00 f0.50 w- l0.50
  close 0.2, own line 4.0: NLo 4.00 f0.20 w- l0.20o
  close thin: M f- w- l-
  gap: M f- wC l3.00
  restart: M f- w- l-
  link down: LD f- wC l3.00
  marks late: LATE f- wD l-
not locked, gap
  tick, judge agrees: M f- wC l3.00
  tick, live 2.6: M f- wC l3.00
  tick, judge does not agree: M f- wC l3.00
  close 0.5, judge agrees: NS 0.50..3.00 f0.50 w- l0.50
  close 1.9, judge agrees: NS 1.90..3.00 f1.90 w- l1.90
  close 3.0: NL 3.00 f3.00 w- l3.00
  close 0.5, judge does not agree: NS 0.50..3.00 f0.50 w- l0.50
  close 0.2, own line 4.0: NLo 4.00 f0.20 w- l0.20o
  close thin: M f- w- l-
  gap: M f- wC l3.00
  restart: M f- w- l-
  link down: LD f- wC l3.00
  marks late: LATE f- wD l-
waiting a close
  tick, judge agrees: M f- wC l0.50
  tick, live 2.6: M f- wC l0.50
  tick, judge does not agree: M f- wC l0.50
  close 0.5, judge agrees: A f0.50 w- l0.50
  close 1.9, judge agrees: A f1.90 w- l1.90
  close 3.0: M 1/1 f3.00 w- l3.00
  close 0.5, judge does not agree: M f0.50 w- l0.50
  close 0.2, own line 4.0: M 1/1 f0.20 w- l0.20o
  close thin: M f- w- l-
  gap: M f- wC l0.50
  restart: M f- w- l-
  link down: LD f- wC l0.50
  marks late: LATE f- wD l-
discarding
  tick, judge agrees: M f- wD l-
  tick, live 2.6: M f- wD l-
  tick, judge does not agree: M f- wD l-
  close 0.5, judge agrees: M f- w- l-
  close 1.9, judge agrees: M f- w- l-
  close 3.0: M f- w- l-
  close 0.5, judge does not agree: M f- w- l-
  close 0.2, own line 4.0: M f- w- l-
  close thin: M f- w- l-
  gap: M f- wD l-
  restart: M f- w- l-
  link down: LD f- wD l-
  marks late: LATE f- wD l-
discarding, link down
  tick, judge agrees: M f- wD l-
  tick, live 2.6: M f- wD l-
  tick, judge does not agree: M f- wD l-
  close 0.5, judge agrees: M f- w- l-
  close 1.9, judge agrees: M f- w- l-
  close 3.0: M f- w- l-
  close 0.5, judge does not agree: M f- w- l-
  close 0.2, own line 4.0: M f- w- l-
  close thin: M f- w- l-
  gap: M f- wD l-
  restart: M f- w- l-
  link down: LD f- wD l-
  marks late: LATE f- wD l-
nine within after one beyond
  tick, judge agrees: M 1/9 f0.50 w- l1.90
  tick, live 2.6: M 1/9 f1.90 w- l1.90
  tick, judge does not agree: M 1/9 f0.50 w- l1.90
  close 0.5, judge agrees: NS 0.50..2.50 f0.50 w- l0.50
  close 1.9, judge agrees: NS 1.90..2.50 f1.90 w- l1.90
  close 3.0: NS 1.90..3.00 f3.00 w- l3.00
  close 0.5, judge does not agree: NS 0.50..2.50 f0.50 w- l0.50
  close 0.2, own line 4.0: NS 0.20..2.50 f0.20 w- l0.20o
  close thin: M f- w- l-
  gap: M 1/9 f- wC l1.90
  restart: M f- w- l-
  link down: LD f- wC l1.90
  marks late: LATE f- wD l-
wandering
  tick, judge agrees: NS 1.70..2.10 f0.50 w- l1.70
  tick, live 2.6: NS 1.70..2.10 f1.70 w- l1.70
  tick, judge does not agree: NS 1.70..2.10 f0.50 w- l1.70
  close 0.5, judge agrees: NS 0.50..2.10 f0.50 w- l0.50
  close 1.9, judge agrees: NS 1.70..2.10 f1.90 w- l1.90
  close 3.0: M 1/1 f3.00 w- l3.00
  close 0.5, judge does not agree: NS 0.50..2.10 f0.50 w- l0.50
  close 0.2, own line 4.0: M 1/1 f0.20 w- l0.20o
  close thin: M f- w- l-
  gap: M f- wC l1.70
  restart: M f- w- l-
  link down: LD f- wC l1.70
  marks late: LATE f- wD l-
";

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
        Run::new().walk(&[agree(0.3), apart(2.6), agree(0.4), agree(0.2)]),
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
    // The same for an own line.
    assert_eq!(last(&[agree(0.3), own(2.0, 0.3)]), "M");
    assert_eq!(last(&[agree(0.3), own(2.01, 0.3)]), "M 1/1");
    assert_eq!(last(&[own(2.5, 0.3), own(1.79, 0.3), own(1.79, 0.3)]), "M");
    assert_eq!(
        last(&[own(2.5, 0.3), own(1.79, 0.3), own(1.81, 0.3)]),
        "M 1/3"
    );
}

#[test]
fn ten_within_the_rule_end_a_count() {
    let mut inputs = vec![agree(0.3), apart(2.5)];
    inputs.extend([agree(1.9); 12]);
    let shown = Run::new().walk(&inputs);
    // Not steady while 2.5 is among the ten, then agreement for good.
    assert_eq!(shown[9], "M 1/9");
    assert_eq!(shown[10], "NS 1.90..2.50");
    assert!(shown[11..].iter().all(|s| s == "A"), "{shown:?}");
}

#[test]
fn ten_beyond_of_one_sign_are_not_locked_and_leave_through_not_steady() {
    let mut r = Run::new();
    let shown = r.walk(&[apart(3.0); 10]);
    assert_eq!(shown[8], "M 9/9");
    assert_eq!(shown[9], "NL 3.00");
    assert_eq!(r.walk(&[agree(0.5), agree(0.4)]), ["NS 0.50..3.00", "A"]);
    // Both signs: not of one mind.
    let mut inputs = vec![apart(3.0), apart(-2.5)];
    inputs.extend([apart(3.0); 8]);
    assert_eq!(last(&inputs), "NS -2.50..3.00");
}

#[test]
fn an_interface_on_its_own_crystal_is_not_locked_by_either_line() {
    // Its own line about the cross-rig rate, the two either side of the rule in turn.
    let free: Vec<Input> = (0..10)
        .map(|k| {
            let p = if k % 2 == 0 { 2.0 } else { 2.5 };
            own(2.0 + 2.5 - p, p)
        })
        .collect();
    assert_eq!(last(&free), "NL 2.50");
    let free: Vec<Input> = (0..10)
        .map(|k| {
            if k % 2 == 0 {
                own(2.4, 2.0)
            } else {
                own(2.2, 2.5)
            }
        })
        .collect();
    // Its own line beyond in every one: that line, newest.
    assert_eq!(last(&free), "NLo 2.20");
    // The own line beyond all ten, the cross figure not: its line.
    assert_eq!(last(&[own(4.0, 0.2); 10]), "NLo 4.00");
    let mixed: Vec<Input> = (0..10)
        .map(|k| if k < 9 { own(4.0, 2.5) } else { own(4.0, 0.2) })
        .collect();
    assert_eq!(last(&mixed), "NLo 4.00");
}

#[test]
fn an_own_line_that_came_and_went_says_whose_and_its_spread() {
    let mut inputs = vec![own(4.0, 0.2); 9];
    inputs.push(own(0.4, 0.2));
    assert_eq!(last(&inputs), "NSo 0.40..4.00");
    let mut r = Run::from(&inputs);
    let rel = r.h.step(Input {
        now: r.now,
        ..tick(Raw::NotAgree, LIVE)
    });
    assert_eq!(
        rel,
        PeerRelation::CantTell {
            reason: CantTellReason::NotSteady {
                low: 0.4,
                high: 4.0,
                line: Some(RIG)
            }
        }
    );
}

#[test]
fn an_own_line_beyond_between_closes_leaves_its_trace() {
    let mut r = Run::from(&[agree(0.3)]);
    let beyond = Input {
        line: Some(Line { rig: RIG, ppm: 3.0 }),
        ..tick(Raw::NotAgree, LIVE)
    };
    for _ in 0..59 {
        r.step(beyond);
    }
    // Back within on the closing report: the minute is beyond all the same.
    assert_eq!(r.step(agree(0.3)), "M 1/1");
    assert_eq!(r.h.last.unwrap().line.unwrap().ppm, 3.0);
}

#[test]
fn agreement_needs_the_judge_at_the_close_and_three_reports_between() {
    // A close with the judge measuring (an own line missing) takes agreement away.
    let mut r = Run::from(&[agree(0.3)]);
    assert_eq!(r.walk(&[apart(0.3)]), ["M"]);
    assert_eq!(r.walk(&[tick(Raw::NotAgree, LIVE); 5]), ["M"; 5]);
    // Between closes, once the judge agrees three reports running.
    let agreeing = tick(Raw::Agree, LIVE);
    let odd = tick(Raw::NotAgree, LIVE);
    assert_eq!(
        r.walk(&[agreeing, agreeing, odd, agreeing, agreeing, agreeing]),
        ["M", "M", "M", "M", "M", "A"]
    );
}

#[test]
fn between_closes_only_a_figure_beyond_the_rule_moves_what_is_shown() {
    let mut r = Run::from(&[agree(0.2)]);
    // The judge's short window beyond, the live figure under a step away: still agreeing.
    assert_eq!(r.walk(&[tick(Raw::NotAgree, Some((1.1, 60.0)))]), ["A"]);
    assert_eq!(r.h.figure, Some((0.2, 60.0)));
    // An interface fallen to a free crystal: the figure beyond the rule, not agreement.
    assert_eq!(r.walk(&[tick(Raw::NotAgree, Some((12.4, 60.0)))]), ["M"]);
    assert_eq!(r.h.figure, Some((12.4, 60.0)));
    // A step exactly is not more than a step.
    let mut r = Run::from(&[agree(0.2)]);
    r.step(tick(Raw::Agree, Some((1.2, 60.0))));
    assert_eq!(r.h.figure, Some((0.2, 60.0)));
    r.step(tick(Raw::Agree, Some((1.21, 60.0))));
    assert_eq!(r.h.figure, Some((1.21, 60.0)));
}

#[test]
fn no_figure_until_a_minute_closes_then_the_minutes_own() {
    let mut r = Run::new();
    r.step(tick(Raw::NotAgree, Some((0.9, 12.0))));
    assert_eq!(r.h.figure, None);
    r.step(input(
        Raw::Agree,
        Event::Close(Some(1.984)),
        Some((2.3, 60.0)),
    ));
    assert_eq!(r.h.figure, Some((1.98, 60.0)));
}

#[test]
fn evidence_that_stops_keeps_the_count_and_both_verdicts_wait_a_close() {
    let mut r = Run::from(&[agree(0.3), apart(2.6)]);
    assert_eq!(
        r.walk(&[tick(link_down(), None), tick(Raw::Agree, LIVE)]),
        ["LD", "M 1/1"]
    );
    let agreeing = tick(Raw::Agree, LIVE);
    let mut r = Run::from(&[agree(0.3)]);
    let back = [
        tick(link_down(), None),
        agreeing,
        agreeing,
        agreeing,
        agree(0.3),
    ];
    assert_eq!(r.walk(&back), ["LD", "M", "M", "M", "A"]);
    // Not locked the same way.
    let mut r = Run::from(&[apart(3.0); 10]);
    let back = [tick(link_down(), None), agreeing, apart(3.0)];
    assert_eq!(r.walk(&back), ["LD", "M", "NL 3.00"]);
    // A gap in the marks alone: the same, and no figure from before it.
    let mut r = Run::from(&[agree(0.3)]);
    let gap = [
        event(Raw::Agree, Event::Gap),
        agreeing,
        agreeing,
        agreeing,
        agree(0.3),
    ];
    assert_eq!(r.walk(&gap), ["M", "M", "M", "M", "A"]);
}

#[test]
fn holes_that_keep_every_minute_from_closing_say_so() {
    let mut r = Run::from(&[agree(0.3)]);
    let gap = event(Raw::Agree, Event::Gap);
    let quiet = tick(Raw::Agree, LIVE);
    // A gap every 30 s for five minutes.
    let mut shown = Vec::new();
    for _ in 0..10 {
        shown.push(r.step(gap));
        shown.extend(r.walk(&vec![quiet; 29]));
    }
    assert_eq!(shown[200], "M");
    assert_eq!(shown.last().unwrap(), "BRK");
    // A minute closes at last: measured again.
    assert_eq!(r.step(agree(0.3)), "A");
}

#[test]
fn counts_that_keep_starting_are_not_steady_until_a_quiet_half_hour() {
    let mut r = Run::new();
    let shown = r.walk(&[apart(2.1), agree(1.7), agree(1.7)].repeat(3));
    assert_eq!(shown[2], "A");
    assert_eq!(shown[5], "A");
    assert_eq!(shown[8], "NS 1.70..2.10");
    // Settled: still not steady until no count started for half an hour. The third began
    // at 7 min; the minutes close a minute apart from 10 min.
    let settled = r.walk(&[agree(0.3); 30]);
    assert_eq!(settled[0], "NS 0.30..2.10");
    assert_eq!(
        settled.iter().position(|s| s == "A"),
        Some(28),
        "{settled:?}"
    );
}

#[test]
fn what_the_judge_cannot_tell_ends_the_count_and_the_minute_open_then() {
    let mut r = Run::from(&[apart(3.0); 5]);
    let late = input(marks_late(), Event::Close(Some(3.0)), LIVE);
    // The close on the same report as the lateness is not counted, nor the next.
    let after = [late, tick(Raw::Agree, LIVE), agree(0.3), agree(0.3)];
    assert_eq!(r.walk(&after), ["LATE", "M", "M", "A"]);
    // Another nominal rate the same way.
    let other = PeerRelation::NotLocked {
        reason: NotLockedReason::OtherRate {
            theirs_hz: 48_000,
            ours_hz: 96_000,
        },
    };
    let mut r = Run::from(&[apart(3.0); 5]);
    r.step(tick(Raw::of(other), None));
    assert_eq!(r.walk(&[apart(3.0)]), ["M"]);
    assert_eq!(r.h.counting(), None);
}

#[test]
fn a_long_hole_starts_the_count_and_the_agreement_again() {
    let mut inputs = vec![apart(3.0); 5];
    inputs.push(event(Raw::NotAgree, Event::Restart));
    inputs.extend([apart(3.0); 5]);
    assert_eq!(last(&inputs), "M 5/5");
    let mut r = Run::from(&[agree(0.2)]);
    let after = [
        event(Raw::Agree, Event::Restart),
        tick(Raw::Agree, LIVE),
        agree(0.1),
    ];
    assert_eq!(r.walk(&after), ["M", "M", "A"]);
}

#[test]
fn thin_minutes_break_the_run() {
    let mut inputs = vec![apart(3.0); 9];
    inputs.push(thin());
    inputs.push(apart(3.0));
    assert_eq!(last(&inputs), "M 1/1");
}
