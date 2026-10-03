//! The per-second log of each SPL meter and what follows from it: the rolling windows are
//! rebuilt from it whenever a meter's job starts, the session and the autosave save it,
//! `spl.log_get` serves it (`docs/design/leq.md`).
//!
//! The log is owned by the control thread (one per SPL measurement, for the measurement's
//! lifetime) and shared with the meter's job, which appends one row a second; both hold
//! the lock only to copy a row in or out.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use ac2_core::leq::{RollingLeq, Second};
use ac2_proto::model::{LeqConfig, LeqJudgement, LeqWindow, LeqWindowState, SplLogPage, SplLogRow};
use ac2_proto::units::{MeasId, WallNs};

const NS: u64 = 1_000_000_000;

/// One meter's log: the newest [`SplLogPage::RETAINED_ROWS`] rows and how many were logged.
#[derive(Debug, Default)]
pub(crate) struct LeqLog {
    rows: VecDeque<SplLogRow>,
    total: u64,
}

/// A log shared between the control thread and the meter's job.
pub(crate) type SharedLog = Arc<Mutex<LeqLog>>;

/// Locks a shared log; a job that panicked while holding it left whole rows behind.
pub(crate) fn lock(l: &SharedLog) -> std::sync::MutexGuard<'_, LeqLog> {
    l.lock().unwrap_or_else(PoisonError::into_inner)
}

impl LeqLog {
    /// A log holding `rows` (a loaded session's), numbered from 0.
    pub(crate) fn from_rows(rows: Vec<SplLogRow>) -> Self {
        let mut l = Self::default();
        for r in rows {
            l.push(r);
        }
        l
    }

    /// Appends a row, dropping the oldest beyond the retention.
    pub(crate) fn push(&mut self, r: SplLogRow) {
        if self.rows.len() >= SplLogPage::RETAINED_ROWS {
            self.rows.pop_front();
        }
        self.rows.push_back(r);
        self.total += 1;
    }

    /// Rows logged so far.
    pub(crate) fn total(&self) -> u64 {
        self.total
    }

    /// Wall time of the oldest row held.
    pub(crate) fn started_at(&self) -> Option<WallNs> {
        self.rows.front().map(|r| r.start)
    }

    /// Every row held, oldest first.
    pub(crate) fn rows(&self) -> Vec<SplLogRow> {
        self.rows.iter().copied().collect()
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
    /// windows then count as elapsed from the oldest row inside that span.
    pub(crate) fn rebuild(&self, ring: &mut RollingLeq, now: u64) {
        ring.clear();
        let cap = u64::from(ring.capacity());
        let span_start = now.saturating_sub(cap * NS);
        let mut slots: Vec<Option<Second>> = vec![None; cap as usize];
        for r in self.rows.iter().rev() {
            // Rows are a second apart; half a second either way decides the slot.
            let Some(off) = (r.start.0 + NS / 2).checked_sub(span_start) else {
                break;
            };
            let k = off / NS;
            if k >= cap {
                continue;
            }
            let s = row_second(r);
            let slot = &mut slots[k as usize];
            *slot = Some(match slot {
                Some(o) => Second {
                    energy: [
                        o.energy[0] + s.energy[0],
                        o.energy[1] + s.energy[1],
                        o.energy[2] + s.energy[2],
                    ],
                    measured: o.measured + s.measured,
                },
                None => s,
            });
        }
        let Some(first) = slots.iter().position(Option::is_some) else {
            return;
        };
        for s in &slots[first..] {
            ring.push(s.unwrap_or(Second::GAP));
        }
    }
}

/// A log row back as energy.
pub(crate) fn row_second(r: &SplLogRow) -> Second {
    Second::from_levels([r.laeq.0, r.lceq.0, r.lzeq.0], r.measured.0)
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
            sensitivity: Some(Db(120.0)),
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
        l.rebuild(&mut r, t0 + 240 * NS);
        let v = r.value(0);
        assert_eq!(v.elapsed, 240);
        assert!((v.measured - 120.0).abs() < 1e-9);
        assert!(v.incomplete());
        let want = 10.0 * ((10f64.powf(-2.0) + 10f64.powf(-3.0)) / 2.0).log10();
        assert!((v.leq_dbfs - want).abs() < 1e-9, "{}", v.leq_dbfs);
        let v = r.value(1);
        assert!((v.leq_dbfs + 30.0).abs() < 1e-9);
        assert!(!v.incomplete());
        // Nothing within the span: empty windows.
        l.rebuild(&mut r, t0 + 5000 * NS);
        assert_eq!(r.pushed(), 0);
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
