//! Fractional-octave display smoothing on the base-2 log grid (PLAN.md §5.1).
//!
//! Smoothing is a display operation on columns whose H1 and coherence are already formed.
//! Averaging happens across columns, never across cross-spectra, so it cannot reintroduce
//! the delay sensitivity that summing `Sxy` inside a wide column would.
//!
//! # Kernel
//!
//! Hann in log2-frequency. A boxcar would let a narrow feature enter and leave the window
//! abruptly and draw ripples beside it; Hann tapers to zero at its edges.
//!
//! Tapering narrows the effective width, so the kernel is widened to keep the label honest.
//! For a continuous Hann of full width `W`, `(∫w)² / ∫w² = 2W/3` — the width of the
//! rectangular average with the same variance reduction (the window's ENBW). The full width
//! is therefore **`W = 1.5 / b` octaves** for a `1/b`-octave setting, and the smoothed curve
//! averages noise like a `1/b`-octave boxcar. On a grid whose half-width
//! `H = 0.75·ppo/b` columns is an integer or half-integer the sampled kernel's ENBW is
//! exactly `ppo/b` columns (tested).
//!
//! The kernel's peak response is a different number: a feature one column wide is
//! reduced by `Σw ≈ ppo·W/2` (`ppo/(1.33·b)` columns), not by the ENBW. At 1/3 octave on
//! 48 ppo that is a factor of 12 in power, and the smoothed feature takes the kernel's own
//! shape: full width at half the excess power `W/2` octaves.
//!
//! At 1/48 octave on a 48 ppo grid the half-width is 0.75 columns and the kernel is the
//! identity: the column aggregation already is that wide.
//!
//! # Where it averages
//!
//! - **Base-2 octaves** only, matching the grid ([`crate::grid`]): a base-2 kernel spans a
//!   fixed number of columns along the whole axis.
//! - **Truncated at the axis ends**, renormalised over the weights that exist. Reflecting
//!   would draw columns that were never measured.
//! - **Confined to runs of valid columns.** An invalid (masked or non-finite) column splits
//!   the curve: its neighbours are not neighbours in any sense the measurement supports.
//!   Invalid columns pass through unchanged and stay invalid.
//! - **Coherence is never smoothed.** It is the trust indicator; smoothing it would make a
//!   bad measurement look better. [`SmoothedTf::coherence`] is the input copied verbatim.
//!
//! # Modes
//!
//! - [`SmoothingMode::Magnitude`]: magnitude only. `|H|` becomes `sqrt(Σ w|H|² / Σ w)`; the
//!   phase of each column is left as measured.
//! - [`SmoothingMode::MagnitudePhase`]: magnitude as in `Magnitude`, and phase smoothed too.
//!   Phase is unwrapped along each valid run first — a mean taken across a ±π wrap lands
//!   near 0, a value the measurement never contained — then averaged with the same weights.
//!
//! A vector mean `Σ wH / Σ w` is deliberately not offered: wherever a residual delay rotates
//! the phase across the kernel, the vectors cancel and the magnitude drops, which would draw
//! a delay error as a response loss.
//!
//! On a pure delay `τ` the magnitude-and-phase mode returns magnitude 1 and phase `-2πτ·f̄`,
//! where `f̄` is the kernel-weighted mean of the column frequencies. Because `f` is convex in
//! `log f`, `f̄` sits slightly above the column centre (≈ 0.3 % at 1/3 octave), so the
//! smoothed phase of a delay is slightly steeper than the unsmoothed one. Unwrapping follows
//! the smaller step between adjacent columns, so a residual delay with `τ·Δf > ½` between
//! columns (at 48 ppo: `τ·f > 34`) cannot be followed; set the delay first.
//!
//! # Linear-bin power spectra
//!
//! [`LinearSmoother`] applies the same kernel to a narrowband power spectrum on its FFT bins
//! (`f_k = k·fs/n`), which are uniform in frequency, not in log frequency. The weight of bin
//! `j` in the average at bin `k` is the Hann of `log2(f_j/f_k)` (full width `1.5/b` octaves,
//! as above) times `1/f_j`: a bin spans `Δf/(f·ln 2)` octaves, so weighting by `1/f` makes
//! the sum an integral over log frequency — on a fine grid it converges to what [`Smoother`]
//! computes on the log grid (tested), and the `1/b`-octave label means the same thing.
//!
//! - Only bins inside the kernel count, so where the kernel is narrower than one bin (the
//!   lowest bins) a bin is its own average and passes through unchanged.
//! - DC has no log frequency: bin 0 is never averaged into another bin and passes through.
//! - Runs of valid bins and the spectrum's ends truncate the kernel as on the log grid.
//! - Power is averaged (a level in dB is converted to power by the caller); zero power is a
//!   valid value (a bin below the analyser's floor), only non-finite bins are gaps.
//!
//! The kernel spans up to `0.35·k` bins at bin `k` (1/3 octave), far too many to sum per bin
//! for a 64 k-point FFT at a display rate. The Hann separates:
//! `cos(θ_j − θ_k) = cos θ_j cos θ_k + sin θ_j sin θ_k` with `θ = π·log2 f / H`, so each output
//! needs three windowed sums of per-bin terms, read from a segment tree in `O(log n)`.
//! Prefix sums would be `O(1)` but subtract running totals that contain every louder bin
//! below: a tone 150 dB above the noise floor would leave nothing of the floor's digits.

use std::f64::consts::{PI, TAU};

use num_complex::Complex64;

use crate::grid::LogGrid;

/// Smoothing bandwidth, in base-2 octaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SmoothingFraction {
    /// 1/3 octave.
    Third,
    /// 1/6 octave.
    Sixth,
    /// 1/12 octave.
    Twelfth,
    /// 1/24 octave.
    TwentyFourth,
    /// 1/48 octave.
    FortyEighth,
}

impl SmoothingFraction {
    /// All fractions, widest first.
    pub const ALL: [SmoothingFraction; 5] = [
        SmoothingFraction::Third,
        SmoothingFraction::Sixth,
        SmoothingFraction::Twelfth,
        SmoothingFraction::TwentyFourth,
        SmoothingFraction::FortyEighth,
    ];

    /// `b` in `1/b` octave.
    pub fn denominator(self) -> u32 {
        match self {
            SmoothingFraction::Third => 3,
            SmoothingFraction::Sixth => 6,
            SmoothingFraction::Twelfth => 12,
            SmoothingFraction::TwentyFourth => 24,
            SmoothingFraction::FortyEighth => 48,
        }
    }

    /// Nominal (ENBW) bandwidth in octaves.
    pub fn octaves(self) -> f64 {
        1.0 / f64::from(self.denominator())
    }

    /// Full width of the Hann kernel in octaves: 1.5 × nominal, see the module docs.
    pub fn kernel_width_octaves(self) -> f64 {
        HANN_ENBW_WIDENING * self.octaves()
    }
}

/// Full width of a Hann window relative to its equivalent noise bandwidth.
pub const HANN_ENBW_WIDENING: f64 = 1.5;

/// What is smoothed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SmoothingMode {
    /// Power-average magnitude; phase untouched.
    Magnitude,
    /// Power-average magnitude and average unwrapped phase.
    MagnitudePhase,
}

/// Columns of one transfer function on a [`LogGrid`], as produced by the MTW stage.
#[derive(Debug, Clone, Copy)]
pub struct TfColumns<'a> {
    /// H1 per column.
    pub h: &'a [Complex64],
    /// γ² per column.
    pub coherence: &'a [f64],
    /// Column carries a usable estimate. Non-finite `h` is treated as invalid as well.
    pub valid: &'a [bool],
}

/// Smoothed columns.
#[derive(Debug, Clone, PartialEq)]
pub struct SmoothedTf {
    /// Smoothed H. Invalid columns hold their input value.
    pub h: Vec<Complex64>,
    /// The input coherence, unchanged.
    pub coherence: Vec<f64>,
    /// Validity used for smoothing (input validity and finiteness of `h`).
    pub valid: Vec<bool>,
    /// Bandwidth applied.
    pub fraction: SmoothingFraction,
    /// Mode applied.
    pub mode: SmoothingMode,
}

/// Precomputed kernel for one (grid, fraction).
///
/// The grid is uniform in log frequency, so one tap vector serves every column; truncation
/// at the axis ends and at invalid gaps is handled by renormalising over the taps that fall
/// inside the current valid run.
#[derive(Debug, Clone, PartialEq)]
pub struct Smoother {
    grid: LogGrid,
    fraction: SmoothingFraction,
    /// `taps[d]` is the weight at column offset `±d`; `taps[0] = 1`.
    taps: Vec<f64>,
}

impl Smoother {
    /// Kernel for `fraction` on `grid`.
    pub fn new(grid: LogGrid, fraction: SmoothingFraction) -> Self {
        // Half-width in columns. Taps at |d| >= half are zero (Hann reaches zero there).
        let half = 0.5 * fraction.kernel_width_octaves() * f64::from(grid.ppo);
        let mut taps = vec![1.0];
        let mut d = 1usize;
        while (d as f64) < half {
            taps.push(0.5 * (1.0 + (PI * d as f64 / half).cos()));
            d += 1;
        }
        Self {
            grid,
            fraction,
            taps,
        }
    }

    /// Grid the kernel was built for.
    pub fn grid(&self) -> &LogGrid {
        &self.grid
    }

    /// Bandwidth.
    pub fn fraction(&self) -> SmoothingFraction {
        self.fraction
    }

    /// One-sided taps: weight at column offset `±d` is `taps()[d]`.
    pub fn taps(&self) -> &[f64] {
        &self.taps
    }

    /// Equivalent noise bandwidth of the untruncated kernel, in columns.
    pub fn enbw_columns(&self) -> f64 {
        let (s, s2) = self
            .taps
            .iter()
            .enumerate()
            .fold((0.0, 0.0), |(s, s2), (d, w)| {
                let k = if d == 0 { 1.0 } else { 2.0 };
                (s + k * w, s2 + k * w * w)
            });
        s * s / s2
    }

    /// Smooth `tf`.
    ///
    /// # Panics
    /// If the slices are not all `grid().len()` long.
    pub fn smooth(&self, tf: TfColumns<'_>, mode: SmoothingMode) -> SmoothedTf {
        let n = self.grid.len();
        assert!(
            tf.h.len() == n && tf.coherence.len() == n && tf.valid.len() == n,
            "column slices must match the grid length {n}"
        );
        let valid: Vec<bool> = tf
            .valid
            .iter()
            .zip(tf.h)
            .map(|(v, h)| *v && h.re.is_finite() && h.im.is_finite())
            .collect();
        let mut h = tf.h.to_vec();
        for (start, end) in valid_runs(&valid) {
            self.smooth_run(&tf.h[start..end], &mut h[start..end], mode);
        }
        SmoothedTf {
            h,
            coherence: tf.coherence.to_vec(),
            valid,
            fraction: self.fraction,
            mode,
        }
    }

    /// Smooth one run of valid columns.
    fn smooth_run(&self, input: &[Complex64], out: &mut [Complex64], mode: SmoothingMode) {
        let power: Vec<f64> = input.iter().map(|z| z.norm_sqr()).collect();
        let smoothed_power = self.apply(&power);
        match mode {
            SmoothingMode::Magnitude => {
                for ((o, z), p) in out.iter_mut().zip(input).zip(&smoothed_power) {
                    *o = Complex64::from_polar(p.sqrt(), z.arg());
                }
            }
            SmoothingMode::MagnitudePhase => {
                let phase = unwrap(input.iter().map(|z| z.arg()));
                let smoothed_phase = self.apply(&phase);
                for ((o, p), ph) in out.iter_mut().zip(&smoothed_power).zip(&smoothed_phase) {
                    *o = Complex64::from_polar(p.sqrt(), *ph);
                }
            }
        }
    }

    /// Weighted moving average over one run, truncated and renormalised at its ends.
    fn apply(&self, x: &[f64]) -> Vec<f64> {
        let n = x.len();
        let reach = self.taps.len() - 1;
        (0..n)
            .map(|i| {
                let lo = i.saturating_sub(reach);
                let hi = (i + reach).min(n - 1);
                let (num, den) = (lo..=hi).fold((0.0, 0.0), |(num, den), j| {
                    let w = self.taps[i.abs_diff(j)];
                    (num + w * x[j], den + w)
                });
                num / den
            })
            .collect()
    }
}

/// Fractional-octave power smoothing of a narrowband spectrum on its linear FFT bins
/// (module docs, "Linear-bin power spectra").
///
/// Bin `k` is at `k·fs/n`; only ratios of bin frequencies enter the kernel, so the weights
/// depend on the bin count and the fraction, not on `fs`. Build once per (bin count,
/// fraction) and reuse; [`LinearSmoother::smooth_into`] also reuses its working memory.
#[derive(Debug, Clone)]
pub struct LinearSmoother {
    fraction: SmoothingFraction,
    /// Kernel half-width, octaves.
    half_oct: f64,
    /// Per bin: the half-open range of bins inside its kernel (`(0, 0)` for DC).
    window: Vec<(u32, u32)>,
    /// `cos θ_k`, `sin θ_k` with `θ_k = π·log2(k)/half_oct` (0 for DC).
    phase: Vec<(f64, f64)>,
    /// Working memory of [`LinearSmoother::smooth_into`].
    scratch: Scratch,
}

/// Working memory of one smoothing pass, sized to the bin count on first use.
#[derive(Debug, Clone, Default)]
struct Scratch {
    /// Bin is valid (and finite).
    ok: Vec<bool>,
    /// Half-open run of valid bins each valid bin belongs to.
    run: Vec<(u32, u32)>,
    /// Range-sum tree over the per-bin terms.
    tree: SumTree,
}

/// Per-bin terms summed over a kernel: `[u, u·cos θ, u·sin θ, u·P, u·P·cos θ, u·P·sin θ]`.
type Terms = [f64; 6];

impl LinearSmoother {
    /// Kernel for `fraction` on a spectrum of `bins` bins (`n/2 + 1` for an `n`-point FFT).
    pub fn new(bins: usize, fraction: SmoothingFraction) -> Self {
        let half_oct = 0.5 * fraction.kernel_width_octaves();
        let up = half_oct.exp2();
        let mut window = Vec::with_capacity(bins);
        let mut phase = Vec::with_capacity(bins);
        for k in 0..bins {
            if k == 0 {
                window.push((0, 0));
                phase.push((0.0, 0.0));
                continue;
            }
            let kf = k as f64;
            // Bins with |log2(j/k)| < H; the edge bins carry zero weight either way.
            let lo = ((kf / up).floor() as usize + 1).clamp(1, k);
            let hi = ((kf * up).ceil() as usize).clamp(k + 1, bins);
            window.push((lo as u32, hi as u32));
            let th = PI * kf.log2() / half_oct;
            phase.push((th.cos(), th.sin()));
        }
        Self {
            fraction,
            half_oct,
            window,
            phase,
            scratch: Scratch::default(),
        }
    }

    /// Number of bins.
    pub fn bins(&self) -> usize {
        self.window.len()
    }

    /// Bandwidth.
    pub fn fraction(&self) -> SmoothingFraction {
        self.fraction
    }

    /// Kernel weight of bin `j` in the average at bin `k` (both ≥ 1), before the `1/f_j`
    /// log-measure factor.
    pub fn hann(&self, k: usize, j: usize) -> f64 {
        let d = (j as f64 / k as f64).log2();
        if d.abs() >= self.half_oct {
            0.0
        } else {
            0.5 * (1.0 + (PI * d / self.half_oct).cos())
        }
    }

    /// Smooth `power` (linear power per bin, not dB). Bins where `valid` is false or the
    /// power is not finite are gaps: they pass through unchanged and no kernel crosses them.
    ///
    /// # Panics
    /// If the slices are not `bins()` long.
    pub fn smooth(&self, power: &[f64], valid: &[bool]) -> Vec<f64> {
        let n = self.bins();
        assert!(
            power.len() == n && valid.len() == n,
            "power and validity must be {n} bins long"
        );
        let mut out = vec![0.0; n];
        self.smooth_with(&mut Scratch::default(), power, |j| valid[j], &mut out);
        out
    }

    /// [`LinearSmoother::smooth`] with every finite bin valid, written to `out`. Its working
    /// memory stays in the smoother, so a live spectrum smoothed every frame allocates only
    /// on the first.
    ///
    /// # Panics
    /// If the slices are not `bins()` long.
    pub fn smooth_into(&mut self, power: &[f64], out: &mut [f64]) {
        let n = self.bins();
        assert!(
            power.len() == n && out.len() == n,
            "power and output must be {n} bins long"
        );
        let mut scratch = std::mem::take(&mut self.scratch);
        self.smooth_with(&mut scratch, power, |_| true, out);
        self.scratch = scratch;
    }

    fn smooth_with(
        &self,
        s: &mut Scratch,
        power: &[f64],
        valid: impl Fn(usize) -> bool,
        out: &mut [f64],
    ) {
        let n = self.bins();
        s.ok.clear();
        s.ok.extend(
            power
                .iter()
                .enumerate()
                .map(|(j, p)| valid(j) && p.is_finite()),
        );
        // Run bounds per valid bin.
        s.run.clear();
        s.run.resize(n, (0, 0));
        let mut a = 0;
        while a < n {
            if !s.ok[a] {
                a += 1;
                continue;
            }
            let mut b = a;
            while b < n && s.ok[b] {
                b += 1;
            }
            s.run[a..b].fill((a as u32, b as u32));
            a = b;
        }
        s.tree.build(n, |j| {
            if j == 0 || !s.ok[j] {
                return [0.0; 6];
            }
            let u = 1.0 / j as f64;
            let (c, si) = self.phase[j];
            let p = power[j];
            [u, u * c, u * si, u * p, u * p * c, u * p * si]
        });
        for (k, o) in out.iter_mut().enumerate() {
            if k == 0 || !s.ok[k] {
                *o = power[k];
                continue;
            }
            let (lo, hi) = self.window[k];
            let (a, b) = s.run[k];
            let t = s.tree.sum(lo.max(a) as usize, hi.min(b) as usize);
            let (c, si) = self.phase[k];
            // Σ u·w·(·) with w = ½(1 + cos θ_j cos θ_k + sin θ_j sin θ_k).
            let den = t[0] + c * t[1] + si * t[2];
            let num = t[3] + c * t[4] + si * t[5];
            // Rounding can leave a hair below zero where every weighted bin is silent.
            *o = (num / den).max(0.0);
        }
    }
}

/// `power` smoothed by `fraction` on its linear bins ([`LinearSmoother`]); every finite bin
/// valid. Builds the kernel each call: keep a [`LinearSmoother`] to smooth repeatedly.
pub fn smooth_linear_bins(power: &[f64], fraction: SmoothingFraction) -> Vec<f64> {
    LinearSmoother::new(power.len(), fraction).smooth(power, &vec![true; power.len()])
}

/// Range sums over per-bin terms. A query adds only nodes inside the range, so a sum never
/// contains — and never loses digits to — values outside it.
#[derive(Debug, Clone, Default)]
struct SumTree {
    n: usize,
    node: Vec<Terms>,
}

impl SumTree {
    /// Rebuilds the tree over `n` leaves `leaf(0) … leaf(n − 1)`, reusing its memory.
    fn build(&mut self, n: usize, leaf: impl Fn(usize) -> Terms) {
        self.n = n;
        self.node.clear();
        self.node.resize(2 * n, [0.0; 6]);
        for j in 0..n {
            self.node[n + j] = leaf(j);
        }
        for i in (1..n).rev() {
            let (a, b) = (self.node[2 * i], self.node[2 * i + 1]);
            self.node[i] = std::array::from_fn(|t| a[t] + b[t]);
        }
    }

    /// Sum over leaves `[lo, hi)`.
    fn sum(&self, lo: usize, hi: usize) -> Terms {
        let mut acc = [0.0; 6];
        let add = |acc: &mut Terms, x: &Terms| acc.iter_mut().zip(x).for_each(|(a, v)| *a += v);
        let (mut l, mut r) = (lo + self.n, hi + self.n);
        while l < r {
            if l & 1 == 1 {
                add(&mut acc, &self.node[l]);
                l += 1;
            }
            if r & 1 == 1 {
                r -= 1;
                add(&mut acc, &self.node[r]);
            }
            l >>= 1;
            r >>= 1;
        }
        acc
    }
}

/// Half-open index ranges of consecutive `true` values.
pub(crate) fn valid_runs(valid: &[bool]) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut i = 0;
    while i < valid.len() {
        if valid[i] {
            let start = i;
            while i < valid.len() && valid[i] {
                i += 1;
            }
            runs.push((start, i));
        } else {
            i += 1;
        }
    }
    runs
}

/// Unwrap a phase sequence (radians), choosing the smaller step between neighbours.
pub(crate) fn unwrap(phase: impl IntoIterator<Item = f64>) -> Vec<f64> {
    let mut out: Vec<f64> = Vec::new();
    let mut offset = 0.0;
    let mut prev: Option<f64> = None;
    for p in phase {
        if let Some(q) = prev {
            let step = p - q;
            offset -= TAU * (step / TAU).round();
        }
        prev = Some(p);
        out.push(p + offset);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_testkit::golden::GoldenSet;

    fn grid() -> LogGrid {
        LogGrid::covering(48, 20.0, 20_000.0)
    }

    fn all_valid(n: usize) -> Vec<bool> {
        vec![true; n]
    }

    #[test]
    fn enbw_matches_label_on_48_ppo() {
        for fr in SmoothingFraction::ALL {
            let s = Smoother::new(grid(), fr);
            let expected = 48.0 / f64::from(fr.denominator());
            assert!(
                (s.enbw_columns() - expected).abs() < 1e-12,
                "{fr:?}: enbw {} columns, expected {expected}",
                s.enbw_columns()
            );
        }
        assert_eq!(
            Smoother::new(grid(), SmoothingFraction::FortyEighth).taps(),
            &[1.0]
        );
    }

    #[test]
    fn flat_response_stays_flat() {
        let g = grid();
        let n = g.len();
        let h = vec![Complex64::from_polar(0.5, -1.0); n];
        let coh = vec![0.9; n];
        let valid = all_valid(n);
        for fr in SmoothingFraction::ALL {
            let s = Smoother::new(g, fr);
            for mode in [SmoothingMode::Magnitude, SmoothingMode::MagnitudePhase] {
                let out = s.smooth(
                    TfColumns {
                        h: &h,
                        coherence: &coh,
                        valid: &valid,
                    },
                    mode,
                );
                for (a, b) in out.h.iter().zip(&h) {
                    assert!((a - b).norm() < 1e-12, "{fr:?} {mode:?}: {a} vs {b}");
                }
            }
        }
    }

    /// A one-column power excess takes the kernel's shape: height reduced by Σw = 12 at
    /// 1/3 octave on 48 ppo (H = 12 columns), half-excess points at ±6 columns.
    #[test]
    fn narrow_peak_at_third_octave_has_kernel_shape() {
        let g = grid();
        let n = g.len();
        let c = n / 2;
        let excess: f64 = 99.0;
        let mut h = vec![Complex64::new(1.0, 0.0); n];
        h[c] = Complex64::new((1.0 + excess).sqrt(), 0.0);
        let coh = vec![1.0; n];
        let valid = all_valid(n);
        let s = Smoother::new(g, SmoothingFraction::Third);
        let out = s.smooth(
            TfColumns {
                h: &h,
                coherence: &coh,
                valid: &valid,
            },
            SmoothingMode::Magnitude,
        );
        let p: Vec<f64> = out.h.iter().map(|z| z.norm_sqr() - 1.0).collect();
        assert!((p[c] - excess / 12.0).abs() < 1e-9, "peak excess {}", p[c]);
        assert!((p[c + 6] - excess / 24.0).abs() < 1e-9);
        assert!((p[c - 6] - excess / 24.0).abs() < 1e-9);
        assert!(p[c + 12].abs() < 1e-12 && p[c - 12].abs() < 1e-12);
        // Full width at half excess = W/2 = 0.25 octave.
        let half_width_oct = (g.frequency(c + 6) / g.frequency(c - 6)).log2();
        assert!((half_width_oct - 0.25).abs() < 1e-12);
    }

    #[test]
    fn never_smooths_across_invalid_gap() {
        let g = grid();
        let n = g.len();
        let gap = 200..203;
        let mut h = vec![Complex64::new(1.0, 0.0); n];
        // Large values on one side of the gap and inside it must not leak to the other side.
        for z in &mut h[..gap.start] {
            *z = Complex64::new(10.0, 0.0);
        }
        h[201] = Complex64::new(f64::NAN, 0.0);
        let mut valid = all_valid(n);
        valid[200] = false;
        valid[202] = false;
        let coh = vec![1.0; n];
        let s = Smoother::new(g, SmoothingFraction::Third);
        let out = s.smooth(
            TfColumns {
                h: &h,
                coherence: &coh,
                valid: &valid,
            },
            SmoothingMode::MagnitudePhase,
        );
        assert!(!out.valid[201], "non-finite column must be invalid");
        for i in gap.clone() {
            assert_eq!(
                out.h[i].re.to_bits(),
                h[i].re.to_bits(),
                "gap passes through"
            );
        }
        for i in 0..gap.start {
            assert!((out.h[i].re - 10.0).abs() < 1e-12, "column {i}");
        }
        for i in gap.end..n {
            assert!((out.h[i].re - 1.0).abs() < 1e-12, "column {i}");
        }
    }

    #[test]
    fn coherence_is_passed_through() {
        let g = grid();
        let n = g.len();
        let h = vec![Complex64::new(1.0, 0.0); n];
        let coh: Vec<f64> = (0..n).map(|i| (i % 7) as f64 / 7.0).collect();
        let valid = all_valid(n);
        let out = Smoother::new(g, SmoothingFraction::Third).smooth(
            TfColumns {
                h: &h,
                coherence: &coh,
                valid: &valid,
            },
            SmoothingMode::MagnitudePhase,
        );
        assert_eq!(out.coherence, coh);
    }

    /// Pure delay: both modes keep |H| = 1. Power keeps the measured phase; complex returns
    /// `-2πτ·f̄` with `f̄` the kernel-weighted mean frequency — continuous across wraps.
    #[test]
    fn delayed_response_power_vs_complex() {
        let g = grid();
        let n = g.len();
        let tau = 0.5e-3;
        let f = g.frequencies();
        let h: Vec<Complex64> = f
            .iter()
            .map(|f| Complex64::from_polar(1.0, -TAU * f * tau))
            .collect();
        let coh = vec![1.0; n];
        let valid = all_valid(n);
        let s = Smoother::new(g, SmoothingFraction::Third);
        let tf = TfColumns {
            h: &h,
            coherence: &coh,
            valid: &valid,
        };
        let pow = s.smooth(tf, SmoothingMode::Magnitude);
        let cpx = s.smooth(tf, SmoothingMode::MagnitudePhase);
        let reach = s.taps().len() - 1;
        for i in 0..n {
            assert!((pow.h[i].norm() - 1.0).abs() < 1e-12);
            assert!((cpx.h[i].norm() - 1.0).abs() < 1e-12);
            assert!((pow.h[i] - h[i]).norm() < 1e-12, "power mode keeps phase");
            let lo = i.saturating_sub(reach);
            let hi = (i + reach).min(n - 1);
            let (num, den) = (lo..=hi).fold((0.0, 0.0), |(a, b), j| {
                let w = s.taps()[i.abs_diff(j)];
                (a + w * f[j], b + w)
            });
            let expected = Complex64::from_polar(1.0, -TAU * tau * num / den);
            assert!(
                (cpx.h[i] - expected).norm() < 1e-9,
                "column {i} at {} Hz: {} vs {}",
                f[i],
                cpx.h[i],
                expected
            );
        }
        // Away from the axis ends the weighted mean lies above the centre (convexity).
        let mid = n / 2;
        let lag = cpx.h[mid].arg() - h[mid].arg();
        let lag = lag - TAU * (lag / TAU).round();
        assert!(lag < 0.0, "smoothed phase leads the column phase: {lag}");
    }

    /// Direct evaluation of the linear-bin kernel: Σ hann·(1/j)·P over the run, normalised.
    fn linear_brute(s: &LinearSmoother, p: &[f64], valid: &[bool]) -> Vec<f64> {
        let ok: Vec<bool> = valid
            .iter()
            .zip(p)
            .map(|(v, x)| *v && x.is_finite())
            .collect();
        let mut out = p.to_vec();
        for (a, b) in valid_runs(&ok) {
            let a = a.max(1);
            for (k, o) in out.iter_mut().enumerate().take(b).skip(a) {
                let (num, den) = (a..b).fold((0.0, 0.0), |(num, den), j| {
                    let w = s.hann(k, j) / j as f64;
                    (num + w * p[j], den + w)
                });
                *o = num / den;
            }
        }
        out
    }

    #[test]
    fn linear_bins_match_direct_evaluation() {
        let n = 4097;
        // Deterministic pseudo-random levels over 160 dB, a few gaps and silent bins.
        let mut x = 0x2545_f491_4f6c_dd1du64;
        let mut p: Vec<f64> = (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                10f64.powf(-16.0 * (x % 10_000) as f64 / 10_000.0)
            })
            .collect();
        p[700] = 0.0;
        p[1500] = f64::NAN;
        let mut valid = vec![true; n];
        valid[2000..2010].fill(false);
        for fr in SmoothingFraction::ALL {
            let s = LinearSmoother::new(n, fr);
            let fast = s.smooth(&p, &valid);
            let slow = linear_brute(&s, &p, &valid);
            for k in 0..n {
                if p[k].is_nan() {
                    assert!(fast[k].is_nan());
                    continue;
                }
                assert!(
                    (fast[k] - slow[k]).abs() <= 1e-9 * slow[k].abs(),
                    "{fr:?} bin {k}: {} vs {}",
                    fast[k],
                    slow[k]
                );
            }
            for k in 2000..2010 {
                assert_eq!(fast[k].to_bits(), p[k].to_bits(), "gap passes through");
            }
        }
    }

    /// A full-scale tone low in the band must not wipe out a floor 150 dB down elsewhere.
    #[test]
    fn linear_bins_keep_a_quiet_floor_beside_a_loud_tone() {
        let n = 32_769;
        let mut p = vec![1e-15; n];
        p[50] = 1.0;
        let out = LinearSmoother::new(n, SmoothingFraction::Third).smooth(&p, &vec![true; n]);
        for (k, v) in out.iter().enumerate().skip(100) {
            assert!((v / 1e-15 - 1.0).abs() < 1e-6, "bin {k}: {v:e}");
        }
        assert!(out[50] < 0.5 && out[50] > 1e-3, "tone spread: {}", out[50]);
    }

    #[test]
    fn linear_bins_flat_stays_flat_and_low_bins_pass_through() {
        let n = 1025;
        let mut p = vec![0.25; n];
        p[0] = 7.0;
        p[1] = 3.0;
        p[2] = 0.5;
        for fr in SmoothingFraction::ALL {
            let s = LinearSmoother::new(n, fr);
            let out = s.smooth(&p, &vec![true; n]);
            assert_eq!(out[0], 7.0, "DC passes through");
            // At 1/3 octave the kernel at bin 1 spans 0.84 … 1.19: only bin 1 itself.
            assert_eq!(out[1], 3.0, "{fr:?}: bin 1 is narrower than the kernel");
            for (k, v) in out.iter().enumerate().skip(12) {
                assert!((v - 0.25).abs() < 1e-12, "{fr:?} bin {k}: {v}");
            }
        }
        assert_eq!(smooth_linear_bins(&p, SmoothingFraction::Sixth)[0], 7.0);
    }

    /// On a fine linear grid the log-measure weighting makes the linear-bin smoother agree
    /// with the log-grid smoother on the same curve: the `1/b` label means the same thing.
    #[test]
    fn linear_bins_agree_with_the_log_grid() {
        let fs = 48_000.0;
        let n_fft = 1usize << 20;
        let bins = n_fft / 2 + 1;
        let df = fs / n_fft as f64;
        // Power ripple of ±50 % with a 0.6-octave period: smoothing changes it visibly.
        let curve = |f: f64| 1.0 + 0.5 * (TAU * (f / 1000.0).log2() / 0.6).sin();
        let p: Vec<f64> = (0..bins).map(|k| curve(k as f64 * df)).collect();
        let g = LogGrid::covering(48, 20.0, 20_000.0);
        let f = g.frequencies();
        let h: Vec<Complex64> = f
            .iter()
            .map(|f| Complex64::new(curve(*f).sqrt(), 0.0))
            .collect();
        let coh = vec![1.0; h.len()];
        let valid = vec![true; h.len()];
        for fr in [SmoothingFraction::Third, SmoothingFraction::Twelfth] {
            let lin = LinearSmoother::new(bins, fr).smooth(&p, &vec![true; bins]);
            let log = Smoother::new(g, fr).smooth(
                TfColumns {
                    h: &h,
                    coherence: &coh,
                    valid: &valid,
                },
                SmoothingMode::Magnitude,
            );
            let mut worst: f64 = 0.0;
            let (mut lo, mut hi) = (f64::INFINITY, 0.0f64);
            for (i, fc) in f
                .iter()
                .enumerate()
                .filter(|(_, f)| **f > 200.0 && **f < 10_000.0)
            {
                // Linear interpolation between the bins either side of the column.
                let x = fc / df;
                let k = x.floor() as usize;
                let t = x - k as f64;
                let a = lin[k] * (1.0 - t) + lin[k + 1] * t;
                let b = log.h[i].norm_sqr();
                worst = worst.max((10.0 * (a / b).log10()).abs());
                (lo, hi) = (lo.min(a), hi.max(a));
            }
            assert!(worst < 0.005, "{fr:?}: {worst} dB apart");
            // The curve really was smoothed: the 3:1 power ripple shrank.
            assert!(hi / lo < 2.9, "{fr:?}: ripple {hi}/{lo}");
        }
    }

    #[test]
    fn matches_refgen_biquad_golden() {
        let gs = GoldenSet::load("display_smoothing_biquad").expect("golden set");
        let ppo = gs.scalar("ppo").expect("ppo") as u32;
        let g = LogGrid::covering(
            ppo,
            gs.scalar("f_lo_hz").expect("f_lo"),
            gs.scalar("f_hi_hz").expect("f_hi"),
        );
        let freq = gs.f64("freq_hz").expect("freq");
        assert_eq!(freq.len(), g.len());
        for (a, b) in g.frequencies().iter().zip(&freq) {
            assert!((a / b - 1.0).abs() < 1e-12);
        }
        let h = gs.c128("h").expect("h");
        let valid: Vec<bool> = gs
            .f64("valid")
            .expect("valid")
            .iter()
            .map(|v| *v > 0.5)
            .collect();
        let coh = vec![1.0; h.len()];
        let tf = TfColumns {
            h: &h,
            coherence: &coh,
            valid: &valid,
        };
        let third = Smoother::new(g, SmoothingFraction::Third).smooth(tf, SmoothingMode::Magnitude);
        let mag: Vec<f64> = third.h.iter().map(|z| z.norm()).collect();
        gs.assert_f64("power_third_mag", &mag);
        let sixth =
            Smoother::new(g, SmoothingFraction::Sixth).smooth(tf, SmoothingMode::MagnitudePhase);
        gs.assert_c128("complex_sixth", &sixth.h);
    }
}
