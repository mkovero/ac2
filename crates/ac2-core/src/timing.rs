//! Generator-to-loopback timing estimator (design Q3).
//!
//! The mapping watched here is `capture_index = output_index + offset`. Three layers, all
//! pure (no threads, no I/O):
//!
//! - [`GccPhat`]: GCC-PHAT between a capture window and the matching slice of generator
//!   history, with parabolic sub-sample peak and peak-to-sidelobe ratio.
//! - [`GccPhat::measure`]: applies the stimulus, loopback-level and confidence floors and
//!   turns one window into a [`WindowMeasurement`].
//! - [`TimingTracker`]: the NoStimulus / Acquiring / Locked / Jumped / Lost state machine and
//!   the drift line ([`drift`]), fed one measurement per hop.
//!
//! [`LoopbackTiming`] bundles the three for the daemon job.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

pub mod drift;
mod tracker;

pub use drift::{DriftEstimate, DriftLine};
pub use tracker::{
    LastLock, LoopbackTiming, TimingEvent, TimingEvents, TimingState, TimingTracker,
};

/// Window length at 48 kHz; other rates use the nearest power of two to the same duration.
pub const WINDOW_AT_48K: usize = 1 << 15;

/// Hop between windows, s.
pub const HOP_SECONDS: f64 = 0.25;

/// Analysis window length for a sample rate: W = 2^15 at 48 kHz, scaled with the rate to the
/// nearest power of two.
pub fn window_for_rate(sample_rate: f64) -> usize {
    let exact = WINDOW_AT_48K as f64 * sample_rate / 48_000.0;
    1usize << exact.log2().round().max(10.0) as u32
}

/// Inclusive range of offsets (samples) a window searches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LagRange {
    /// Smallest offset searched.
    pub min: i64,
    /// Largest offset searched.
    pub max: i64,
}

impl LagRange {
    /// Number of offsets beyond the first: `max − min`.
    pub fn span(&self) -> usize {
        (self.max - self.min).max(0) as usize
    }

    /// Length of the generator-history slice a window of `window` samples needs.
    pub fn reference_len(&self, window: usize) -> usize {
        window + self.span()
    }

    /// Output index of the first history sample for a window starting at capture index
    /// `capture_start`: the slice covers output indices
    /// `[capture_start − max, capture_start + window − min)`. May be negative before the
    /// stream has run long enough; such samples are zero.
    pub fn reference_start(&self, capture_start: u64) -> i64 {
        capture_start as i64 - self.max
    }

    /// Output index of the first of the newest `window` samples of that slice, the ones
    /// stimulus presence is judged on: `capture_start − min`.
    pub fn newest_start(&self, capture_start: u64) -> i64 {
        capture_start as i64 - self.min
    }
}

/// Parameters of the monitor. [`TimingConfig::for_rate`] gives the Q3 defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimingConfig {
    /// Sample rate, Hz.
    pub sample_rate: f64,
    /// Window length W, samples.
    pub window: usize,
    /// Hop between windows, samples.
    pub hop: usize,
    /// Offsets searched while acquiring (default 0 … 1 s).
    pub acquisition: LagRange,
    /// Half-width of the tracking search around the locked offset.
    pub track_radius: i64,
    /// Minimum peak-to-sidelobe ratio (peak over RMS of all other lags), dB.
    pub psr_floor_db: f64,
    /// Widest main lobe (at half the peak) a window may have to give an offset, s. The
    /// lobe is about the inverse of the stimulus bandwidth. A narrowband window (a sweep's
    /// first seconds, even a few hundred hertz wide) still has a clear peak, but where its
    /// whitened band ends the leakage of the window's edges pulls the peak off the true lag
    /// by a fraction of the lobe: tens of samples at a 500-lag lobe, a sample at 30, none
    /// below 15 (96 kHz, 0.68 s windows). Against `jump_samples` that reads as a lock
    /// elsewhere, a jump back, and a drift line tilted by the biased points.
    pub max_lobe_s: f64,
    /// Generator-history level below which there is no stimulus, dBFS RMS.
    pub stimulus_floor_dbfs: f64,
    /// Loopback level below which a window is not measured, dBFS RMS.
    pub loopback_floor_dbfs: f64,
    /// Windows of one acquisition or jump candidate agree when each lies within this many
    /// samples of the candidate's line.
    pub agree_samples: f64,
    /// A locked window further than this from the drift line's prediction (or three times
    /// the prediction's uncertainty, if larger) is a jump candidate, samples.
    pub jump_samples: f64,
    /// Largest slope a candidate line may have, ppm. The stimulus slides by `W·ε` within
    /// one window, which smears the correlation peak over that many lags (6.5 samples at
    /// 200 ppm with 0.68 s windows), so single windows scatter along the smear. Drift is
    /// followed reliably up to about 200 ppm; around 300 ppm the lock comes and goes, and
    /// far beyond the bound the monitor reports Lost rather than a line through scatter.
    pub max_drift_ppm: f64,
    /// Agreeing windows needed to lock; the first and last must not overlap.
    pub acquire_windows: usize,
    /// Consecutive unmeasurable windows after which the state is Lost.
    pub lost_after_windows: usize,
    /// Drift regression length, s.
    pub drift_window_s: f64,
    /// Shortest regression span on which drift is judged, s.
    pub drift_min_span_s: f64,
    /// Drift above which input and output are considered on different clocks, ppm.
    pub drift_threshold_ppm: f64,
    /// Standard uncertainty of the drift line's slope as the clocks' drift, ppm; it widens
    /// the prediction past the newest window (across a gap in the stimulus). A stimulus
    /// whose spectrum moves (a sweep) is timed at the group delay of the frequency it is
    /// at, plus the estimator's band-dependent bias, so on one clock its offset still moves
    /// within the sweep: half a sample in 2.25 s on an RME FF400 loopback (2.3 ppm). Such a
    /// slope, carried 20 s to the next sweep, misses by 4 samples. The regression across
    /// sweeps is not fooled (the same sweep repeats its trend), so the judged drift is.
    pub slope_sigma_ppm: f64,
}

impl TimingConfig {
    /// Q3 defaults for a sample rate.
    pub fn for_rate(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            window: window_for_rate(sample_rate),
            hop: (HOP_SECONDS * sample_rate).round() as usize,
            acquisition: LagRange {
                min: 0,
                max: sample_rate.round() as i64,
            },
            track_radius: 64,
            psr_floor_db: 20.0,
            max_lobe_s: 0.2e-3,
            stimulus_floor_dbfs: -100.0,
            loopback_floor_dbfs: -80.0,
            agree_samples: 1.0,
            jump_samples: 0.5,
            max_drift_ppm: 500.0,
            acquire_windows: 3,
            lost_after_windows: 4,
            drift_window_s: 30.0,
            drift_min_span_s: 10.0,
            drift_threshold_ppm: 2.0,
            slope_sigma_ppm: 1.0,
        }
    }
}

/// Why a window could not be processed (a caller bug, not a measurement outcome).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimingError {
    /// Capture length differs from the configured window.
    CaptureLength,
    /// History length differs from `range.reference_len(window)`.
    ReferenceLength,
    /// The search range is wider than the estimator was built for.
    RangeTooWide,
}

impl std::fmt::Display for TimingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CaptureLength => write!(f, "capture slice length differs from the window"),
            Self::ReferenceLength => write!(f, "history slice length does not match the range"),
            Self::RangeTooWide => write!(f, "search range wider than the estimator supports"),
        }
    }
}

impl std::error::Error for TimingError {}

/// Correlation peak of one window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Peak {
    /// Integer offset (capture − output), samples.
    pub offset: i64,
    /// Parabolic sub-sample correction in (−0.5, 0.5); diagnostics only.
    pub fraction: f64,
    /// Peak over RMS of all other lags (excluding the main lobe), dB.
    pub psr_db: f64,
    /// Width of the main lobe where it stays at or above half the peak, lags.
    pub lobe: usize,
}

/// Why a window with stimulus present produced no offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NoEstimate {
    /// Loopback level below the floor.
    LowLoopbackLevel,
    /// Correlation peak not clear enough.
    LowConfidence,
    /// Peak clear but its lobe too wide to time to a sample ([`TimingConfig::max_lobe_s`]).
    Narrowband,
}

/// Outcome of one window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    /// Generator history below the stimulus floor.
    NoStimulus,
    /// Stimulus present but no trustworthy offset.
    NoEstimate(NoEstimate),
    /// A confident offset.
    Offset(Peak),
}

/// One window's measurement, the tracker's only input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowMeasurement {
    /// Capture index of the window's first sample.
    pub capture_start: u64,
    /// What the window showed.
    pub outcome: Outcome,
    /// Loopback (capture) level, dBFS RMS.
    pub loopback_dbfs: f64,
    /// Level of the newest W generator-history samples, dBFS RMS.
    pub stimulus_dbfs: f64,
}

struct Plan {
    n: usize,
    fwd: Arc<dyn RealToComplex<f64>>,
    inv: Arc<dyn ComplexToReal<f64>>,
    a: Vec<f64>,
    b: Vec<f64>,
    sa: Vec<Complex<f64>>,
    sb: Vec<Complex<f64>>,
    scratch: Vec<Complex<f64>>,
}

impl std::fmt::Debug for Plan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plan").field("n", &self.n).finish()
    }
}

/// GCC-PHAT estimator with all FFT plans and buffers allocated up front.
#[derive(Debug)]
pub struct GccPhat {
    window: usize,
    max_span: usize,
    plans: Vec<Plan>,
    /// Capture taper, see [`capture_taper`].
    taper: Vec<f64>,
}

/// Fraction of the capture window tapered at each end.
const TAPER_FRACTION: f64 = 0.125;

/// Raised-cosine (Tukey) taper for the capture window. PHAT whitening gives every bin unit
/// weight, so when the stimulus is narrowband (a sweep's first second is a 20–35 Hz tone)
/// the bins outside its band carry only the leakage of the window's truncation steps. Those
/// steps then correlate with the reference slice's own ends and the peak lands exactly on
/// an end of the searched range (offset `min` or `max`) with a high peak-to-sidelobe ratio.
/// Fading the capture in and out removes its steps, so out-of-band bins are noise and stay
/// incoherent; the flat middle keeps most of the window's energy for the true peak.
fn capture_taper(window: usize) -> Vec<f64> {
    let edge = ((window as f64 * TAPER_FRACTION) as usize).max(1);
    (0..window)
        .map(|i| {
            let d = i.min(window - 1 - i);
            if d >= edge {
                1.0
            } else {
                0.5 - 0.5 * (std::f64::consts::PI * (d as f64 + 0.5) / edge as f64).cos()
            }
        })
        .collect()
}

/// Lags within this distance of the peak are its main lobe and excluded from the sidelobe
/// statistic.
const MAIN_LOBE: usize = 8;

impl GccPhat {
    /// Estimator for windows of `window` samples and search spans up to `max_span`.
    /// Allocates one plan per power-of-two FFT size the spans can need.
    pub fn new(window: usize, max_span: usize) -> Self {
        let mut planner = RealFftPlanner::<f64>::new();
        // Zero-padded to window + span so the searched lags never wrap.
        let smallest = (window + 1).next_power_of_two();
        let largest = (window + max_span + 1).next_power_of_two();
        let mut plans = Vec::new();
        let mut n = smallest;
        while n <= largest {
            let fwd = planner.plan_fft_forward(n);
            let inv = planner.plan_fft_inverse(n);
            let scratch_len = fwd.get_scratch_len().max(inv.get_scratch_len());
            plans.push(Plan {
                n,
                a: vec![0.0; n],
                b: vec![0.0; n],
                sa: fwd.make_output_vec(),
                sb: fwd.make_output_vec(),
                scratch: vec![Complex::new(0.0, 0.0); scratch_len],
                fwd,
                inv,
            });
            n *= 2;
        }
        Self {
            window,
            max_span,
            plans,
            taper: capture_taper(window),
        }
    }

    /// Window length.
    pub fn window(&self) -> usize {
        self.window
    }

    /// GCC-PHAT peak of `capture` (W samples from `capture_start`) against `reference`
    /// (generator history from output index `range.reference_start(capture_start)`).
    /// Returns `None` if either input is silent.
    pub fn correlate(
        &mut self,
        capture: &[f32],
        reference: &[f32],
        range: LagRange,
    ) -> Result<Option<Peak>, TimingError> {
        if capture.len() != self.window {
            return Err(TimingError::CaptureLength);
        }
        let span = range.span();
        if span > self.max_span {
            return Err(TimingError::RangeTooWide);
        }
        if reference.len() != range.reference_len(self.window) {
            return Err(TimingError::ReferenceLength);
        }
        let need = (self.window + span + 1).next_power_of_two();
        let Some(p) = self.plans.iter_mut().find(|p| p.n >= need) else {
            return Err(TimingError::RangeTooWide);
        };
        let n = p.n;
        p.a.fill(0.0);
        p.b.fill(0.0);
        for ((d, &s), w) in p.a.iter_mut().zip(capture).zip(&self.taper) {
            *d = f64::from(s) * w;
        }
        for (d, &s) in p.b.iter_mut().zip(reference) {
            *d = f64::from(s);
        }
        // Sizes match the plan, so the transforms cannot fail.
        let _ = p
            .fwd
            .process_with_scratch(&mut p.a, &mut p.sa, &mut p.scratch);
        let _ = p
            .fwd
            .process_with_scratch(&mut p.b, &mut p.sb, &mut p.scratch);
        // r[k] = Σ c[i]·ref[i + k]  ↔  conj(C)·REF; PHAT keeps only the phase.
        let mut mean_mag = 0.0;
        for (x, y) in p.sa.iter_mut().zip(&p.sb) {
            *x = x.conj() * y;
            mean_mag += x.norm();
        }
        mean_mag /= p.sa.len() as f64;
        if mean_mag <= 0.0 || !mean_mag.is_finite() {
            return Ok(None);
        }
        // A small floor keeps empty bins (band-limited stimulus) from being amplified.
        let floor = 1e-6 * mean_mag;
        for x in p.sa.iter_mut() {
            *x /= x.norm() + floor;
        }
        let last = p.sa.len() - 1;
        p.sa[0] = Complex::new(0.0, 0.0);
        p.sa[last] = Complex::new(0.0, 0.0);
        let _ = p
            .inv
            .process_with_scratch(&mut p.sa, &mut p.a, &mut p.scratch);
        let r = &p.a;

        let mut k_best = 0;
        let mut best = f64::MIN;
        for (k, v) in r.iter().enumerate().take(span + 1) {
            if v.abs() > best {
                best = v.abs();
                k_best = k;
            }
        }
        let mut side_e = 0.0;
        let mut side_n = 0usize;
        for (k, v) in r.iter().enumerate() {
            let dist = k.abs_diff(k_best).min(n - k.abs_diff(k_best));
            if dist > MAIN_LOBE {
                side_e += v * v;
                side_n += 1;
            }
        }
        let side_rms = (side_e / side_n.max(1) as f64).sqrt();
        let half = 0.5 * best;
        let below = |k: usize| r[k % n].abs() < half;
        let lo = (1..n / 2).find(|&d| below(k_best + n - d)).unwrap_or(n / 2);
        let hi = (1..n / 2).find(|&d| below(k_best + d)).unwrap_or(n / 2);
        let lobe = lo + hi - 1;
        let psr_db = 20.0 * (best / side_rms).log10();
        let delta = if k_best > 0 && k_best < span {
            let (ym, y0, yp) = (r[k_best - 1].abs(), best, r[k_best + 1].abs());
            let den = ym - 2.0 * y0 + yp;
            if den < 0.0 {
                (0.5 * (ym - yp) / den).clamp(-0.5, 0.5)
            } else {
                0.0
            }
        } else {
            0.0
        };
        // Lag k corresponds to offset range.max − k.
        Ok(Some(Peak {
            offset: range.max - k_best as i64,
            fraction: -delta,
            psr_db,
            lobe,
        }))
    }

    /// Correlates one window and applies the floors in `cfg`.
    pub fn measure(
        &mut self,
        cfg: &TimingConfig,
        capture_start: u64,
        capture: &[f32],
        reference: &[f32],
        range: LagRange,
    ) -> Result<WindowMeasurement, TimingError> {
        if capture.len() != self.window {
            return Err(TimingError::CaptureLength);
        }
        if reference.len() != range.reference_len(self.window) {
            return Err(TimingError::ReferenceLength);
        }
        let newest = range.span();
        if let Some(m) =
            self.measure_no_stimulus(cfg, capture_start, capture, &reference[newest..])?
        {
            return Ok(m);
        }
        let stimulus_dbfs = level_dbfs(&reference[newest..]);
        let loopback_dbfs = level_dbfs(capture);
        let outcome = if loopback_dbfs < cfg.loopback_floor_dbfs {
            Outcome::NoEstimate(NoEstimate::LowLoopbackLevel)
        } else {
            match self.correlate(capture, reference, range)? {
                // A wide lobe is the stimulus' own (a missing or unrelated loopback whitens to
                // a narrow one), so it is judged before the peak's clarity.
                Some(p) if p.lobe as f64 > cfg.max_lobe_s * cfg.sample_rate => {
                    Outcome::NoEstimate(NoEstimate::Narrowband)
                }
                Some(p) if p.psr_db < cfg.psr_floor_db => {
                    Outcome::NoEstimate(NoEstimate::LowConfidence)
                }
                Some(p) => Outcome::Offset(p),
                None => Outcome::NoEstimate(NoEstimate::LowConfidence),
            }
        };
        Ok(WindowMeasurement {
            capture_start,
            outcome,
            loopback_dbfs,
            stimulus_dbfs,
        })
    }

    /// The window's measurement if its stimulus is below the floor, judged from `newest`
    /// alone: the W history samples from `range.newest_start(capture_start)`. A wide search
    /// range reaches up to a second back and would otherwise see a generator that has
    /// stopped, so presence never depends on the older part of the slice, and a window
    /// without stimulus needs nothing else. `None`: the stimulus is present and the window
    /// needs [`Self::measure`] with its whole slice.
    pub fn measure_no_stimulus(
        &self,
        cfg: &TimingConfig,
        capture_start: u64,
        capture: &[f32],
        newest: &[f32],
    ) -> Result<Option<WindowMeasurement>, TimingError> {
        // Lengths are checked even in silence so a caller bug is never masked by it.
        if capture.len() != self.window {
            return Err(TimingError::CaptureLength);
        }
        if newest.len() != self.window {
            return Err(TimingError::ReferenceLength);
        }
        let stimulus_dbfs = level_dbfs(newest);
        Ok(
            (stimulus_dbfs < cfg.stimulus_floor_dbfs).then(|| WindowMeasurement {
                capture_start,
                outcome: Outcome::NoStimulus,
                loopback_dbfs: level_dbfs(capture),
                stimulus_dbfs,
            }),
        )
    }
}

/// RMS level in dBFS (0 dBFS = full-scale sine RMS, design Q4); −∞ for silence.
fn level_dbfs(x: &[f32]) -> f64 {
    if x.is_empty() {
        return f64::NEG_INFINITY;
    }
    let p = x.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / x.len() as f64;
    10.0 * (2.0 * p).log10()
}

#[cfg(test)]
mod tests;
