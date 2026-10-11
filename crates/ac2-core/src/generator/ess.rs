//! Exponential sine sweep: the sample-exact plan and its inverse filter.

use std::f64::consts::{PI, SQRT_2, TAU};

use realfft::RealFftPlanner;
use realfft::num_complex::Complex;

use super::{EssConfig, GeneratorError, check_freq, check_rate, dbfs_to_rms};

// ---------------------------------------------------------------------------------------
// ESS

/// Derived, sample-exact sweep plan shared by the generator and the inverse filter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EssPlan {
    /// Start frequency, Hz.
    pub start_hz: f64,
    /// Rate constant L, s: instantaneous frequency is `start_hz · e^(t/L)`.
    pub rate_s: f64,
    /// Sweep length in samples.
    pub len: usize,
    fade_in: usize,
    fade_out: usize,
    fs: f64,
}

impl EssPlan {
    /// Validates `cfg` and derives the synchronised sweep.
    pub fn new(cfg: &EssConfig, fs: f64) -> Result<Self, GeneratorError> {
        check_rate(fs)?;
        check_freq(cfg.start_hz, fs)?;
        if !(cfg.end_hz.is_finite() && cfg.end_hz > cfg.start_hz && cfg.end_hz <= fs / 2.0) {
            return Err(GeneratorError::InvalidFrequency);
        }
        let ln_ratio = (cfg.end_hz / cfg.start_hz).ln();
        if !(cfg.duration_s.is_finite() && cfg.duration_s > 0.0) {
            return Err(GeneratorError::InvalidSweep);
        }
        let cycles = (cfg.start_hz * cfg.duration_s / ln_ratio).round();
        if cycles < 1.0 {
            return Err(GeneratorError::InvalidSweep);
        }
        let rate_s = cycles / cfg.start_hz;
        let len = (rate_s * ln_ratio * fs).floor() as usize;
        let fade = |s: f64| -> Result<usize, GeneratorError> {
            if s.is_finite() && s >= 0.0 {
                Ok((s * fs).round() as usize)
            } else {
                Err(GeneratorError::InvalidSweep)
            }
        };
        let fade_in = fade(cfg.fade_in_s)?;
        let fade_out = fade(cfg.fade_out_s)?;
        if fade_in + fade_out > len {
            return Err(GeneratorError::InvalidSweep);
        }
        Ok(Self {
            start_hz: cfg.start_hz,
            rate_s,
            len,
            fade_in,
            fade_out,
            fs,
        })
    }

    /// Actual sweep duration, s.
    pub fn duration_s(&self) -> f64 {
        self.len as f64 / self.fs
    }

    /// Unit-peak sweep sample `n` (0 outside the sweep), including fades.
    pub fn sample(&self, n: usize) -> f64 {
        if n >= self.len {
            return 0.0;
        }
        let t = n as f64 / self.fs;
        let phase = TAU * self.start_hz * self.rate_s * ((t / self.rate_s).exp_m1());
        let mut env = 1.0;
        if n < self.fade_in {
            env = 0.5 - 0.5 * (PI * n as f64 / self.fade_in as f64).cos();
        }
        let from_end = self.len - 1 - n;
        if from_end < self.fade_out {
            env *= 0.5 - 0.5 * (PI * from_end as f64 / self.fade_out as f64).cos();
        }
        env * phase.sin()
    }
}

/// Inverse filter matched to an emitted sweep (Farina): time-reversed sweep with a
/// +6 dB/oct envelope, scaled so that sweep ⊛ inverse has unit gain mid-band. Built off the
/// audio path; allocates.
#[derive(Debug, Clone)]
pub struct EssInverse {
    /// Filter taps.
    pub taps: Vec<f64>,
    /// Lag at which the linear impulse response appears in [`EssInverse::deconvolve`]
    /// output (sweep length − 1). Harmonic responses appear before it.
    pub latency: usize,
}

impl EssInverse {
    /// Inverse for the sweep `cfg` emitted at `level_dbfs` (the generator's ESS level), so
    /// deconvolving a unit-gain loopback yields a band-limited unit impulse.
    pub fn new(cfg: &EssConfig, fs: f64, level_dbfs: f64) -> Result<Self, GeneratorError> {
        if !level_dbfs.is_finite() {
            return Err(GeneratorError::NonFiniteLevel);
        }
        let plan = EssPlan::new(cfg, fs)?;
        let m = plan.len;
        let amp = dbfs_to_rms(level_dbfs) * SQRT_2;
        // The sweep spends time ∝ 1/f per Hz, so its energy density falls 3 dB/oct; the
        // inverse's envelope rises 6 dB/oct across the reversed sweep to flatten the product.
        let mut taps: Vec<f64> = (0..m)
            .map(|n| plan.sample(m - 1 - n) * (-(n as f64) / (fs * plan.rate_s)).exp())
            .collect();
        let sweep: Vec<f64> = (0..m).map(|n| amp * plan.sample(n)).collect();
        let n_fft = (2 * m).next_power_of_two();
        let mut planner = RealFftPlanner::<f64>::new();
        let fwd = planner.plan_fft_forward(n_fft);
        let spec = |x: &[f64]| -> Vec<Complex<f64>> {
            let mut buf = vec![0.0; n_fft];
            buf[..x.len()].copy_from_slice(x);
            let mut out = fwd.make_output_vec();
            // Lengths match the plan, so the transform cannot fail.
            let _ = fwd.process(&mut buf, &mut out);
            out
        };
        let s = spec(&sweep);
        let i = spec(&taps);
        let centre = (cfg.start_hz * cfg.end_hz).sqrt();
        let (lo, hi) = (centre / SQRT_2, centre * SQRT_2);
        let bin_hz = fs / n_fft as f64;
        let (mut sum, mut count) = (0.0, 0usize);
        for k in (lo / bin_hz).ceil() as usize..=(hi / bin_hz).floor() as usize {
            sum += (s[k] * i[k]).norm();
            count += 1;
        }
        if count == 0 || sum <= 0.0 {
            return Err(GeneratorError::InvalidSweep);
        }
        let scale = count as f64 / sum;
        taps.iter_mut().for_each(|t| *t *= scale);
        Ok(Self {
            taps,
            latency: m - 1,
        })
    }

    /// Linear convolution of `capture` with the inverse (length capture + taps − 1).
    pub fn deconvolve(&self, capture: &[f64]) -> Vec<f64> {
        let out_len = capture.len() + self.taps.len() - 1;
        let n_fft = out_len.next_power_of_two();
        let mut planner = RealFftPlanner::<f64>::new();
        let fwd = planner.plan_fft_forward(n_fft);
        let inv = planner.plan_fft_inverse(n_fft);
        let mut a = vec![0.0; n_fft];
        a[..capture.len()].copy_from_slice(capture);
        let mut b = vec![0.0; n_fft];
        b[..self.taps.len()].copy_from_slice(&self.taps);
        let mut sa = fwd.make_output_vec();
        let mut sb = fwd.make_output_vec();
        let _ = fwd.process(&mut a, &mut sa);
        let _ = fwd.process(&mut b, &mut sb);
        for (x, y) in sa.iter_mut().zip(&sb) {
            *x *= y / n_fft as f64;
        }
        sa[0].im = 0.0;
        if let Some(last) = sa.last_mut() {
            last.im = 0.0;
        }
        let mut out = inv.make_output_vec();
        let _ = inv.process(&mut sa, &mut out);
        out.truncate(out_len);
        out
    }
}
