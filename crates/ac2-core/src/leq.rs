//! Rolling Leq windows over one-second blocks, limits and headroom (`docs/design/leq.md`).
//!
//! [`SecondIntegrator`] turns raw input into one-second blocks of A-, C- and Z-weighted
//! energy (each with its measured time, so a capture gap is never silence).
//! [`RollingLeq`] keeps the newest seconds in a ring sized for the longest window and
//! answers, per window, the Leq over the window (over the elapsed time while it fills), the
//! headroom for a horizon and the time to recover at a level. Nothing allocates after
//! construction.
//!
//! Levels are dBFS on the scale of decision 4a (`10·lg(2·ms)`), as everywhere in
//! [`crate::spl`]; a limit in dB SPL is converted by the caller with the sensitivity.

use crate::mic_curve::PartitionedFir;
use crate::spectrum::power_dbfs;
use crate::weighting::{Weighting, WeightingError, WeightingFilter};

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

/// One second of weighted energy.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Second {
    /// Σ y² / fs per weighting of [`WEIGHTINGS`] (FS²·s).
    pub energy: [f64; 3],
    /// Time actually measured within the second (s); 0 for a second lost to a gap.
    pub measured: f64,
}

impl Second {
    /// A second lost to a capture gap: no energy, nothing measured.
    pub const GAP: Self = Self {
        energy: [0.0; 3],
        measured: 0.0,
    };

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

/// Integrates the input into one-second [`Second`]s of A-, C- and Z-weighted energy, on a
/// grid of whole seconds from the first sample processed.
#[derive(Debug, Clone)]
pub struct SecondIntegrator {
    fs: f64,
    per_second: u64,
    a: WeightingFilter,
    c: WeightingFilter,
    correction: Option<PartitionedFir>,
    corrected: Vec<f64>,
    /// Samples of the current second passed (measured or skipped).
    pos: u64,
    measured: u64,
    acc: [f64; 3],
}

impl SecondIntegrator {
    /// At `fs` Hz (a whole number of samples per second); fails if the rate is too low
    /// for A/C weighting.
    pub fn new(fs: f64) -> Result<Self, WeightingError> {
        Ok(Self {
            fs,
            per_second: (fs.round() as u64).max(1),
            a: WeightingFilter::new(Weighting::A, fs)?,
            c: WeightingFilter::new(Weighting::C, fs)?,
            correction: None,
            corrected: Vec::new(),
            pos: 0,
            measured: 0,
            acc: [0.0; 3],
        })
    }

    /// Runs the weighted paths through `taps` (a mic-curve correction FIR, as
    /// [`crate::spl::SplMeter::set_correction`]); `None` removes it.
    pub fn set_correction(&mut self, taps: Option<&[f64]>) {
        let part = crate::mic_curve::fir_partition(self.fs);
        self.correction = taps.map(|h| PartitionedFir::new(h, part));
        let lat = self.correction.as_ref().map_or(0, PartitionedFir::latency);
        self.corrected = vec![0.0; lat];
    }

    /// Whether a mic-curve correction is in the path.
    pub fn has_correction(&self) -> bool {
        self.correction.is_some()
    }

    /// Samples into the current second.
    pub fn position(&self) -> u64 {
        self.pos
    }

    /// Samples per second.
    pub fn samples_per_second(&self) -> u64 {
        self.per_second
    }

    fn flush(&mut self, emit: &mut impl FnMut(Second)) {
        let fs = self.fs;
        emit(Second {
            energy: [self.acc[0] / fs, self.acc[1] / fs, self.acc[2] / fs],
            measured: self.measured as f64 / self.per_second as f64,
        });
        self.acc = [0.0; 3];
        self.measured = 0;
        self.pos = 0;
    }

    #[inline]
    fn step(&mut self, x: f64, emit: &mut impl FnMut(Second)) {
        let ya = self.a.process_sample(x);
        let yc = self.c.process_sample(x);
        self.acc[0] += ya * ya;
        self.acc[1] += yc * yc;
        self.acc[2] += x * x;
        self.measured += 1;
        self.pos += 1;
        if self.pos == self.per_second {
            self.flush(emit);
        }
    }

    /// Processes raw input samples (FS); `emit` receives every completed second. Does not
    /// allocate.
    pub fn process(&mut self, block: &[f64], mut emit: impl FnMut(Second)) {
        let Some(mut fir) = self.correction.take() else {
            for &x in block {
                self.step(x, &mut emit);
            }
            return;
        };
        let mut corrected = std::mem::take(&mut self.corrected);
        for chunk in block.chunks(corrected.len().max(1)) {
            let c = &mut corrected[..chunk.len()];
            fir.process(chunk, c);
            for &x in c.iter() {
                self.step(x, &mut emit);
            }
        }
        self.corrected = corrected;
        self.correction = Some(fir);
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
}

impl WindowValue {
    /// Some of the elapsed time was not measured (capture gaps, the meter stopped).
    pub fn incomplete(&self) -> bool {
        self.measured < f64::from(self.elapsed) - 1e-6
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
    spec: WindowSpec,
    w: usize,
    /// Slots that stay in the window over the horizon: `seconds − horizon`, or 0.
    keep: u32,
    full: Sum,
    kept: Sum,
    since_exact: u32,
}

/// The newest seconds and the rolling windows over them.
#[derive(Debug, Clone)]
pub struct RollingLeq {
    ring: Vec<Second>,
    /// Next slot written.
    head: usize,
    /// Slots pushed since the start (or the last [`Self::clear`]).
    pushed: u64,
    horizon: u32,
    windows: Vec<Acc>,
}

impl RollingLeq {
    /// Windows `specs` (lengths clamped to ≥ 1 s) with headroom over `horizon` seconds
    /// (≥ 1); the ring holds the longest window.
    pub fn new(specs: &[WindowSpec], horizon: u32) -> Self {
        let horizon = horizon.max(1);
        let cap = specs.iter().map(|s| s.seconds.max(1)).max().unwrap_or(1);
        Self {
            ring: vec![Second::GAP; cap as usize],
            head: 0,
            pushed: 0,
            horizon,
            windows: specs
                .iter()
                .map(|s| {
                    let seconds = s.seconds.max(1);
                    Acc {
                        spec: WindowSpec {
                            seconds,
                            weighting: s.weighting,
                        },
                        w: w_index(s.weighting),
                        keep: seconds.saturating_sub(horizon),
                        full: Sum::default(),
                        kept: Sum::default(),
                        since_exact: 0,
                    }
                })
                .collect(),
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

    /// The windows, in construction order.
    pub fn specs(&self) -> impl Iterator<Item = WindowSpec> + '_ {
        self.windows.iter().map(|a| a.spec)
    }

    /// Seconds pushed since the start.
    pub fn pushed(&self) -> u64 {
        self.pushed
    }

    /// Slot `k` seconds back from the newest (`k = 0` is the newest); `None` before the
    /// start.
    fn back(&self, k: u32) -> Option<&Second> {
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
                Some(x) => s.add(x.energy[w], x.measured),
                None => break,
            }
        }
        s
    }

    /// Adds the newest second.
    pub fn push(&mut self, s: Second) {
        // The slots leaving each window are read before the new one may overwrite them.
        for i in 0..self.windows.len() {
            let (n, keep, w) = {
                let a = &self.windows[i];
                (a.spec.seconds, a.keep, a.w)
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
            a.full.add(s.energy[w], s.measured);
            if let Some(o) = out_full {
                a.full.add(-o.energy[w], -o.measured);
            }
            if keep > 0 {
                a.kept.add(s.energy[w], s.measured);
                if let Some(o) = out_kept {
                    a.kept.add(-o.energy[w], -o.measured);
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
            if a.since_exact >= a.spec.seconds {
                let (w, n, keep) = (a.w, a.spec.seconds, a.keep);
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
        self.ring.iter_mut().for_each(|s| *s = Second::GAP);
        self.head = 0;
        self.pushed = 0;
        for a in &mut self.windows {
            a.full = Sum::default();
            a.kept = Sum::default();
            a.since_exact = 0;
        }
    }

    /// Value of window `i`.
    ///
    /// # Panics
    /// If `i` is not a window index.
    pub fn value(&self, i: usize) -> WindowValue {
        let a = &self.windows[i];
        let m = a.full.measured();
        WindowValue {
            leq_dbfs: if m > 0.0 {
                power_dbfs(a.full.energy() / m)
            } else {
                f64::NAN
            },
            elapsed: self.pushed.min(u64::from(a.spec.seconds)) as u32,
            measured: m,
        }
    }

    /// Headroom of window `i` against a limit given as a mean square (FS²): the highest
    /// steady mean square for the next horizon that leaves the window at the limit, or how
    /// long recovery takes at the limit when no level can.
    ///
    /// # Panics
    /// If `i` is not a window index.
    pub fn headroom(&self, i: usize, limit_ms: f64) -> Headroom {
        let a = &self.windows[i];
        if a.keep == 0 {
            // The whole window is replaced within the horizon.
            return Headroom::Allowed { ms: limit_ms };
        }
        let h = f64::from(self.horizon);
        let x = (limit_ms * (a.kept.measured() + h) - a.kept.energy()) / h;
        if x > 0.0 {
            Headroom::Allowed { ms: x }
        } else {
            Headroom::CannotRecover {
                recover_s: self
                    .recover_time(i, limit_ms, limit_ms)
                    .unwrap_or(a.spec.seconds),
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
        let n = a.spec.seconds;
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
                neumaier(&mut e, &mut ec, x.energy[a.w]);
                neumaier(&mut m, &mut mc, x.measured);
            }
            let t = f64::from(n - j);
            if (e + ec) + t * level_ms <= limit_ms * ((m + mc) + t) {
                best = Some(n - j);
            }
        }
        best
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

#[cfg(test)]
mod tests;
