//! Replay of an SPL meter's logged seconds into the same windows the job computed.

use super::{NS, RollingLeq, Second, Sum, WindowSpec, w_index};
use crate::spectrum::power_dbfs;
use crate::weighting::Weighting;

/// The windows second by second as an SPL meter's job computed them, replayed from its log
/// (each logged second's wall-time start and energy, oldest first), so a client that was
/// not there when they were computed gets the same values and judgements.
///
/// The job pushes every second, logged or not; a second without a row (nothing measured
/// in it) is a gap. A stretch of seconds without rows is where the meter was stopped or the
/// daemon was down, and the job that starts after it [refills](RollingLeq::refill) its
/// windows from the rows in its span: the slots before the oldest of those count as not
/// elapsed. The replay does the same, cheaply: the slots it pushes for the stretch are gaps
/// either way, so only the elapsed time changes ([`RollingLeq::forget_before`]); a stretch
/// longer than the longest window empties the windows.
#[derive(Debug, Clone)]
pub struct LogReplay {
    ring: RollingLeq,
    /// Start of the previous row, ns.
    prev: Option<u64>,
    /// Slots pushed since the replay began (never reset).
    slot: u64,
    /// Slot numbers of the rows within the newest capacity slots, oldest first.
    rows: std::collections::VecDeque<u64>,
    /// The windows no longer depend on anything before the first row replayed.
    settled: bool,
}

impl LogReplay {
    /// Windows `specs` with headroom over `horizon` s, as [`RollingLeq::new`].
    /// `from_log_start`: the first row given is the log's first, so nothing came before it.
    pub fn new(specs: &[WindowSpec], horizon: u32, from_log_start: bool) -> Self {
        Self {
            ring: RollingLeq::new(specs, horizon),
            prev: None,
            slot: 0,
            rows: std::collections::VecDeque::new(),
            settled: from_log_start,
        }
    }

    /// Adds the log's next row: a second that started at `start_ns` (wall time).
    pub fn push(&mut self, start_ns: u64, s: Second) {
        let cap = u64::from(self.ring.capacity());
        if let Some(prev) = self.prev {
            // Rows are a second apart; a longer step is seconds without a row.
            let step = ((start_ns.saturating_sub(prev) + NS / 2) / NS).max(1);
            let missing = step - 1;
            if missing >= cap {
                // Nothing logged within the longest window before this row: the job that
                // logged it started on empty windows.
                self.ring.clear();
                self.rows.clear();
                self.slot += missing;
                self.settled = true;
            } else if missing > 0 {
                for _ in 0..missing {
                    self.ring.push(Second::GAP);
                }
                self.slot += missing;
                // The job restarted here and refilled its windows from the rows within
                // its span; the slots before the oldest of them are gaps.
                let span_start = self.slot.saturating_sub(cap);
                while self.rows.front().is_some_and(|&r| r < span_start) {
                    self.rows.pop_front();
                }
                if let Some(&first) = self.rows.front() {
                    self.ring.forget_before(self.slot - first);
                }
            }
        }
        self.ring.push(s);
        self.rows.push_back(self.slot);
        self.slot += 1;
        while self.rows.front().is_some_and(|&r| r + cap < self.slot) {
            self.rows.pop_front();
        }
        if self.slot >= cap {
            self.settled = true;
        }
        self.prev = Some(start_ns);
    }

    /// The windows as of the newest row.
    pub fn windows(&self) -> &RollingLeq {
        &self.ring
    }

    /// Whether the windows are what the job computed: true from the log's first row on,
    /// or once the longest window holds only rows replayed (or was emptied by a stretch
    /// without rows). Before that they lack what the log held before the first row given.
    pub fn settled(&self) -> bool {
        self.settled
    }
}

/// The energy average over a whole log: per-weighting energy and measured time of every
/// second added, less those removed (a log trimmed at its oldest end). Gaps add nothing,
/// so the level is over the measured time, as a window's.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LogTotal {
    sums: [Sum; 3],
}

impl LogTotal {
    /// The exact total of `seconds`.
    pub fn of<'a>(seconds: impl IntoIterator<Item = &'a Second>) -> Self {
        let mut t = Self::default();
        for s in seconds {
            t.add(s);
        }
        t
    }

    /// Adds a second.
    pub fn add(&mut self, s: &Second) {
        for (w, sum) in self.sums.iter_mut().enumerate() {
            sum.add(s.energy[w], s.measured);
        }
    }

    /// Takes away a second added before.
    pub fn remove(&mut self, s: &Second) {
        for (w, sum) in self.sums.iter_mut().enumerate() {
            sum.add(-s.energy[w], -s.measured);
        }
    }

    /// Measured time, s.
    pub fn measured(&self) -> f64 {
        self.sums[0].measured()
    }

    /// Energy of weighting `w` (FS²·s).
    pub fn energy(&self, w: Weighting) -> f64 {
        self.sums[w_index(w)].energy()
    }

    /// Leq of weighting `w` over the measured time, dBFS; NaN when nothing was measured.
    pub fn level_dbfs(&self, w: Weighting) -> f64 {
        let m = self.measured();
        if m > 0.0 {
            power_dbfs(self.energy(w) / m)
        } else {
            f64::NAN
        }
    }
}
