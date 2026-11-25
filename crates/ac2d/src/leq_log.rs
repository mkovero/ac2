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
use ac2_core::leq::{LogTotal, PEAK_HOLD_S, RecentPeaks, RollingLeq, Second};
use ac2_proto::model::BandPeriod;
use ac2_proto::model::{
    LeqConfig, LeqJudgement, LeqPeakState, LeqWindow, LeqWindowState, PeakLimit, PeakQuantity,
    PeakStates, SplLogPage, SplLogRow,
};
use ac2_proto::units::{MeasId, WallNs};
use ac2_traces::band_log::BandLogRow;

pub(crate) mod runs;

const NS: u64 = 1_000_000_000;

/// Rows taken off the oldest end between exact recomputes of the total: subtracting what
/// was added long ago leaves rounding behind, and an exact sum once an hour of trimming
/// bounds it.
const EXACT_EVERY: u32 = 3600;

/// One meter's log: the newest [`SplLogPage::RETAINED_ROWS`] rows and how many were logged,
/// with the whole log's total kept as rows come and go. Rows are keyed by their second
/// index ([`SplLogRow::second`]); wall times are annotations ([`runs`]).
#[derive(Debug)]
pub(crate) struct LeqLog {
    /// Unique among the logs of this process: the autosave tells logs apart by it, the
    /// one that replaced a meter's log included.
    id: u64,
    /// When the log started: its identity to clients ([`SplLogPage::started_at`]).
    started: WallNs,
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
    /// Runs, indices and the high-water mark.
    runs: runs::Runs,
}

/// The log as a whole: from its oldest kept second to its newest.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LogRun {
    pub(crate) started_at: WallNs,
    pub(crate) until: WallNs,
    /// Measured time, s.
    pub(crate) measured: f64,
    /// Seconds between the oldest and the newest index not measured (an unknown gap
    /// counts none: its length is not known).
    pub(crate) gaps: f64,
    /// At the retention: older seconds were (or may have been) dropped.
    pub(crate) trimmed: bool,
    /// LAeq, LCeq, LZeq over the measured time, dBFS.
    pub(crate) levels_dbfs: [f64; 3],
}

/// The oldest and newest second a log holds, with their wall annotations.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Extent {
    pub(crate) first: u64,
    pub(crate) last: u64,
    pub(crate) start: WallNs,
    pub(crate) until: WallNs,
}

/// A log shared between the control thread and the meter's job.
pub(crate) type SharedLog = Arc<Mutex<LeqLog>>;

/// Locks a shared log; a job that panicked while holding it left whole rows behind.
pub(crate) fn lock(l: &SharedLog) -> std::sync::MutexGuard<'_, LeqLog> {
    l.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Now on the wall clock to the millisecond: a log's start, which its file's header holds
/// at that resolution, so a log read back keeps the identity it was served under.
fn started_now() -> WallNs {
    WallNs(crate::util::wall_ns() / 1_000_000 * 1_000_000)
}

impl Default for LeqLog {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            started: started_now(),
            rows: VecDeque::new(),
            missing_before: VecDeque::new(),
            missing: 0,
            total: 0,
            sum: LogTotal::default(),
            trimmed_since_exact: 0,
            epoch: 0,
            bands: VecDeque::new(),
            band_total: 0,
            runs: runs::Runs::default(),
        }
    }
}

impl LeqLog {
    /// Which log this is, unique in the process.
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    /// When the log started (its identity to clients).
    pub(crate) fn started(&self) -> WallNs {
        self.started
    }

    /// A log read back (a loaded session's, the autosave's, the rig log directory's), its
    /// rows numbered from 0: the first run of this process starts from its last row and
    /// its high-water mark ([`runs`]). A mark that is missing, unreadable or below the log
    /// leaves the rows served but never written unknown: the log takes a new start, so
    /// clients drop what they cached, and the next run's gap is unknown.
    pub(crate) fn restored(l: ac2_traces::session::SavedSplLog) -> Self {
        let mut log = Self {
            started: l.info.started,
            ..Self::default()
        };
        for r in l.rows {
            log.push(r);
        }
        for b in l.bands {
            log.push_band(b);
        }
        runs::restore(&mut log, l.info.meas, l.mark);
        log
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

    /// The band rows starting in `[from, until)` (wall time, as logged).
    pub(crate) fn band_rows_in(&self, from: WallNs, until: WallNs) -> Vec<BandLogRow> {
        self.bands
            .iter()
            .filter(|r| r.start >= from && r.start < until)
            .copied()
            .collect()
    }

    /// The band seconds of the `seconds` indices before `now` (a second index), placed by
    /// index and with the §13 correction each was logged with applied, never across an
    /// unknown gap: what rebuilds the band windows ([`Placed::seconds`], whose times are
    /// the indices in ns). A second whose period was can't tell is placed as day; the
    /// band meter keeps the can't-tell apart ([`Self::newest_cant_tell`]).
    pub(crate) fn place_bands(&self, now: u64, seconds: u32) -> Placed {
        let from = now
            .saturating_sub(u64::from(seconds) + 1)
            .max(self.runs.floor);
        Placed::new(
            self.bands
                .iter()
                .rev()
                .take_while(|r| r.second >= from)
                .map(|r| {
                    let p = r.period.map_or(Period::Day, conv_period);
                    (r.second * NS, band_second(r), p)
                }),
            now * NS,
            seconds,
        )
    }

    /// The newest band second before `now` whose period was can't tell, within the
    /// windows' reach (not across an unknown gap).
    pub(crate) fn newest_cant_tell(&self, now: u64) -> Option<u64> {
        self.bands
            .iter()
            .rev()
            .take_while(|r| r.second >= self.runs.floor && r.second < now)
            .find(|r| r.period.is_none())
            .map(|r| r.second)
    }

    /// An empty log, the next of the meter after `self`: a new start, second 0 and run 0
    /// (the meter's job starts a run on it when it sees the new epoch).
    pub(crate) fn next(&self) -> Self {
        Self {
            epoch: self.epoch + 1,
            runs: self.runs.fresh(),
            ..Self::default()
        }
    }

    /// Which log of the meter this is.
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Appends a row, dropping the oldest beyond the retention.
    fn push(&mut self, r: SplLogRow) {
        if self.rows.len() >= SplLogPage::RETAINED_ROWS {
            self.pop_oldest();
        }
        // Indices of seconds without a row between two rows; an unknown gap advanced the
        // index by one, so it adds none.
        let missing = self
            .rows
            .back()
            .map_or(0, |p| r.second.saturating_sub(p.second).saturating_sub(1));
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

    /// Wall time of the oldest row held, as logged.
    pub(crate) fn started_at(&self) -> Option<WallNs> {
        self.rows.front().map(|r| r.start)
    }

    /// The whole log's span, measured time, gaps and levels; `None` while empty.
    pub(crate) fn run(&self) -> Option<LogRun> {
        let (first, last) = (self.rows.front()?, self.rows.back()?);
        let measured = self.sum.measured();
        // Each row is a second slot: what it did not measure is gap, as is every index
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

    /// The rows held whose index lies in `from ..= to`, oldest first, and whether the
    /// first of them is the log's first row.
    pub(crate) fn rows_in(&self, from: u64, to: u64) -> (Vec<SplLogRow>, bool) {
        let lo = self.rows.partition_point(|r| r.second < from);
        let hi = self.rows.partition_point(|r| r.second <= to);
        let first = self.total - self.rows.len() as u64 + lo as u64;
        (
            self.rows.range(lo..hi.max(lo)).copied().collect(),
            first == 0,
        )
    }

    /// The oldest and newest second held; `None` while empty.
    pub(crate) fn extent(&self) -> Option<Extent> {
        let (a, z) = (self.rows.front()?, self.rows.back()?);
        Some(Extent {
            first: a.second,
            last: z.second,
            start: a.start,
            until: WallNs(z.start.0 + NS),
        })
    }

    /// Up to `max` rows from row number `from` (or the oldest held).
    pub(crate) fn page(&self, meas: MeasId, from: u64, max: u32) -> SplLogPage {
        let oldest = self.total - self.rows.len() as u64;
        let from = from.clamp(oldest, self.total);
        let n = max.min(SplLogPage::MAX_ROWS) as usize;
        let skip = (from - oldest) as usize;
        SplLogPage {
            meas,
            started_at: self.started,
            from,
            total: self.total,
            rows: self.rows.iter().skip(skip).take(n).copied().collect(),
        }
    }

    /// Refills `ring` with the rows whose index lies within the `ring.capacity()` seconds
    /// before `now` (the index of the second about to start), never across an unknown gap;
    /// indices without a row are gaps. The windows then count as elapsed from the oldest
    /// row inside that span ([`RollingLeq::refill`], which a client's replay of the log
    /// follows too). `peaks` gets the rows of the newest [`PEAK_HOLD_S`] indices before
    /// `now` under the same rule, so a restarted job judges its peak limits on what was
    /// just logged. After a power cut with an unset clock the windows start over: a level
    /// of unknown age is never presented as recent.
    pub(crate) fn rebuild(&self, ring: &mut RollingLeq, peaks: &mut RecentPeaks, now: u64) {
        let floor = self.runs.floor;
        ring.refill(
            self.rows
                .iter()
                .rev()
                .take_while(|r| r.second >= floor)
                .map(|r| (r.second * NS, row_second(r))),
            now * NS,
        );
        peaks.clear();
        let from = now.saturating_sub(PEAK_HOLD_S as u64).max(floor);
        let recent = self
            .rows
            .iter()
            .rev()
            .take_while(|r| r.second >= from)
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
            (Some(oc), Some(o)) if same_peak(oc.peaks.get(q), cfg.peaks.get(q)) => Some(o.get(q)),
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

/// Whether two windows are judged alike: a cue says what to send about a state, it is not
/// part of what is judged, so editing one keeps the window's state.
pub(crate) fn same_window(a: &LeqWindow, b: &LeqWindow) -> bool {
    a.duration == b.duration
        && a.weighting == b.weighting
        && a.limit == b.limit
        && a.warn_margin == b.warn_margin
}

/// The same for a peak limit.
pub(crate) fn same_peak(a: Option<&PeakLimit>, b: Option<&PeakLimit>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.limit == b.limit && a.warn_margin == b.warn_margin,
        (None, None) => true,
        _ => false,
    }
}

/// Whether two Leq configurations judge alike (they differ in cues at most).
pub(crate) fn same_judging(a: &LeqConfig, b: &LeqConfig) -> bool {
    a.horizon == b.horizon
        && a.windows.len() == b.windows.len()
        && a.windows
            .iter()
            .zip(&b.windows)
            .all(|(x, y)| same_window(x, y))
        && PeakQuantity::ALL
            .into_iter()
            .all(|q| same_peak(a.peaks.get(q), b.peaks.get(q)))
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
                    .position(|o| same_window(o, w))
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
mod tests;
