use super::*;
use ac2_core::leq::WindowSpec;
use ac2_core::weighting::Weighting;
use ac2_proto::model::LogClock;
use ac2_proto::units::{Db, Dbfs, Seconds};

/// A row of run 0 at wall time `t` (ns), its index the whole seconds of `t`.
pub(super) fn row(t: u64, level: f64) -> SplLogRow {
    SplLogRow {
        second: t / NS,
        run: 0,
        start: WallNs(t),
        clock: LogClock::Synced,
        gap: None,
        measured: Seconds(1.0),
        laeq: Dbfs(level),
        lceq: Dbfs(level),
        lzeq: Dbfs(level),
        lcpeak: Dbfs(level + 12.0),
        lafmax: Dbfs(level + 2.0),
        sensitivity: Some(Db(120.0)),
        position: None,
    }
}

#[test]
fn pages_and_retention() {
    let mut l = LeqLog::default();
    for k in 0..10 {
        l.push(row(k * NS, -20.0));
    }
    let p = l.page(MeasId(1), 3, 4);
    assert_eq!((p.from, p.total, p.rows.len()), (3, 10, 4));
    assert_eq!(p.rows[0].start, WallNs(3 * NS));
    let p = l.page(MeasId(1), 50, 4);
    assert_eq!((p.from, p.rows.len()), (10, 0));
    let (t, start) = l.tail(4);
    assert_eq!((t.len(), t[0].start, start), (4, WallNs(6 * NS), false));
    let (t, start) = l.tail(40);
    assert_eq!((t.len(), start), (10, true));
    assert_eq!(l.started_at(), Some(WallNs(0)));
}

/// Rows placed by index: a two-minute pause becomes a gap, the windows count
/// elapsed time from the oldest row in the span, and the Leq is over measured time.
#[test]
fn rebuild_by_index() {
    let t0 = 1_790_000_000 * NS;
    let mut l = LeqLog::default();
    for k in 0..60 {
        l.push(row(t0 + k * NS, -20.0));
    }
    for k in 180..240 {
        l.push(row(t0 + k * NS + NS / 10, -30.0));
    }
    let specs = [
        WindowSpec {
            seconds: 600,
            weighting: Weighting::A,
        },
        WindowSpec {
            seconds: 60,
            weighting: Weighting::A,
        },
    ];
    let mut r = RollingLeq::new(&specs, 60);
    let mut peaks = RecentPeaks::default();
    l.rebuild(&mut r, &mut peaks, t0 / NS + 240);
    // The newest ten seconds' maxima: -30 dBFS rows, +12 dB peaks.
    assert!((peaks.max_dbfs()[0] + 18.0).abs() < 1e-9);
    let v = r.value(0);
    assert_eq!(v.elapsed, 240);
    assert!((v.measured - 120.0).abs() < 1e-9);
    assert!(v.incomplete());
    let want = 10.0 * ((10f64.powf(-2.0) + 10f64.powf(-3.0)) / 2.0).log10();
    assert!((v.leq_dbfs - want).abs() < 1e-9, "{}", v.leq_dbfs);
    let v = r.value(1);
    assert!((v.leq_dbfs + 30.0).abs() < 1e-9);
    assert!(!v.incomplete());
    // Nothing within the span: empty windows, no peaks.
    l.rebuild(&mut r, &mut peaks, t0 / NS + 5000);
    assert_eq!(r.pushed(), 0);
    assert!(peaks.max_dbfs()[0].is_nan());
}

/// Brute force over `rows`: span, measured time, gaps and LAeq.
fn brute(rows: &[SplLogRow]) -> (u64, u64, f64, f64, f64) {
    let start = rows[0].start.0;
    let until = rows[rows.len() - 1].start.0 + NS;
    let m: f64 = rows.iter().map(|r| r.measured.0).sum();
    let e: f64 = rows
        .iter()
        .map(|r| 10f64.powf(r.laeq.0 / 10.0) / 2.0 * r.measured.0)
        .sum();
    let slots = (until - start + NS / 2) / NS;
    (
        start,
        until,
        m,
        slots as f64 - m,
        10.0 * (2.0 * e / m).log10(),
    )
}

/// The whole log's total is the energy average over the measured time, exactly; a
/// missing stretch (the meter stopped, a lost second) and partial seconds count as
/// gaps, never as silence.
#[test]
fn run_total_and_gaps_match_brute_force() {
    let t0 = 1_790_000_000 * NS;
    let mut l = LeqLog::default();
    assert_eq!(l.run(), None);
    let mut rows = Vec::new();
    let mut t = t0;
    for k in 0..5000u64 {
        // Two pauses (12 s and 5 min), some lost seconds and partial ones.
        t += match k {
            1000 => 13 * NS,
            3000 => 301 * NS,
            _ if k % 97 == 0 && k > 0 => 2 * NS,
            _ if k > 0 => NS,
            _ => 0,
        };
        let level = -40.0 + 30.0 * ((k as f64) * 0.37).sin();
        let mut r = row(t + (k % 3) * NS / 50, level);
        if k % 41 == 0 {
            r.measured = Seconds(0.3);
        }
        rows.push(r);
        l.push(r);
    }
    let run = l.run().expect("rows");
    let (start, until, m, gaps, laeq) = brute(&rows);
    assert_eq!(run.started_at, WallNs(start));
    assert!(run.until.0.abs_diff(until) < NS / 10);
    assert!((run.measured - m).abs() < 1e-9);
    assert!((run.gaps - gaps).abs() < 1e-6, "{} vs {gaps}", run.gaps);
    assert!(run.gaps > 12.0 + 300.0, "{}", run.gaps);
    assert!(
        (run.levels_dbfs[0] - laeq).abs() < 1e-9,
        "{run:?} vs {laeq}"
    );
    assert!(!run.trimmed);
    // Reloaded from its rows (a daemon restart): the same run, from the same start.
    let again = LeqLog::from_rows(l.rows(), Vec::new()).run().expect("rows");
    assert_eq!(again.started_at, run.started_at);
    assert!((again.levels_dbfs[0] - run.levels_dbfs[0]).abs() < 1e-9);
    assert!((again.gaps - run.gaps).abs() < 1e-9);
}

/// At the retention the oldest rows go: the run starts at the oldest kept second, says
/// it is trimmed, and its total and gaps cover only what is kept.
#[test]
fn trimmed_log_runs_from_the_oldest_kept_second() {
    let t0 = 1_790_000_000 * NS;
    let n = SplLogPage::RETAINED_ROWS as u64 + 5000;
    let mut l = LeqLog::default();
    let mut rows = Vec::new();
    for k in 0..n {
        // A ten-second pause early on: trimmed away later.
        let t = t0 + k * NS + if k >= 100 { 10 * NS } else { 0 };
        let level = if k < 4000 {
            -5.0
        } else {
            -60.0 + (k % 10) as f64
        };
        let r = row(t, level);
        rows.push(r);
        l.push(r);
    }
    assert_eq!(l.total(), n);
    let kept = &rows[rows.len() - SplLogPage::RETAINED_ROWS..];
    let run = l.run().expect("rows");
    assert!(run.trimmed);
    assert_eq!(run.started_at, kept[0].start);
    let (_, _, m, gaps, laeq) = brute(kept);
    assert!((run.measured - m).abs() < 1e-6);
    assert!(run.gaps.abs() < 1e-6 && gaps.abs() < 1e-6, "{}", run.gaps);
    // The loud first 4000 s left no residue once trimmed away.
    assert!(
        (run.levels_dbfs[0] - laeq).abs() < 1e-9,
        "{} vs {laeq}",
        run.levels_dbfs[0]
    );
    // A full log reloaded is still the last 48 h.
    assert!(
        LeqLog::from_rows(l.rows(), Vec::new())
            .run()
            .expect("rows")
            .trimmed
    );
}

#[test]
fn next_log_is_empty_and_numbered_on() {
    let mut l = LeqLog::default();
    l.push(row(NS, -20.0));
    let n = l.next();
    assert_eq!((n.epoch(), n.total(), n.run()), (1, 0, None));
}

#[test]
fn states_keep_unchanged_windows() {
    let mut a = LeqConfig::default_windows();
    let t = WallNs(5);
    let old: Vec<LeqWindowState> = a
        .windows
        .iter()
        .map(|w| LeqWindowState {
            duration: w.duration,
            weighting: w.weighting,
            judgement: LeqJudgement::NoLimit,
            since: WallNs(1),
        })
        .collect();
    let before = a.clone();
    a.windows[3].limit = Some(ac2_proto::units::DbSpl(99.0));
    let s = window_states(&a, Some(&before), &old, true, t);
    assert_eq!(s[0].since, WallNs(1));
    assert_eq!(s[3].judgement, LeqJudgement::Ok);
    assert_eq!(s[3].since, t);
    let s = window_states(&a, Some(&before), &old, false, t);
    assert_eq!(s[3].judgement, LeqJudgement::NotCalibrated);
}
