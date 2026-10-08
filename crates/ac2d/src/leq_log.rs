//! The per-second log of each SPL meter and what follows from it: the rolling windows are
//! rebuilt from it whenever a meter's job starts, the session and the autosave save it,
//! `spl.log_get` serves it (`docs/design/leq.md`).
//!
//! The log is owned by the control thread (one per SPL measurement, for the measurement's
//! lifetime) and shared with the meter's job, which appends one row a second; both hold
//! the lock only to copy a row in or out.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use ac2_core::band_leq::{BandSecond, Period, Placed};
use ac2_core::leq::{LogTotal, PEAK_HOLD_S, PeakHold, RollingLeq, Second};
use ac2_proto::model::BandPeriod;
use ac2_proto::model::{
    LeqConfig, LeqJudgement, LeqPeakState, LeqWindow, LeqWindowState, PeakQuantity, PeakStates,
    SplLogPage, SplLogRow,
};
use ac2_proto::units::{MeasId, WallNs};
use ac2_traces::band_log::BandLogRow;

const NS: u64 = 1_000_000_000;

/// Rows taken off the oldest end between exact recomputes of the total: subtracting what
/// was added long ago leaves rounding behind, and an exact sum once an hour of trimming
/// bounds it.
const EXACT_EVERY: u32 = 3600;

/// One meter's log: the newest [`SplLogPage::RETAINED_ROWS`] rows and how many were logged,
/// with the whole log's total kept as rows come and go.
#[derive(Debug)]
pub(crate) struct LeqLog {
    /// Unique among the logs of this process: the autosave tells logs apart by it, the
    /// one that replaced a meter's log included.
    id: u64,
    rows: VecDeque<SplLogRow>,
    /// Whole seconds without a row before each row (none before the oldest).
    missing_before: VecDeque<u64>,
    /// Σ `missing_before`.
    missing: u64,
    total: u64,
    sum: LogTotal,
    trimmed_since_exact: u32,
    /// Which log of the meter this is: `spl.log_new` starts the next. A job that finds the
    /// number changed starts its windows over.
    epoch: u64,
    /// The band meter's seconds, newest [`SplLogPage::RETAINED_ROWS`], kept beside the
    /// A/C/Z rows (`docs/design/band-leq.md`, *Stored per second*).
    bands: VecDeque<BandLogRow>,
    /// Band rows logged so far.
    band_total: u64,
}

/// The log as a whole: from its oldest kept second to its newest.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LogRun {
    pub(crate) started_at: WallNs,
    pub(crate) until: WallNs,
    /// Measured time, s.
    pub(crate) measured: f64,
    /// Time between `started_at` and `until` not measured, s.
    pub(crate) gaps: f64,
    /// At the retention: older seconds were (or may have been) dropped.
    pub(crate) trimmed: bool,
    /// LAeq, LCeq, LZeq over the measured time, dBFS.
    pub(crate) levels_dbfs: [f64; 3],
}

/// A log shared between the control thread and the meter's job.
pub(crate) type SharedLog = Arc<Mutex<LeqLog>>;

/// Locks a shared log; a job that panicked while holding it left whole rows behind.
pub(crate) fn lock(l: &SharedLog) -> std::sync::MutexGuard<'_, LeqLog> {
    l.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Default for LeqLog {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            rows: VecDeque::new(),
            missing_before: VecDeque::new(),
            missing: 0,
            total: 0,
            sum: LogTotal::default(),
            trimmed_since_exact: 0,
            epoch: 0,
            bands: VecDeque::new(),
            band_total: 0,
        }
    }
}

impl LeqLog {
    /// Which log this is, unique in the process.
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    /// A log holding `rows` and band rows `bands` (a loaded session's), numbered from 0.
    pub(crate) fn from_rows(rows: Vec<SplLogRow>, bands: Vec<BandLogRow>) -> Self {
        let mut l = Self::default();
        for r in rows {
            l.push(r);
        }
        for b in bands {
            l.push_band(b);
        }
        l
    }

    /// Appends a band row, dropping the oldest beyond the retention.
    pub(crate) fn push_band(&mut self, r: BandLogRow) {
        if self.bands.len() >= SplLogPage::RETAINED_ROWS {
            self.bands.pop_front();
        }
        self.bands.push_back(r);
        self.band_total += 1;
    }

    /// Band rows logged so far.
    pub(crate) fn band_total(&self) -> u64 {
        self.band_total
    }

    /// Every band row held, oldest first.
    pub(crate) fn band_rows(&self) -> Vec<BandLogRow> {
        self.bands.iter().copied().collect()
    }

    /// The band rows numbered `from` on, oldest first.
    pub(crate) fn band_rows_from(&self, from: u64) -> Vec<BandLogRow> {
        let oldest = self.band_total - self.bands.len() as u64;
        let skip = usize::try_from(from.saturating_sub(oldest)).unwrap_or(usize::MAX);
        self.bands.iter().skip(skip).copied().collect()
    }

    /// The band rows starting in `[from, until)`.
    pub(crate) fn band_rows_in(&self, from: WallNs, until: WallNs) -> Vec<BandLogRow> {
        self.bands
            .iter()
            .filter(|r| r.start >= from && r.start < until)
            .copied()
            .collect()
    }

    /// The band seconds of the `seconds` before `now` (wall ns), placed by wall time and
    /// with the §13 correction each was logged with applied: what rebuilds the band windows
    /// ([`Placed::seconds`]).
    pub(crate) fn place_bands(&self, now: u64, seconds: u32) -> Placed {
        let from = now.saturating_sub(u64::from(seconds) * NS + NS);
        Placed::new(
            self.bands
                .iter()
                .rev()
                .take_while(|r| r.start.0 >= from)
                .map(|r| (r.start.0, band_second(r), conv_period(r.period))),
            now,
            seconds,
        )
    }

    /// An empty log, the next of the meter after `self`.
    pub(crate) fn next(&self) -> Self {
        Self {
            epoch: self.epoch + 1,
            ..Self::default()
        }
    }

    /// Which log of the meter this is.
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Appends a row, dropping the oldest beyond the retention.
    pub(crate) fn push(&mut self, r: SplLogRow) {
        if self.rows.len() >= SplLogPage::RETAINED_ROWS {
            self.pop_oldest();
        }
        // Rows are a second apart; a longer step is seconds without a row.
        let missing = self.rows.back().map_or(0, |p| {
            let step = (r.start.0.saturating_sub(p.start.0) + NS / 2) / NS;
            step.saturating_sub(1)
        });
        self.missing += missing;
        self.missing_before.push_back(missing);
        self.sum.add(&row_second(&r));
        self.rows.push_back(r);
        self.total += 1;
    }

    fn pop_oldest(&mut self) {
        let (Some(r), Some(m)) = (self.rows.pop_front(), self.missing_before.pop_front()) else {
            return;
        };
        self.missing -= m;
        // The time between the dropped row and the new oldest is no longer in the log.
        if let Some(next) = self.missing_before.front_mut() {
            self.missing -= *next;
            *next = 0;
        }
        self.sum.remove(&row_second(&r));
        self.trimmed_since_exact += 1;
        if self.trimmed_since_exact >= EXACT_EVERY {
            let mut exact = LogTotal::default();
            for r in &self.rows {
                exact.add(&row_second(r));
            }
            self.sum = exact;
            self.trimmed_since_exact = 0;
        }
    }

    /// Rows logged so far.
    pub(crate) fn total(&self) -> u64 {
        self.total
    }

    /// Wall time of the oldest row held.
    pub(crate) fn started_at(&self) -> Option<WallNs> {
        self.rows.front().map(|r| r.start)
    }

    /// The whole log's span, measured time, gaps and levels; `None` while empty.
    pub(crate) fn run(&self) -> Option<LogRun> {
        let (first, last) = (self.rows.front()?, self.rows.back()?);
        let measured = self.sum.measured();
        // Each row is a second slot: what it did not measure is gap, as is every second
        // between rows.
        let gaps = (self.rows.len() as f64 - measured).max(0.0) + self.missing as f64;
        Some(LogRun {
            started_at: first.start,
            until: WallNs(last.start.0 + NS),
            measured,
            gaps,
            trimmed: self.rows.len() >= SplLogPage::RETAINED_ROWS,
            levels_dbfs: ac2_core::leq::WEIGHTINGS.map(|w| self.sum.level_dbfs(w)),
        })
    }

    /// Every row held, oldest first.
    pub(crate) fn rows(&self) -> Vec<SplLogRow> {
        self.rows.iter().copied().collect()
    }

    /// The rows numbered `from` on (row numbers count from the log's first row; rows no
    /// longer held are left out), oldest first.
    pub(crate) fn rows_from(&self, from: u64) -> Vec<SplLogRow> {
        let oldest = self.total - self.rows.len() as u64;
        let skip = usize::try_from(from.saturating_sub(oldest)).unwrap_or(usize::MAX);
        self.rows.iter().skip(skip).copied().collect()
    }

    /// The newest `n` rows held (or all), oldest first, and whether the first of them is
    /// the log's first row (nothing was logged before it).
    pub(crate) fn tail(&self, n: usize) -> (Vec<SplLogRow>, bool) {
        let skip = self.rows.len().saturating_sub(n);
        let first = self.total - self.rows.len() as u64 + skip as u64;
        (self.rows.iter().skip(skip).copied().collect(), first == 0)
    }

    /// Up to `max` rows from row number `from` (or the oldest held).
    pub(crate) fn page(&self, meas: MeasId, from: u64, max: u32) -> SplLogPage {
        let oldest = self.total - self.rows.len() as u64;
        let from = from.clamp(oldest, self.total);
        let n = max.min(SplLogPage::MAX_ROWS) as usize;
        let skip = (from - oldest) as usize;
        SplLogPage {
            meas,
            from,
            total: self.total,
            rows: self.rows.iter().skip(skip).take(n).copied().collect(),
        }
    }

    /// Refills `ring` with the seconds of the log that fall in the `ring.capacity()`
    /// seconds before `now`, placed by wall time; seconds without a row are gaps. The
    /// windows then count as elapsed from the oldest row inside that span
    /// ([`RollingLeq::refill`], which a client's replay of the log follows too). `peaks`
    /// gets the rows of the newest [`PEAK_HOLD_S`] seconds before `now`, so a restarted
    /// job judges its peak limits on what was just logged.
    pub(crate) fn rebuild(&self, ring: &mut RollingLeq, peaks: &mut PeakHold, now: u64) {
        ring.refill(
            self.rows.iter().rev().map(|r| (r.start.0, row_second(r))),
            now,
        );
        peaks.clear();
        let from = now.saturating_sub(PEAK_HOLD_S as u64 * NS);
        let recent = self
            .rows
            .iter()
            .rev()
            .take_while(|r| r.start.0 >= from)
            .collect::<Vec<_>>();
        for r in recent.into_iter().rev() {
            peaks.push(&row_second(r));
        }
    }
}

/// A band row back as energy, its §13 correction applied (the windows judge the rating
/// level `L + K`).
pub(crate) fn band_second(r: &BandLogRow) -> BandSecond {
    BandSecond::from_levels(&r.levels.map(f64::from), r.measured.0).corrected(r.correction.0)
}

/// The core's period of a logged one.
pub(crate) fn conv_period(p: BandPeriod) -> Period {
    match p {
        BandPeriod::Day => Period::Day,
        BandPeriod::Night => Period::Night,
    }
}

/// A log row back as energy, with its maxima.
pub(crate) fn row_second(r: &SplLogRow) -> Second {
    Second::from_levels([r.laeq.0, r.lceq.0, r.lzeq.0], r.measured.0)
        .with_maxima(r.lcpeak.0, r.lafmax.0)
}

/// Judgement of peak limit `q` of `cfg` before a second was judged.
pub(crate) fn initial_peak_judgement(
    cfg: &LeqConfig,
    q: PeakQuantity,
    calibrated: bool,
) -> LeqJudgement {
    match (cfg.peaks.get(q), calibrated) {
        (None, _) => LeqJudgement::NoLimit,
        (Some(_), false) => LeqJudgement::NotCalibrated,
        (Some(_), true) => LeqJudgement::Ok,
    }
}

/// The peak limits' states for `cfg`: a limit set as before keeps its state, others start
/// from [`initial_peak_judgement`] at `now`.
pub(crate) fn peak_states(
    cfg: &LeqConfig,
    old_cfg: Option<&LeqConfig>,
    old: Option<PeakStates>,
    calibrated: bool,
    now: WallNs,
) -> PeakStates {
    let state = |q: PeakQuantity| {
        let kept = match (old_cfg, old) {
            (Some(oc), Some(o)) if oc.peaks.get(q) == cfg.peaks.get(q) => Some(o.get(q)),
            _ => None,
        };
        kept.unwrap_or(LeqPeakState {
            judgement: initial_peak_judgement(cfg, q, calibrated),
            since: now,
        })
    };
    PeakStates {
        lcpeak: state(PeakQuantity::LcPeak),
        lafmax: state(PeakQuantity::LafMax),
    }
}

/// Judgement of a window without a value to judge yet: what its limit and the
/// calibration allow.
pub(crate) fn initial_judgement(w: &LeqWindow, calibrated: bool) -> LeqJudgement {
    match (w.limit, calibrated) {
        (None, _) => LeqJudgement::NoLimit,
        (Some(_), false) => LeqJudgement::NotCalibrated,
        (Some(_), true) => LeqJudgement::Ok,
    }
}

/// The window states for `cfg`: a window configured as before keeps its state, others
/// start from [`initial_judgement`] at `now`.
pub(crate) fn window_states(
    cfg: &LeqConfig,
    old_cfg: Option<&LeqConfig>,
    old: &[LeqWindowState],
    calibrated: bool,
    now: WallNs,
) -> Vec<LeqWindowState> {
    cfg.windows
        .iter()
        .map(|w| {
            let kept = old_cfg.and_then(|oc| {
                oc.windows
                    .iter()
                    .position(|o| o == w)
                    .and_then(|i| old.get(i))
                    .copied()
            });
            kept.unwrap_or(LeqWindowState {
                duration: w.duration,
                weighting: w.weighting,
                judgement: initial_judgement(w, calibrated),
                since: now,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_core::leq::WindowSpec;
    use ac2_core::weighting::Weighting;
    use ac2_proto::units::{Db, Dbfs, Seconds};

    fn row(t: u64, level: f64) -> SplLogRow {
        SplLogRow {
            start: WallNs(t),
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

    /// Rows placed by wall time: a two-minute pause becomes a gap, the windows count
    /// elapsed time from the oldest row in the span, and the Leq is over measured time.
    #[test]
    fn rebuild_by_wall_time() {
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
        let mut peaks = PeakHold::default();
        l.rebuild(&mut r, &mut peaks, t0 + 240 * NS);
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
        l.rebuild(&mut r, &mut peaks, t0 + 5000 * NS);
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
}
