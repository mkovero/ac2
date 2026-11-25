//! A log's runs, second indices and high-water mark (`docs/design/protocol-next.md` §7).
//!
//! A row's key is its second index, which only rises. Within a run (one stretch of the
//! meter's job on one open stream) the job advances it by the seconds the audio clock
//! counted. A new run starts above everything issued and everything the mark reserved:
//! within this process at `last + d`, `d` the whole seconds the boot clock counted since
//! the last index issued; the first run of a process at `max(mark + 1, last + d)`, `d` from
//! the boot clock (same boot), else from trusted wall clocks, else 1 with the gap unknown.
//!
//! The mark is written by the log's write thread, never here: the job asks for the next
//! reservation a minute before its index reaches the stored one, and a run's first rows are
//! held back until the write thread has written that run's anchor or reported that it could
//! not. So a `(run, second)` a client was served is never reused by a different row.

use std::fmt;
use std::sync::Arc;

use ac2_proto::model::{LogClock, LogGap, SplLogRow};
use ac2_proto::units::{MeasId, WallNs};
use ac2_traces::band_log::BandLogRow;
use ac2_traces::log_mark::{Anchor, LogMark};

use super::{LeqLog, NS, started_now};

/// Seconds an anchor reserves ahead of the index it is written at.
pub(crate) const RESERVE_S: u64 = 600;
/// Seconds before the index reaches the stored reservation that the next one is asked for.
pub(crate) const RESERVE_LEAD_S: u64 = 60;

/// Wakes the log's write thread to write the mark asked for.
#[derive(Clone)]
pub(crate) struct Waker(pub(crate) Arc<dyn Fn() + Send + Sync>);

impl fmt::Debug for Waker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Waker")
    }
}

/// The clocks as a run starts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RunClock<'a> {
    /// Boot clock, ms (suspend included).
    pub(crate) boot_ms: u64,
    /// The boot's id.
    pub(crate) boot_id: &'a str,
    /// Wall clock, ns.
    pub(crate) wall_ns: u64,
    /// What the wall clock rests on.
    pub(crate) clock: LogClock,
}

/// Where a run starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunStart {
    pub(crate) run: u32,
    pub(crate) first: u64,
    pub(crate) gap: LogGap,
}

#[derive(Debug, Clone, Copy)]
struct Issued {
    second: u64,
    run: u32,
    boot_ms: u64,
}

/// The newest row a restored log holds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LastRow {
    pub(crate) second: u64,
    pub(crate) run: u32,
    pub(crate) start: WallNs,
    pub(crate) clock: LogClock,
}

#[derive(Debug, Clone)]
struct Restored {
    mark: Option<LogMark>,
    last: Option<LastRow>,
}

/// A log's runs and mark.
#[derive(Debug, Default)]
pub(crate) struct Runs {
    /// The newest run started and its first row's gap.
    open: Option<(u32, LogGap)>,
    highest: Option<u32>,
    issued: Option<Issued>,
    /// A log read back whose first run in this process is still to start.
    restored: Option<Restored>,
    /// The mark read back with the log, standing until this process issues an index.
    read_back: Option<LogMark>,
    /// No window reaches below this index: the first of the newest run whose gap is
    /// unknown.
    pub(super) floor: u64,
    /// Above every row held and every index issued.
    next_free: u64,
    /// The index the stored mark reserves up to (or the one whose write failed: the next
    /// attempt comes with the next reservation).
    reserved: Option<u64>,
    want: Option<LogMark>,
    /// A run whose anchor is awaited: rows wait in `held`.
    awaiting: Option<u32>,
    held: Vec<SplLogRow>,
    held_bands: Vec<BandLogRow>,
    last_spl_run: Option<u32>,
    last_band_run: Option<u32>,
    /// The write thread keeping this log on disk: while it does, a run's first rows wait
    /// for its anchor.
    waker: Option<Waker>,
    /// The mark's writes are failing (logged once per failing stretch).
    failing: bool,
    boot_id: String,
}

impl Runs {
    /// The runs of the meter's next log: none yet, kept by the same write thread.
    pub(super) fn fresh(&self) -> Self {
        Self {
            waker: self.waker.clone(),
            ..Self::default()
        }
    }

    /// For a log read back with `mark` beside it and `last` its newest row: where the first
    /// run of this process starts from.
    fn restore(
        &mut self,
        meas: MeasId,
        mark: Option<LogMark>,
        last: Option<LastRow>,
        started: &mut WallNs,
    ) {
        let mark = match (mark, last) {
            (Some(m), Some(l)) if !m.covers(l.second, l.run) => {
                tracing::warn!(
                    "SPL meter {meas}: the log's high-water mark (run {}, second {}) lies below \
                     its last row (run {}, second {}): rows served but never written are \
                     unknown, so the log starts anew for clients and the next run's gap is \
                     unknown",
                    m.anchor.run,
                    m.anchor.second,
                    l.run,
                    l.second
                );
                *started = started_now();
                None
            }
            (None, Some(_)) => {
                tracing::warn!(
                    "SPL meter {meas}: the log has no readable high-water mark: rows served but \
                     never written are unknown, so the log starts anew for clients and the \
                     next run's gap is unknown"
                );
                *started = started_now();
                None
            }
            (m, _) => m,
        };
        self.highest = mark
            .as_ref()
            .map(|m| m.highest_run)
            .max(last.map(|l| l.run));
        self.next_free = last.map_or(0, |l| l.second + 1);
        self.read_back.clone_from(&mark);
        self.restored = Some(Restored { mark, last });
    }

    /// The next run's start under `now` (see the module).
    fn start(&mut self, now: &RunClock<'_>) -> RunStart {
        let run = self.highest.map_or(0, |h| h.saturating_add(1));
        // Boot-clock times are where a second ends, so the seconds wholly passed since
        // that end are the gap: a stop shorter than a second leaves none.
        let (first, gap) = if let Some(i) = self.issued {
            let d = now.boot_ms.saturating_sub(i.boot_ms) / 1000;
            (i.second + 1 + d, LogGap::Known)
        } else if let Some(r) = self.restored.take() {
            match (r.last, r.mark) {
                (None, m) => (m.map_or(0, |m| m.anchor.second + 1), LogGap::Known),
                (Some(l), None) => (l.second + 1, LogGap::Unknown),
                (Some(l), Some(m)) => {
                    let next = if m.anchor.boot_id == now.boot_id && m.anchor.run == l.run {
                        now.boot_ms
                            .checked_sub(m.anchor.boot_ms_at(l.second))
                            .map(|ms| l.second + 1 + ms / 1000)
                    } else if l.clock.trusted() && now.clock.trusted() {
                        // A row's wall time is where its second starts.
                        now.wall_ns
                            .checked_sub(l.start.0)
                            .map(|ns| l.second + ns / NS)
                            .filter(|n| *n > l.second)
                    } else {
                        None
                    };
                    let above = m.anchor.second + 1;
                    match next {
                        // A crash's reservation may overstate the gap by up to its
                        // length: that only ages old rows out of the windows sooner.
                        Some(n) => (n.max(above), LogGap::Known),
                        None => ((l.second + 1).max(above), LogGap::Unknown),
                    }
                }
            }
        } else {
            (self.next_free, LogGap::Known)
        };
        let first = first.max(self.next_free);
        if gap == LogGap::Unknown {
            self.floor = first;
        }
        self.highest = Some(run);
        self.open = Some((run, gap));
        self.next_free = first;
        self.boot_id = now.boot_id.to_owned();
        RunStart { run, first, gap }
    }

    fn mark_at(&self, run: u32, second: u64, boot_ms: u64) -> LogMark {
        LogMark::new(
            self.highest.unwrap_or(run).max(run),
            Anchor {
                boot_id: self.boot_id.clone(),
                run,
                second,
                boot_ms,
            },
        )
    }

    fn ask(&mut self, m: LogMark) {
        self.want = Some(m);
        if let Some(w) = &self.waker {
            (w.0)();
        }
    }
}

impl LeqLog {
    /// Starts a run at `now`: the meter's job calls this as it starts on an open stream,
    /// on a new log, and when the boot clock ran more than 2 s ahead of the audio clock (a
    /// suspend the backend did not report). The run's anchor is asked of the write
    /// thread; with one keeping the log on disk, its rows wait for it.
    pub(crate) fn begin_run(&mut self, now: &RunClock<'_>) -> RunStart {
        let s = self.runs.start(now);
        // The run's first second ends a second after it starts.
        let m = self.runs.mark_at(
            s.run,
            s.first + RESERVE_S,
            now.boot_ms + 1000 + RESERVE_S * 1000,
        );
        if self.runs.waker.is_some() {
            self.runs.awaiting = Some(s.run);
        }
        self.runs.ask(m);
        s
    }

    /// Second `second` of run `run` ended at boot-clock `boot_ms` (with or without a row):
    /// asks for the next reservation a minute before the stored one is reached.
    pub(crate) fn issue(&mut self, run: u32, second: u64, boot_ms: u64) {
        let r = &mut self.runs;
        r.issued = Some(Issued {
            second,
            run,
            boot_ms,
        });
        r.next_free = r.next_free.max(second + 1);
        let due = r.reserved.is_none_or(|res| second + RESERVE_LEAD_S >= res);
        if r.waker.is_some() && r.want.is_none() && r.awaiting.is_none() && due {
            let m = r.mark_at(run, second + RESERVE_S, boot_ms + RESERVE_S * 1000);
            r.ask(m);
        }
    }

    /// Logs a row (its `second` and `run` set by the job; its `gap` set here on its run's
    /// first row).
    pub(crate) fn push_row(&mut self, mut r: SplLogRow) {
        let runs = &mut self.runs;
        r.gap = (runs.last_spl_run != Some(r.run)).then(|| runs.gap_of(r.run));
        runs.last_spl_run = Some(r.run);
        if runs.awaiting.is_some() {
            runs.held.push(r);
        } else {
            self.push(r);
        }
    }

    /// Logs a band row, as [`Self::push_row`].
    pub(crate) fn push_band_row(&mut self, mut r: BandLogRow) {
        let runs = &mut self.runs;
        r.gap = (runs.last_band_run != Some(r.run)).then(|| runs.gap_of(r.run));
        runs.last_band_run = Some(r.run);
        if runs.awaiting.is_some() {
            runs.held_bands.push(r);
        } else {
            self.push_band(r);
        }
    }

    fn release(&mut self) {
        self.runs.awaiting = None;
        for r in std::mem::take(&mut self.runs.held) {
            self.push(r);
        }
        for b in std::mem::take(&mut self.runs.held_bands) {
            self.push_band(b);
        }
    }

    /// The write thread takes on the log: it is woken when a mark is asked for.
    pub(crate) fn attach(&mut self, w: Waker) {
        if self.runs.want.is_some() {
            (w.0)();
        }
        self.runs.waker = Some(w);
    }

    /// The write thread lets the log go: nothing waits for an anchor any more.
    pub(crate) fn detach(&mut self) {
        self.runs.waker = None;
        self.runs.want = None;
        self.release();
    }

    /// The mark asked for, taken by the write thread.
    pub(crate) fn take_want(&mut self) -> Option<LogMark> {
        self.runs.want.take()
    }

    /// The write thread wrote `m` (or could not, `Err`): its reservation stands (or is
    /// retried with the next), and rows waiting for its run's anchor are served.
    pub(crate) fn mark_written(&mut self, meas: MeasId, m: &LogMark, result: Result<(), String>) {
        let r = &mut self.runs;
        r.reserved = Some(m.anchor.second);
        match result {
            Ok(()) => {
                if r.failing {
                    tracing::info!("SPL meter {meas}: the log's high-water mark is written again");
                }
                r.failing = false;
            }
            Err(e) => {
                if !r.failing {
                    tracing::warn!(
                        "SPL meter {meas}: the log's high-water mark cannot be written ({e}): \
                         the meter goes on logging, but a second served before a power cut \
                         may be served again as another"
                    );
                }
                r.failing = true;
            }
        }
        if r.awaiting.is_some_and(|a| a <= m.anchor.run) {
            self.release();
        }
    }

    /// The mark at the last index issued, as a clean stop writes it; while this process
    /// issued none, the one read back with the log (`None` without one).
    pub(crate) fn exact_mark(&self) -> Option<LogMark> {
        match self.runs.issued {
            Some(i) => Some(self.runs.mark_at(i.run, i.second, i.boot_ms)),
            None => self.runs.read_back.clone(),
        }
    }

    /// The clock became synchronised at second `at` of run `run`, whose wall time then was
    /// `wall`: the run's rows logged unsynced are re-stamped from their distance in seconds
    /// to it (within a run the index keeps to the boot clock) and marked derived. Their
    /// band periods stay as judged.
    pub(crate) fn restamp(&mut self, run: u32, at: u64, wall: WallNs) {
        let stamp = |second: u64| WallNs(wall.0.saturating_sub(at.saturating_sub(second) * NS));
        let spl = self
            .rows
            .iter_mut()
            .rev()
            .chain(self.runs.held.iter_mut().rev());
        for r in spl.filter(|r| r.run == run && r.clock == LogClock::Unsynced) {
            r.start = stamp(r.second);
            r.clock = LogClock::Derived;
        }
        let bands = self
            .bands
            .iter_mut()
            .rev()
            .chain(self.runs.held_bands.iter_mut().rev());
        for r in bands.filter(|r| r.run == run && r.clock == LogClock::Unsynced) {
            r.start = stamp(r.second);
            r.clock = LogClock::Derived;
        }
    }
}

impl Runs {
    fn gap_of(&self, run: u32) -> LogGap {
        match self.open {
            Some((r, g)) if r == run => g,
            _ => LogGap::Known,
        }
    }
}

/// What [`LeqLog::restored`] hands its runs.
pub(super) fn restore(log: &mut LeqLog, meas: MeasId, mark: Option<LogMark>) {
    let spl = log.rows.back().map(|r| LastRow {
        second: r.second,
        run: r.run,
        start: r.start,
        clock: r.clock,
    });
    let band = log.bands.back().map(|r| LastRow {
        second: r.second,
        run: r.run,
        start: r.start,
        clock: r.clock,
    });
    let last = match (spl, band) {
        (Some(a), Some(b)) => Some(if b.second > a.second { b } else { a }),
        (a, b) => a.or(b),
    };
    log.runs.last_spl_run = log.rows.back().map(|r| r.run);
    log.runs.last_band_run = log.bands.back().map(|r| r.run);
    let mut started = log.started;
    log.runs.restore(meas, mark, last, &mut started);
    log.started = started;
}

#[cfg(test)]
impl LeqLog {
    /// A log holding `rows` and `bands` as given (keys included), as a test builds one.
    pub(crate) fn from_rows(rows: Vec<SplLogRow>, bands: Vec<BandLogRow>) -> Self {
        let mut l = Self::default();
        for r in rows {
            l.push(r);
        }
        for b in bands {
            l.push_band(b);
        }
        l.runs.last_spl_run = l.rows.back().map(|r| r.run);
        l.runs.last_band_run = l.bands.back().map(|r| r.run);
        l.runs.next_free = l
            .rows
            .back()
            .map(|r| r.second + 1)
            .max(l.bands.back().map(|r| r.second + 1))
            .unwrap_or(0);
        l
    }
}

#[cfg(test)]
mod tests;
