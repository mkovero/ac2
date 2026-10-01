//! Per-stage cross-spectral estimator: fixed block framing, windowed FFTs, spectral
//! averaging, and the effective-averages model.
//!
//! Only the auto- and cross-spectra are averaged. H1 and γ² are formed from the averaged
//! spectra afterwards; averaging H, γ² or dB values instead is biased.

use std::collections::VecDeque;
use std::sync::Arc;

use num_complex::Complex64;
use realfft::{RealFftPlanner, RealToComplex};

use crate::window::Window;

/// How block spectra are averaged.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Averaging {
    /// Plain mean of the most recent blocks. `blocks` is the count held by the full-rate
    /// stage (50 % overlap); deeper, more overlapped stages hold as many blocks as they need
    /// to reach the same model effective-average count.
    Fifo {
        /// Blocks held by the full-rate stage (≥ 1).
        blocks: u32,
    },
    /// Exponentially weighted mean. `time_constant_s` sets the full-rate stage's weight
    /// `α = 1 − exp(−hop/τ)`; deeper stages use the α that gives the same model
    /// effective-average count in steady state (so they are slower in wall-clock time).
    Exponential {
        /// Time constant of the full-rate stage, seconds (> 0).
        time_constant_s: f64,
    },
}

/// Concrete per-stage averaging after depth matching.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StageAveraging {
    /// Mean of the last `blocks` blocks.
    Fifo {
        /// Blocks held.
        blocks: usize,
    },
    /// Exponential weighting with per-block weight `alpha` of the newest block.
    Exponential {
        /// Weight of the newest block, 0 < α ≤ 1.
        alpha: f64,
    },
}

/// Stationary white-noise model of the correlation between Hann-windowed DFT coefficients.
///
/// For white input, the DFT of block `i` at bin `k` and of block `i + m` at bin `k + d` have
/// squared correlation
/// `ρ(m, d) = |Σₙ w[n] w[n + mH] e^{−j2πdn/N}|² / (Σₙ w[n]²)²`.
/// An average with block weights `aᵢ` over `K` adjacent bins then has the variance of
/// `N_eff = (Σa)² K² / Σ_{i,j} aᵢ aⱼ Σ_{k,l} ρ(i − j, k − l)` independent averages, and the
/// coherence-bias floor on uncorrelated signals is ≈ 1/N_eff. The model ignores signal
/// colour, non-stationarity and the DC/Nyquist bins; it is reported as a model value.
#[derive(Debug, Clone, PartialEq)]
pub struct OverlapModel {
    /// `rho[m][d]` for block lag `m` (0 ≤ m < ceil(N/H)) and bin lag `d` (0 ≤ d ≤ D_MAX).
    rho: Vec<Vec<f64>>,
}

/// Bin lags beyond this contribute < 1e-6 for Hann products and are dropped.
const D_MAX: usize = 16;

impl OverlapModel {
    /// Model for window `w` and hop `hop`.
    pub fn new(w: &[f64], hop: usize) -> Self {
        let n = w.len();
        let e2: f64 = w.iter().map(|v| v * v).sum();
        let lags = n.div_ceil(hop);
        let rho = (0..lags)
            .map(|m| {
                let s = m * hop;
                (0..=D_MAX)
                    .map(|d| {
                        let mut acc = Complex64::new(0.0, 0.0);
                        for j in 0..n - s {
                            let ph = -std::f64::consts::TAU * (d * j) as f64 / n as f64;
                            acc += Complex64::from_polar(w[j] * w[j + s], ph);
                        }
                        acc.norm_sqr() / (e2 * e2)
                    })
                    .collect()
            })
            .collect();
        Self { rho }
    }

    /// `ρ(m, d)`.
    pub fn rho(&self, m: usize, d: usize) -> f64 {
        self.rho
            .get(m)
            .and_then(|r| r.get(d))
            .copied()
            .unwrap_or(0.0)
    }

    /// Σ_{d=−(K−1)}^{K−1} (K − |d|) ρ(m, |d|).
    fn bin_sum(&self, m: usize, k: usize) -> f64 {
        let mut s = k as f64 * self.rho(m, 0);
        for d in 1..k.min(D_MAX + 1) {
            s += 2.0 * (k - d) as f64 * self.rho(m, d);
        }
        s
    }

    /// Effective averages of a mean over `held` blocks with uniform weights, `bins` bins.
    pub fn neff_fifo(&self, held: usize, bins: usize) -> f64 {
        if held == 0 || bins == 0 {
            return 0.0;
        }
        let mut den = 0.0;
        for m in 0..held.min(self.rho.len()) {
            let c = (held - m) as f64;
            den += if m == 0 { c } else { 2.0 * c } * self.bin_sum(m, bins);
        }
        let num = (held * bins) as f64;
        num * num / den
    }

    /// Effective averages of an exponential average with newest-block weight `alpha` after
    /// `received` blocks (`None` = steady state), `bins` bins.
    pub fn neff_exponential(&self, alpha: f64, received: Option<u64>, bins: usize) -> f64 {
        if bins == 0 || received == Some(0) {
            return 0.0;
        }
        let r = 1.0 - alpha;
        let k = received.map_or(f64::INFINITY, |k| k as f64);
        // r^p for p ≥ 0, with r^∞ = 0 (steady state) and 0^0 = 1.
        let pow = |p: f64| if p.is_infinite() { 0.0 } else { r.powf(p) };
        // Weights a_i = r^i for i < k: Σa and c_m = Σ_i a_i a_{i+m}, both in closed form
        // (finite sums for r = 0, which holds only the newest block).
        let sum_a = if r == 0.0 {
            1.0
        } else {
            (1.0 - pow(k)) / (1.0 - r)
        };
        let c = |m: usize| {
            let mf = m as f64;
            if mf >= k {
                0.0
            } else if r == 0.0 {
                if m == 0 { 1.0 } else { 0.0 }
            } else {
                pow(mf) * (1.0 - pow(2.0 * (k - mf))) / (1.0 - r * r)
            }
        };
        let mut den = 0.0;
        for m in 0..self.rho.len() {
            let cm = c(m);
            den += if m == 0 { cm } else { 2.0 * cm } * self.bin_sum(m, bins);
        }
        let num = sum_a * bins as f64;
        num * num / den
    }
}

/// Smallest FIFO depth whose model effective count reaches `target`.
pub fn matched_fifo_blocks(model: &OverlapModel, target: f64) -> usize {
    let mut n = 1;
    while model.neff_fifo(n, 1) < target * (1.0 - 1e-9) && n < 1 << 20 {
        n += 1;
    }
    n
}

/// α whose steady-state model effective count equals `target`.
pub fn matched_alpha(model: &OverlapModel, target: f64) -> f64 {
    if model.neff_exponential(1.0, None, 1) >= target {
        return 1.0;
    }
    // N_eff decreases monotonically with α.
    let (mut lo, mut hi) = (1e-12, 1.0);
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if model.neff_exponential(mid, None, 1) > target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Auto- and cross-spectra over the one-sided bins.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Spectra {
    pub xx: Vec<f64>,
    pub yy: Vec<f64>,
    pub xy: Vec<Complex64>,
}

impl Spectra {
    fn zeros(bins: usize) -> Self {
        Self {
            xx: vec![0.0; bins],
            yy: vec![0.0; bins],
            xy: vec![Complex64::new(0.0, 0.0); bins],
        }
    }

    fn fill_zero(&mut self) {
        self.xx.fill(0.0);
        self.yy.fill(0.0);
        self.xy.fill(Complex64::new(0.0, 0.0));
    }

    fn add_scaled(&mut self, o: &Spectra, s: f64) {
        for (a, b) in self.xx.iter_mut().zip(&o.xx) {
            *a += s * b;
        }
        for (a, b) in self.yy.iter_mut().zip(&o.yy) {
            *a += s * b;
        }
        for (a, b) in self.xy.iter_mut().zip(&o.xy) {
            *a += s * b;
        }
    }

    fn scale(&mut self, s: f64) {
        self.xx.iter_mut().for_each(|v| *v *= s);
        self.yy.iter_mut().for_each(|v| *v *= s);
        self.xy.iter_mut().for_each(|v| *v *= s);
    }
}

#[derive(Debug, Clone)]
enum AvgState {
    Fifo {
        cap: usize,
        ring: Vec<Spectra>,
        next: usize,
        sum: Spectra,
        /// Blocks since the running sum was last rebuilt exactly.
        since_rebuild: usize,
    },
    Exponential {
        alpha: f64,
        /// Σ (1 − α)^i · G_{newest − i}; unnormalised.
        sum: Spectra,
        /// Σ (1 − α)^i over the blocks received.
        weight: f64,
        received: u64,
    },
}

/// Framing, FFT and averaging for one stage.
pub(crate) struct StageEstimator {
    nfft: usize,
    hop: usize,
    window: Vec<f64>,
    window_sum_sq: f64,
    fft: Arc<dyn RealToComplex<f64>>,
    fft_in: Vec<f64>,
    fft_out: Vec<Complex64>,
    fft_out_y: Vec<Complex64>,
    fft_scratch: Vec<Complex64>,
    block: Spectra,
    /// Stage-rate samples awaiting the next block; `buf_x[0]` is decimated sample `buf_start`.
    buf_x: Vec<f64>,
    buf_y: Vec<f64>,
    buf_start: u64,
    /// Stage-sample intervals `[lo, hi)` that must not enter any block.
    rejected: VecDeque<(u64, u64)>,
    avg: AvgState,
    pub model: OverlapModel,
    /// Start (stage samples) of the newest block that was accumulated.
    pub last_block_start: Option<u64>,
}

impl std::fmt::Debug for StageEstimator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StageEstimator")
            .field("nfft", &self.nfft)
            .field("hop", &self.hop)
            .field("buf_start", &self.buf_start)
            .field("held", &self.held())
            .finish_non_exhaustive()
    }
}

/// What happened to a completed block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockFate {
    Accumulated,
    Rejected,
    Frozen,
}

impl StageEstimator {
    pub fn new(nfft: usize, hop: usize, averaging: StageAveraging, model: OverlapModel) -> Self {
        let window = Window::Hann.coefficients(nfft);
        let window_sum_sq = window.iter().map(|v| v * v).sum();
        let fft = RealFftPlanner::<f64>::new().plan_fft_forward(nfft);
        let bins = nfft / 2 + 1;
        Self {
            nfft,
            hop,
            fft_in: fft.make_input_vec(),
            fft_out: fft.make_output_vec(),
            fft_out_y: fft.make_output_vec(),
            fft_scratch: fft.make_scratch_vec(),
            fft,
            window,
            window_sum_sq,
            block: Spectra::zeros(bins),
            buf_x: Vec::with_capacity(nfft),
            buf_y: Vec::with_capacity(nfft),
            buf_start: 0,
            rejected: VecDeque::new(),
            avg: Self::fresh_avg(averaging, bins),
            model,
            last_block_start: None,
        }
    }

    fn fresh_avg(averaging: StageAveraging, bins: usize) -> AvgState {
        match averaging {
            StageAveraging::Fifo { blocks } => AvgState::Fifo {
                cap: blocks.max(1),
                ring: Vec::new(),
                next: 0,
                sum: Spectra::zeros(bins),
                since_rebuild: 0,
            },
            StageAveraging::Exponential { alpha } => AvgState::Exponential {
                alpha,
                sum: Spectra::zeros(bins),
                weight: 0.0,
                received: 0,
            },
        }
    }

    pub fn bins(&self) -> usize {
        self.nfft / 2 + 1
    }

    /// Drop the averages (framing continues).
    pub fn reset_averages(&mut self, averaging: StageAveraging) {
        self.avg = Self::fresh_avg(averaging, self.bins());
        self.last_block_start = None;
    }

    /// Drop everything: averages, framing and rejection marks.
    pub fn reset_stream(&mut self, averaging: StageAveraging) {
        self.reset_averages(averaging);
        self.buf_x.clear();
        self.buf_y.clear();
        self.buf_start = 0;
        self.rejected.clear();
    }

    /// Mark stage samples `[lo, hi)` as not to be analysed.
    pub fn reject(&mut self, lo: u64, hi: u64) {
        if hi <= lo {
            return;
        }
        if let Some(last) = self.rejected.back_mut()
            && lo <= last.1
        {
            last.0 = last.0.min(lo);
            last.1 = last.1.max(hi);
            return;
        }
        self.rejected.push_back((lo, hi));
    }

    /// Feed stage-rate samples; `on_block` is told the fate and start of every completed
    /// block.
    pub fn feed(
        &mut self,
        x: &[f64],
        y: &[f64],
        frozen: bool,
        mut on_block: impl FnMut(BlockFate, u64),
    ) {
        let mut i = 0;
        while i < x.len() {
            let take = (self.nfft - self.buf_x.len()).min(x.len() - i);
            self.buf_x.extend_from_slice(&x[i..i + take]);
            self.buf_y.extend_from_slice(&y[i..i + take]);
            i += take;
            if self.buf_x.len() == self.nfft {
                let start = self.buf_start;
                let end = start + self.nfft as u64;
                let rejected = self.rejected.iter().any(|&(lo, hi)| lo < end && hi > start);
                let fate = if rejected {
                    BlockFate::Rejected
                } else if frozen {
                    BlockFate::Frozen
                } else {
                    self.accumulate_block();
                    self.last_block_start = Some(start);
                    BlockFate::Accumulated
                };
                on_block(fate, start);
                self.buf_x.drain(..self.hop);
                self.buf_y.drain(..self.hop);
                self.buf_start += self.hop as u64;
                let next = self.buf_start;
                while self.rejected.front().is_some_and(|&(_, hi)| hi <= next) {
                    self.rejected.pop_front();
                }
            }
        }
    }

    fn transform(&mut self, y_leg: bool) {
        let src = if y_leg { &self.buf_y } else { &self.buf_x };
        for ((d, s), w) in self.fft_in.iter_mut().zip(src).zip(&self.window) {
            *d = s * w;
        }
        let out = if y_leg {
            &mut self.fft_out_y
        } else {
            &mut self.fft_out
        };
        // Lengths come from the plan itself, so the transform cannot fail.
        self.fft
            .process_with_scratch(&mut self.fft_in, out, &mut self.fft_scratch)
            .expect("fft buffers sized by plan");
    }

    fn accumulate_block(&mut self) {
        self.transform(false);
        self.transform(true);
        for (k, (x, y)) in self.fft_out.iter().zip(&self.fft_out_y).enumerate() {
            self.block.xx[k] = x.norm_sqr();
            self.block.yy[k] = y.norm_sqr();
            // Convention conj(X)·Y, so Gxy/Gxx = H for y = h * x.
            self.block.xy[k] = x.conj() * y;
        }
        match &mut self.avg {
            AvgState::Fifo {
                cap,
                ring,
                next,
                sum,
                since_rebuild,
            } => {
                if ring.len() < *cap {
                    sum.add_scaled(&self.block, 1.0);
                    ring.push(self.block.clone());
                    *next = ring.len() % *cap;
                } else {
                    sum.add_scaled(&ring[*next], -1.0);
                    sum.add_scaled(&self.block, 1.0);
                    ring[*next].clone_from(&self.block);
                    *next = (*next + 1) % *cap;
                    *since_rebuild += 1;
                    // Subtract-and-add drifts by rounding; rebuild exactly once per cycle.
                    if *since_rebuild >= *cap {
                        *since_rebuild = 0;
                        sum.fill_zero();
                        for s in ring.iter() {
                            sum.add_scaled(s, 1.0);
                        }
                    }
                }
            }
            AvgState::Exponential {
                alpha,
                sum,
                weight,
                received,
            } => {
                sum.scale(1.0 - *alpha);
                sum.add_scaled(&self.block, 1.0);
                *weight = *weight * (1.0 - *alpha) + 1.0;
                *received += 1;
            }
        }
    }

    /// Unnormalised averaged spectra (scale-free ratios only) and the total block weight.
    pub fn sums(&self) -> (&Spectra, f64) {
        match &self.avg {
            AvgState::Fifo { ring, sum, .. } => (sum, ring.len() as f64),
            AvgState::Exponential { sum, weight, .. } => (sum, *weight),
        }
    }

    /// Blocks contributing (FIFO: held; exponential: received).
    pub fn held(&self) -> u64 {
        match &self.avg {
            AvgState::Fifo { ring, .. } => ring.len() as u64,
            AvgState::Exponential { received, .. } => *received,
        }
    }

    /// Model effective averages for a column of `bins` adjacent bins.
    pub fn neff(&self, bins: usize) -> f64 {
        match &self.avg {
            AvgState::Fifo { ring, .. } => self.model.neff_fifo(ring.len(), bins),
            AvgState::Exponential {
                alpha, received, ..
            } => self.model.neff_exponential(*alpha, Some(*received), bins),
        }
    }

    /// Σw² of the analysis window (PSD scaling).
    pub fn window_sum_sq(&self) -> f64 {
        self.window_sum_sq
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hann_model(n: usize, hop: usize) -> OverlapModel {
        OverlapModel::new(&Window::Hann.coefficients(n), hop)
    }

    #[test]
    fn hann_correlations_closed_form() {
        let m = hann_model(4096, 2048);
        // Σ w² = 3N/8; 50 % lag: Σ_{n<N/2} w[n] w[n+N/2] = Σ sin²·cos² = N/16 → r = 1/6
        // (Harris's 16.7 % overlap correlation), ρ = r² = 1/36.
        assert!((m.rho(0, 0) - 1.0).abs() < 1e-12);
        assert!((m.rho(1, 0) - 1.0 / 36.0).abs() < 1e-9);
        // Adjacent bins of one block: |W²(1)| / Σw² = 2/3; two apart: 1/6.
        assert!((m.rho(0, 1) - 4.0 / 9.0).abs() < 1e-9);
        assert!((m.rho(0, 2) - 1.0 / 36.0).abs() < 1e-9);
        assert!(m.rho(0, 3) < 1e-20);
    }

    #[test]
    fn independent_blocks_count_fully() {
        let m = hann_model(256, 256);
        assert!((m.neff_fifo(10, 1) - 10.0).abs() < 1e-9);
        // α = 1 keeps only the newest block.
        assert!((m.neff_exponential(1.0, None, 1) - 1.0).abs() < 1e-9);
        // Non-overlapped EMA: (2 − α)/α.
        let a = 0.2;
        assert!((m.neff_exponential(a, None, 1) - (2.0 - a) / a).abs() < 1e-6);
        // Finite-k EMA approaches steady state.
        let k = m.neff_exponential(a, Some(1000), 1);
        assert!((k - (2.0 - a) / a).abs() < 1e-6);
        assert!((m.neff_exponential(a, Some(1), 1) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn matched_depths() {
        let m0 = hann_model(4096, 2048);
        let m2 = hann_model(4096, 512);
        let target = m0.neff_fifo(8, 1);
        let n2 = matched_fifo_blocks(&m2, target);
        assert!(m2.neff_fifo(n2, 1) >= target * (1.0 - 1e-9));
        assert!(m2.neff_fifo(n2 - 1, 1) < target);
        let a = matched_alpha(&m2, target);
        assert!((m2.neff_exponential(a, None, 1) / target - 1.0).abs() < 1e-9);
    }
}
