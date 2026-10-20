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

mod judge;
mod replay;

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
    #[cfg(test)]
    pub(crate) fn allowed_dbfs(&self) -> Option<f64> {
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

pub use judge::{
    Judgement, Latch, PEAK_HOLD_S, RELEASE_DB, RELEASE_HOLD_S, RecentPeaks, Verdict, judge,
    judge_window, round_tenth,
};
pub use replay::{LogReplay, LogTotal};

#[cfg(test)]
mod tests;
