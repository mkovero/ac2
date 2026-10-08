//! Rolling Leq windows over one-second blocks, limits and headroom (`docs/design/leq.md`).
//!
//! [`SecondIntegrator`] sums an SPL meter's weighted signal into one-second blocks of A-,
//! C- and Z-weighted energy (each with its measured time, so a capture gap is never
//! silence).
//! [`RollingLeq`] keeps the newest seconds in a ring sized for the longest window and
//! answers, per window, the Leq over the window (over the elapsed time while it fills), the
//! headroom for a horizon and the time to recover at a level. [`judge_window`] judges a
//! full window on its Leq and a filling one on its energy budget. Nothing allocates after
//! construction.
//!
//! Levels are dBFS on the scale of decision 4a (`10·lg(2·ms)`), as everywhere in
//! [`crate::spl`]; a limit in dB SPL is converted by the caller with the sensitivity.

use crate::spectrum::power_dbfs;
use crate::weighting::Weighting;

/// The weightings every second is integrated in, in this order.
pub const WEIGHTINGS: [Weighting; 3] = [Weighting::A, Weighting::C, Weighting::Z];

fn w_index(w: Weighting) -> usize {
    match w {
        Weighting::A => 0,
        Weighting::C => 1,
        Weighting::Z => 2,
    }
}

/// Mean square (FS²) of a dBFS level: the inverse of [`power_dbfs`].
pub fn mean_square(level_dbfs: f64) -> f64 {
    10f64.powf(level_dbfs / 10.0) / 2.0
}

/// One second of weighted energy, and the second's highest C-weighted peak and A-weighted
/// Fast level (the quantities peak limits are set on: LCpeak, LAFmax).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Second {
    /// Σ y² / fs per weighting of [`WEIGHTINGS`] (FS²·s).
    pub energy: [f64; 3],
    /// Time actually measured within the second (s); 0 for a second lost to a gap.
    pub measured: f64,
    /// Highest |y_C|² of the second (the C-weighted peak, squared; FS²).
    pub c_peak_sq: f64,
    /// Highest A-weighted Fast mean square of the second (FS²).
    pub af_max_ms: f64,
}

impl Second {
    /// A second lost to a capture gap: no energy, nothing measured.
    pub const GAP: Self = Self {
        energy: [0.0; 3],
        measured: 0.0,
        c_peak_sq: 0.0,
        af_max_ms: 0.0,
    };

    /// LCpeak of the second, dBFS (`10·lg(2·peak²)`, as [`crate::spl::PeakDetector`]);
    /// −∞ without one.
    pub fn lcpeak_dbfs(&self) -> f64 {
        power_dbfs(self.c_peak_sq)
    }

    /// LAFmax of the second, dBFS; −∞ without one.
    pub fn lafmax_dbfs(&self) -> f64 {
        power_dbfs(self.af_max_ms)
    }

    /// From per-weighting levels over `measured` seconds (a log row read back).
    pub fn from_levels(levels_dbfs: [f64; 3], measured: f64) -> Self {
        let m = measured.clamp(0.0, 1.0);
        let e = |l: f64| {
            let ms = mean_square(l);
            if ms.is_finite() { ms * m } else { 0.0 }
        };
        Self {
            energy: [e(levels_dbfs[0]), e(levels_dbfs[1]), e(levels_dbfs[2])],
            measured: m,
            c_peak_sq: 0.0,
            af_max_ms: 0.0,
        }
    }

    /// With the second's LCpeak and LAFmax (dBFS) as logged.
    pub fn with_maxima(self, lcpeak_dbfs: f64, lafmax_dbfs: f64) -> Self {
        let ms = |l: f64| {
            let v = mean_square(l);
            if v.is_finite() { v } else { 0.0 }
        };
        Self {
            c_peak_sq: ms(lcpeak_dbfs),
            af_max_ms: ms(lafmax_dbfs),
            ..self
        }
    }

    /// Energy of weighting `w` (FS²·s).
    pub fn energy(&self, w: Weighting) -> f64 {
        self.energy[w_index(w)]
    }

    /// Leq over the measured part, dBFS; NaN when nothing was measured.
    pub fn level_dbfs(&self, w: Weighting) -> f64 {
        if self.measured > 0.0 {
            power_dbfs(self.energy(w) / self.measured)
        } else {
            f64::NAN
        }
    }
}

/// One second of energy on a few channels, as a rolling window sums it: a [`Second`]'s
/// weightings, or a band meter's bands (`crate::band_leq`). A gap slot has neither energy
/// nor measured time.
pub trait Slot: Copy {
    /// A second lost to a gap.
    const GAP: Self;
    /// Energy of channel `ch` (FS²·s).
    fn slot_energy(&self, ch: usize) -> f64;
    /// Time measured within the second (s).
    fn slot_measured(&self) -> f64;
}

impl Slot for Second {
    const GAP: Self = Second::GAP;

    #[inline]
    fn slot_energy(&self, ch: usize) -> f64 {
        self.energy[ch]
    }

    #[inline]
    fn slot_measured(&self) -> f64 {
        self.measured
    }
}

/// Sums A-, C- and Z-weighted energy into one-second [`Second`]s on a grid of whole seconds
/// from the first sample. The weighted samples come from the meter's own weighting chain
/// ([`crate::spl::SplMeter::process`]), so the log and the meter read one filtered signal.
#[derive(Debug, Clone)]
pub struct SecondIntegrator {
    fs: f64,
    per_second: u64,
    /// Samples of the current second passed (measured or skipped).
    pos: u64,
    measured: u64,
    acc: [f64; 3],
    /// Highest C peak² and A Fast mean square so far in the second.
    max: [f64; 2],
}

impl SecondIntegrator {
    /// At `fs` Hz (a whole number of samples per second).
    pub fn new(fs: f64) -> Self {
        Self {
            fs,
            per_second: (fs.round() as u64).max(1),
            pos: 0,
            measured: 0,
            acc: [0.0; 3],
            max: [0.0; 2],
        }
    }

    /// Samples into the current second.
    pub fn position(&self) -> u64 {
        self.pos
    }

    /// Samples per second.
    pub fn samples_per_second(&self) -> u64 {
        self.per_second
    }

    /// Samples left in the current second.
    pub fn room(&self) -> u64 {
        self.per_second - self.pos
    }

    /// Adds `n` measured samples (at most [`Self::room`]) whose Σy² per weighting of
    /// [`WEIGHTINGS`] is `energy` and whose highest C peak² and A Fast mean square are
    /// `max`; a completed second goes to `emit`.
    #[inline]
    pub fn add(&mut self, energy: [f64; 3], max: [f64; 2], n: u64, emit: &mut impl FnMut(Second)) {
        debug_assert!(n <= self.room());
        for (a, e) in self.acc.iter_mut().zip(energy) {
            *a += e;
        }
        for (a, m) in self.max.iter_mut().zip(max) {
            *a = a.max(m);
        }
        self.measured += n;
        self.pos += n;
        if self.pos == self.per_second {
            self.flush(emit);
        }
    }

    fn flush(&mut self, emit: &mut impl FnMut(Second)) {
        let fs = self.fs;
        emit(Second {
            energy: [self.acc[0] / fs, self.acc[1] / fs, self.acc[2] / fs],
            measured: self.measured as f64 / self.per_second as f64,
            c_peak_sq: self.max[0],
            af_max_ms: self.max[1],
        });
        self.acc = [0.0; 3];
        self.max = [0.0; 2];
        self.measured = 0;
        self.pos = 0;
    }

    /// `samples` were lost (a capture discontinuity): the second grid moves on without
    /// energy or measured time; completed seconds go to `emit`.
    pub fn skip(&mut self, samples: u64, mut emit: impl FnMut(Second)) {
        let mut left = samples;
        while left > 0 {
            let room = self.per_second - self.pos;
            if left < room {
                self.pos += left;
                return;
            }
            left -= room;
            self.pos = self.per_second;
            self.flush(&mut emit);
        }
    }
}

/// One rolling window: its length and weighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowSpec {
    /// Length in seconds (≥ 1).
    pub seconds: u32,
    /// Frequency weighting.
    pub weighting: Weighting,
}

/// A window's current value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowValue {
    /// Leq over the measured time in the window, dBFS; NaN when nothing was measured.
    pub leq_dbfs: f64,
    /// Seconds of the window covered so far (`< seconds` while it fills).
    pub elapsed: u32,
    /// Measured seconds within those.
    pub measured: f64,
    /// Energy over the measured time (FS²·s).
    pub energy: f64,
    /// The window's length, s.
    pub seconds: u32,
}

impl WindowValue {
    /// Some of the elapsed time was not measured (capture gaps, the meter stopped).
    pub fn incomplete(&self) -> bool {
        self.measured < f64::from(self.elapsed) - 1e-6
    }

    /// The window has not yet covered its whole length.
    pub fn filling(&self) -> bool {
        self.elapsed < self.seconds
    }

    /// Seconds until the window covers its whole length (0 once full).
    pub fn remaining(&self) -> u32 {
        self.seconds.saturating_sub(self.elapsed)
    }

    /// The Leq the window ends at when the rest of it is measured silence, dBFS: the energy
    /// so far over the measured time plus the remaining seconds. The least the full window
    /// can read, so a value above the limit makes going over a certainty; equal to the Leq
    /// once the window is full. A gap already in the window adds no time, as in the Leq.
    /// NaN when nothing was measured.
    pub fn least_dbfs(&self) -> f64 {
        if self.measured > 0.0 {
            power_dbfs(self.energy / (self.measured + f64::from(self.remaining())))
        } else {
            f64::NAN
        }
    }

    /// Seconds until the energy reaches the budget of a filling window, `limit_ms` over its
    /// measured time plus the remaining seconds, when the rest is played at the mean power
    /// so far; `None` for a full window, without energy, or when the budget outlasts the
    /// window (the pace keeps it at or under the limit). `Some(0)` once it is spent.
    pub fn over_in(&self, limit_ms: f64) -> Option<f64> {
        if !self.filling() || self.measured <= 0.0 || self.energy <= 0.0 {
            return None;
        }
        let pace = self.energy / self.measured;
        let budget = limit_ms * (self.measured + f64::from(self.remaining()));
        let t = ((budget - self.energy) / pace).max(0.0);
        (t < f64::from(self.remaining())).then_some(t)
    }
}

/// What the next horizon may hold for a window to stay at or below its limit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Headroom {
    /// The highest steady mean square (FS²) for the whole horizon.
    Allowed {
        /// Mean square.
        ms: f64,
    },
    /// Over the limit at the end of the horizon whatever is played. Playing at the limit,
    /// the window is back at it after `recover_s` seconds.
    CannotRecover {
        /// Seconds.
        recover_s: u32,
    },
}

impl Headroom {
    /// The allowed level in dBFS, if any.
    pub fn allowed_dbfs(&self) -> Option<f64> {
        match self {
            Headroom::Allowed { ms } => Some(power_dbfs(*ms)),
            Headroom::CannotRecover { .. } => None,
        }
    }
}

/// Compensated running sum of energy and measured time.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Sum {
    e: f64,
    ec: f64,
    m: f64,
    mc: f64,
}

/// Neumaier step: adds `v` to `(s, c)`.
#[inline]
fn neumaier(s: &mut f64, c: &mut f64, v: f64) {
    let t = *s + v;
    if s.abs() >= v.abs() {
        *c += (*s - t) + v;
    } else {
        *c += (v - t) + *s;
    }
    *s = t;
}

impl Sum {
    fn add(&mut self, e: f64, m: f64) {
        neumaier(&mut self.e, &mut self.ec, e);
        neumaier(&mut self.m, &mut self.mc, m);
    }

    fn energy(&self) -> f64 {
        (self.e + self.ec).max(0.0)
    }

    fn measured(&self) -> f64 {
        (self.m + self.mc).max(0.0)
    }
}

#[derive(Debug, Clone)]
struct Acc {
    /// Length, s (≥ 1).
    seconds: u32,
    /// Channel of the slot summed.
    ch: usize,
    /// Slots that stay in the window over the horizon: `seconds − horizon`, or 0.
    keep: u32,
    full: Sum,
    kept: Sum,
    since_exact: u32,
}

/// The newest seconds and the rolling windows over them, each window on one channel of the
/// slot (a weighting of a [`Second`], a band of a band second).
#[derive(Debug, Clone)]
pub struct RollingLeq<S: Slot = Second> {
    ring: Vec<S>,
    /// Next slot written.
    head: usize,
    /// Slots pushed since the start (or the last [`Self::clear`]).
    pushed: u64,
    horizon: u32,
    windows: Vec<Acc>,
}

impl RollingLeq<Second> {
    /// Windows `specs` (lengths clamped to ≥ 1 s) with headroom over `horizon` seconds
    /// (≥ 1); the ring holds the longest window.
    pub fn new(specs: &[WindowSpec], horizon: u32) -> Self {
        Self::of_channels(
            specs.iter().map(|s| (s.seconds, w_index(s.weighting))),
            horizon,
        )
    }

    /// The windows, in construction order.
    pub fn specs(&self) -> impl Iterator<Item = WindowSpec> + '_ {
        self.windows.iter().map(|a| WindowSpec {
            seconds: a.seconds,
            weighting: WEIGHTINGS[a.ch],
        })
    }

    /// Refills the windows with the logged seconds `newest_first` (each with the wall time
    /// of its start, ns, newest first) that fall in the [`Self::capacity`] seconds before
    /// `now_ns`, placed by wall time: logged seconds are a second apart and half a second
    /// either way decides the slot, two in one slot add up, and slots without one are gaps.
    /// The windows then count as elapsed from the oldest logged second inside that span —
    /// what a meter's job does whenever it starts on a log it did not write itself.
    pub fn refill(&mut self, newest_first: impl IntoIterator<Item = (u64, Second)>, now_ns: u64) {
        self.clear();
        let cap = u64::from(self.capacity());
        let span_start = now_ns.saturating_sub(cap * NS);
        let mut slots: Vec<Option<Second>> = vec![None; cap as usize];
        for (start, s) in newest_first {
            let Some(off) = (start + NS / 2).checked_sub(span_start) else {
                break;
            };
            let k = off / NS;
            if k >= cap {
                continue;
            }
            let slot = &mut slots[k as usize];
            *slot = Some(match slot {
                Some(o) => Second {
                    energy: [
                        o.energy[0] + s.energy[0],
                        o.energy[1] + s.energy[1],
                        o.energy[2] + s.energy[2],
                    ],
                    measured: o.measured + s.measured,
                    c_peak_sq: o.c_peak_sq.max(s.c_peak_sq),
                    af_max_ms: o.af_max_ms.max(s.af_max_ms),
                },
                None => s,
            });
        }
        let Some(first) = slots.iter().position(Option::is_some) else {
            return;
        };
        for s in &slots[first..] {
            self.push(s.unwrap_or(Second::GAP));
        }
    }
}

impl<S: Slot> RollingLeq<S> {
    /// Windows of `(seconds, channel)` (lengths clamped to ≥ 1 s) with headroom over
    /// `horizon` seconds (≥ 1); the ring holds the longest window.
    pub fn of_channels(windows: impl IntoIterator<Item = (u32, usize)>, horizon: u32) -> Self {
        let horizon = horizon.max(1);
        let windows: Vec<Acc> = windows
            .into_iter()
            .map(|(seconds, ch)| {
                let seconds = seconds.max(1);
                Acc {
                    seconds,
                    ch,
                    keep: seconds.saturating_sub(horizon),
                    full: Sum::default(),
                    kept: Sum::default(),
                    since_exact: 0,
                }
            })
            .collect();
        let cap = windows.iter().map(|a| a.seconds).max().unwrap_or(1);
        Self {
            ring: vec![S::GAP; cap as usize],
            head: 0,
            pushed: 0,
            horizon,
            windows,
        }
    }

    /// Ring length: the longest window, s.
    pub fn capacity(&self) -> u32 {
        self.ring.len() as u32
    }

    /// Headroom horizon, s.
    pub fn horizon(&self) -> u32 {
        self.horizon
    }

    /// Number of windows.
    pub fn len(&self) -> usize {
        self.windows.len()
    }

    /// True without windows.
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    /// Seconds pushed since the start.
    pub fn pushed(&self) -> u64 {
        self.pushed
    }

    /// Slot `k` seconds back from the newest (`k = 0` is the newest); `None` before the
    /// start.
    fn back(&self, k: u32) -> Option<&S> {
        if u64::from(k) >= self.pushed || k as usize >= self.ring.len() {
            return None;
        }
        let n = self.ring.len();
        Some(&self.ring[(self.head + n - 1 - k as usize) % n])
    }

    /// Exact sums over the newest `len` slots of weighting `w`.
    fn exact(&self, w: usize, len: u32) -> Sum {
        let mut s = Sum::default();
        for k in 0..len {
            match self.back(k) {
                Some(x) => s.add(x.slot_energy(w), x.slot_measured()),
                None => break,
            }
        }
        s
    }

    /// Adds the newest second.
    pub fn push(&mut self, s: S) {
        // The slots leaving each window are read before the new one may overwrite them.
        for i in 0..self.windows.len() {
            let (n, keep, w) = {
                let a = &self.windows[i];
                (a.seconds, a.keep, a.ch)
            };
            // A window of n slots loses its oldest, n − 1 back, once it is full.
            let out_full = self
                .back(n - 1)
                .copied()
                .filter(|_| self.pushed >= u64::from(n));
            let out_kept = (keep > 0 && self.pushed >= u64::from(keep))
                .then(|| self.back(keep - 1).copied())
                .flatten();
            let a = &mut self.windows[i];
            let (e, m) = (s.slot_energy(w), s.slot_measured());
            a.full.add(e, m);
            if let Some(o) = out_full {
                a.full.add(-o.slot_energy(w), -o.slot_measured());
            }
            if keep > 0 {
                a.kept.add(e, m);
                if let Some(o) = out_kept {
                    a.kept.add(-o.slot_energy(w), -o.slot_measured());
                }
            }
        }
        let n = self.ring.len();
        self.ring[self.head] = s;
        self.head = (self.head + 1) % n;
        self.pushed += 1;
        // Subtracting what entered long ago leaves rounding behind; an exact sum every
        // window length bounds it.
        for i in 0..self.windows.len() {
            let a = &mut self.windows[i];
            a.since_exact += 1;
            if a.since_exact >= a.seconds {
                let (w, n, keep) = (a.ch, a.seconds, a.keep);
                let full = self.exact(w, n);
                let kept = self.exact(w, keep);
                let a = &mut self.windows[i];
                a.full = full;
                a.kept = kept;
                a.since_exact = 0;
            }
        }
    }

    /// Empties every window.
    pub fn clear(&mut self) {
        self.ring.iter_mut().for_each(|s| *s = S::GAP);
        self.head = 0;
        self.pushed = 0;
        for a in &mut self.windows {
            a.full = Sum::default();
            a.kept = Sum::default();
            a.since_exact = 0;
        }
    }

    /// Counts the windows as elapsed from `keep` slots back at most, as if they had been
    /// refilled from those slots alone. Only for slots older than that which are gaps: their
    /// sums are then unchanged and only the elapsed time (and with it the judgement of a
    /// filling window) differs.
    fn forget_before(&mut self, keep: u64) {
        self.pushed = self.pushed.min(keep);
    }

    /// Value of window `i`.
    ///
    /// # Panics
    /// If `i` is not a window index.
    pub fn value(&self, i: usize) -> WindowValue {
        let a = &self.windows[i];
        let m = a.full.measured();
        let e = a.full.energy();
        WindowValue {
            leq_dbfs: if m > 0.0 { power_dbfs(e / m) } else { f64::NAN },
            elapsed: self.pushed.min(u64::from(a.seconds)) as u32,
            measured: m,
            energy: e,
            seconds: a.seconds,
        }
    }

    /// Headroom of window `i` against a limit given as a mean square (FS²): the highest
    /// steady mean square for the next horizon that leaves the window at the limit, or how
    /// long recovery takes at the limit when no level can.
    ///
    /// A window still filling for at least the horizon loses nothing before it is full and
    /// is judged on its budget (`P · (measured + remaining)`, [`judge_window`]), so its level
    /// is the one that, held until the window is full, spends exactly what is left of it:
    /// `(P·(M + r) − E) / r` over the remaining `r` seconds.
    ///
    /// # Panics
    /// If `i` is not a window index.
    pub fn headroom(&self, i: usize, limit_ms: f64) -> Headroom {
        let a = &self.windows[i];
        let rem = u64::from(a.seconds).saturating_sub(self.pushed);
        let x = if rem >= u64::from(self.horizon) {
            let r = rem as f64;
            (limit_ms * (a.full.measured() + r) - a.full.energy()) / r
        } else if a.keep == 0 {
            // The whole window is replaced within the horizon.
            return Headroom::Allowed { ms: limit_ms };
        } else {
            let h = f64::from(self.horizon);
            (limit_ms * (a.kept.measured() + h) - a.kept.energy()) / h
        };
        if x > 0.0 {
            Headroom::Allowed { ms: x }
        } else {
            Headroom::CannotRecover {
                recover_s: self
                    .recover_time(i, limit_ms, limit_ms)
                    .unwrap_or(a.seconds),
            }
        }
    }

    /// Seconds from now until window `i` is at or below `limit_ms` when `level_ms` is
    /// played steadily from now on: the smallest t ≥ the horizon with
    /// `E_(N−t) + t·level ≤ limit·(M_(N−t) + t)` over the newest N − t slots. `None` when
    /// the level itself is above the limit and the window never gets there.
    ///
    /// # Panics
    /// If `i` is not a window index.
    pub fn recover_time(&self, i: usize, limit_ms: f64, level_ms: f64) -> Option<u32> {
        let a = &self.windows[i];
        let n = a.seconds;
        let h = self.horizon.min(n);
        let mut best: Option<u32> = None;
        let mut e = 0.0;
        let mut ec = 0.0;
        let mut m = 0.0;
        let mut mc = 0.0;
        // j = newest slots kept, t = n − j; scanning j up finds the latest-kept (earliest)
        // time that satisfies the condition.
        for j in 0..=(n - h) {
            if j > 0
                && let Some(x) = self.back(j - 1)
            {
                neumaier(&mut e, &mut ec, x.slot_energy(a.ch));
                neumaier(&mut m, &mut mc, x.slot_measured());
            }
            let t = f64::from(n - j);
            if (e + ec) + t * level_ms <= limit_ms * ((m + mc) + t) {
                best = Some(n - j);
            }
        }
        best
    }
}

/// Nanoseconds per second: logged seconds carry their wall time in ns.
const NS: u64 = 1_000_000_000;

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

/// A window's state against its limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Judgement {
    /// More than the margin below the limit.
    Ok,
    /// Within the margin below the limit, or at it.
    Near,
    /// Above the limit.
    Over,
}

/// `v` at the 0.1 dB resolution levels are shown at.
pub fn round_tenth(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// Judges a Leq against `limit_db` with a warn `margin_db`, at the displayed 0.1 dB
/// resolution so the state never disagrees with the number shown. `None` without a value.
pub fn judge(leq_db: f64, limit_db: f64, margin_db: f64) -> Option<Judgement> {
    if !leq_db.is_finite() {
        return (leq_db == f64::NEG_INFINITY).then_some(Judgement::Ok);
    }
    let l = round_tenth(leq_db);
    Some(if l > limit_db {
        Judgement::Over
    } else if l > limit_db - margin_db.max(0.0) {
        Judgement::Near
    } else {
        Judgement::Ok
    })
}

/// A window's judgement, with how a filling window stands on its budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Verdict {
    pub judgement: Judgement,
    /// Filling, not over yet, and the Leq so far above the limit: at the pace so far the
    /// full window ends over it (`judgement` is then [`Judgement::Near`]).
    pub on_course: bool,
}

/// Judges window value `v` against `limit_db` with a warn `margin_db`, the window's levels
/// raised by `offset_db` (the sensitivity, dBFS → dB SPL), at the displayed 0.1 dB
/// resolution.
///
/// A full window is judged on its Leq ([`judge`]). Limits are defined on full windows, so a
/// filling one is over only when the full window must end over the limit even if the rest
/// of it is silent ([`WindowValue::least_dbfs`] over the limit: its energy has spent the
/// budget `limit · (measured + remaining)`). Its Leq so far is what it ends at if the rest
/// goes on at the same mean power: above the limit it is on course to go over (near),
/// within the margin near, else ok. Once full, the least level is the Leq and both rules
/// agree. `None` without a value.
pub fn judge_window(
    v: &WindowValue,
    offset_db: f64,
    limit_db: f64,
    margin_db: f64,
) -> Option<Verdict> {
    let j = judge(v.leq_dbfs + offset_db, limit_db, margin_db)?;
    let certain = !v.filling()
        || judge(v.least_dbfs() + offset_db, limit_db, margin_db) == Some(Judgement::Over);
    Some(match j {
        Judgement::Over if !certain => Verdict {
            judgement: Judgement::Near,
            on_course: true,
        },
        j => Verdict {
            judgement: j,
            on_course: false,
        },
    })
}

/// Seconds a peak limit is judged over: its level is the highest LCpeak (LAFmax) of the
/// newest this many seconds. A peak is an instant: judged on its own second it would be
/// over for one second and back the next, too short to be seen from a desk and a flicker
/// with every kick drum near the limit. Held this long, the same dwell as
/// [`RELEASE_HOLD_S`], one peak over keeps the state over long enough to be noticed, and
/// peaks recurring within the hold keep it over without toggling.
pub const PEAK_HOLD_S: usize = 10;

/// The newest [`PEAK_HOLD_S`] seconds' LCpeak and LAFmax. No allocation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PeakHold {
    ring: [[f64; 2]; PEAK_HOLD_S],
    head: usize,
    len: usize,
}

impl PeakHold {
    /// Adds the newest second (a gap adds nothing: its maxima are zero).
    pub fn push(&mut self, s: &Second) {
        self.ring[self.head] = [s.c_peak_sq, s.af_max_ms];
        self.head = (self.head + 1) % PEAK_HOLD_S;
        self.len = (self.len + 1).min(PEAK_HOLD_S);
    }

    /// Forgets every second.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// The highest LCpeak and LAFmax over the seconds held, dBFS; NaN before any second,
    /// −∞ when none of them had signal.
    pub fn max_dbfs(&self) -> [f64; 2] {
        if self.len == 0 {
            return [f64::NAN; 2];
        }
        let mut m = [0.0f64; 2];
        for k in 0..self.len {
            let i = (self.head + PEAK_HOLD_S - 1 - k) % PEAK_HOLD_S;
            m[0] = m[0].max(self.ring[i][0]);
            m[1] = m[1].max(self.ring[i][1]);
        }
        m.map(power_dbfs)
    }
}

/// A judged state is lowered at once only when its level is this far under the state's
/// boundary (the limit for over, the limit less the margin for near). A full window's Leq
/// moves by about `4.34 · (10^(Δ/10) − 1) / N` dB in a second whose level is Δ dB off the
/// window's: a 6 dB louder second moves a 1 min window 0.22 dB, a 15 min one 0.015 dB. Three
/// display steps (0.3 dB) are more than one loud second can undo for any window of a minute
/// or more, so a window let go this far below does not come straight back.
pub const RELEASE_DB: f64 = 0.3;

/// …or when it has stayed under the boundary this many seconds in a row. A long window
/// drifting across its limit dithers between the two 0.1 dB steps either side for a few
/// seconds (its per-second movement is far below the display step, a few hundredths of a
/// dB for 15 min); 10 s outlasts that, and is short against every window a rule defines
/// (15 min and up: about 1 %), so "recovered" is never late by more than a glance.
pub const RELEASE_HOLD_S: u32 = 10;

fn rank(j: Judgement) -> u8 {
    match j {
        Judgement::Ok => 0,
        Judgement::Near => 1,
        Judgement::Over => 2,
    }
}

/// A window's judgement with hysteresis, one call per second: a state rises at once (an
/// alarm is never late) and is lowered only when the level is [`RELEASE_DB`] under the
/// state's boundary, or has been under it for [`RELEASE_HOLD_S`] seconds in a row. A window
/// hovering at its limit thus reports at most one over / recovered pair per hold, instead of
/// one every time its rounded Leq crosses the limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Latch {
    held: Option<Judgement>,
    /// Seconds in a row the judgement has been below the held one.
    under: u32,
}

impl Latch {
    /// Holding `j` (a job that carries on from a state another run reported).
    pub fn holding(j: Option<Judgement>) -> Self {
        Self { held: j, under: 0 }
    }

    /// The state held.
    pub fn held(&self) -> Option<Judgement> {
        self.held
    }

    /// This second's verdict `raw` ([`judge_window`]) on a window whose Leq is `leq_db`
    /// (the unit of `limit_db`), against `limit_db` with warn `margin_db`: the verdict with
    /// the held state. A lowered state not yet released is reported as held (not on course).
    pub fn judge(
        &mut self,
        raw: Option<Verdict>,
        leq_db: f64,
        limit_db: f64,
        margin_db: f64,
    ) -> Option<Verdict> {
        let Some(v) = raw else {
            *self = Self::default();
            return None;
        };
        let held = match self.held {
            Some(h) if rank(v.judgement) < rank(h) => h,
            _ => {
                *self = Self::holding(Some(v.judgement));
                return Some(v);
            }
        };
        let boundary = match held {
            Judgement::Over => limit_db,
            _ => limit_db - margin_db.max(0.0),
        };
        self.under += 1;
        let far = leq_db.is_finite() && round_tenth(leq_db) <= boundary - RELEASE_DB + 1e-9;
        let far = far || leq_db == f64::NEG_INFINITY;
        if far || self.under >= RELEASE_HOLD_S {
            *self = Self::holding(Some(v.judgement));
            return Some(v);
        }
        Some(Verdict {
            judgement: held,
            on_course: false,
        })
    }
}

#[cfg(test)]
mod tests;
