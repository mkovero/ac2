//! Stimulus generation: noise, periodic pink, sine, ESS; level conventions per design Q4.
//!
//! Everything here is built for an audio callback: [`Generator::new`] does all allocation,
//! planning and filter design; [`Generator::fill`] only does arithmetic and atomic loads, for
//! any block size, and the output does not depend on how the stream is cut into blocks.
//!
//! Level convention (decision 4a): the level is the signal's RMS in dBFS, where 0 dBFS RMS is
//! the RMS of a full-scale sine (1/√2 FS). A level whose RMS times the signal's crest factor
//! would exceed full scale is refused with the maximum achievable level; nothing clips
//! silently.
//!
//! Every source produces a unit-RMS sequence; the output is that sequence times a gain that
//! ramps linearly over [`RAMP_SECONDS`] whenever the level or mute changes, so level changes
//! never step.

use std::f64::consts::{FRAC_1_SQRT_2, PI, SQRT_2, TAU};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use realfft::RealFftPlanner;
use realfft::num_complex::Complex;

/// Duration of the linear gain ramp applied to every level or mute change.
pub const RAMP_SECONDS: f64 = 0.020;

/// Duration of [`LevelControl::fade_out`]; equal to the gain ramp.
pub const FADE_OUT_SECONDS: f64 = RAMP_SECONDS;

/// Crest factor assumed for filtered free-running noise, whose true peak is unbounded in
/// practice (an exact bound from the impulse response's L1 norm is used when it is lower).
///
/// The noise is driven by bounded uniform samples, so its tails are lighter than Gaussian;
/// a Gaussian exceeds 6σ with probability 2·10⁻⁹ per sample. Any sample that still exceeds
/// full scale is saturated and counted in [`Generator::clipped_samples`].
pub const FILTERED_NOISE_CREST: f64 = 6.0;

/// Pink noise is −3 dB/oct from this frequency up; below it the spectrum flattens so the
/// signal carries no runaway infrasonic energy.
pub const PINK_CORNER_HZ: f64 = 5.0;

/// Band over which pink noise is specified to follow −3 dB/oct within
/// [`PINK_FLATNESS_DB`].
pub const PINK_SPEC_BAND_HZ: (f64, f64) = (20.0, 20_000.0);

/// Maximum deviation (±dB) of the free-running pink filter from an ideal −3 dB/oct line
/// over [`PINK_SPEC_BAND_HZ`] (or up to 0.45·fs at rates below 44.1 kHz).
pub const PINK_FLATNESS_DB: f64 = 0.5;

const SQRT_3: f64 = 1.732_050_807_568_877_2;

/// Converts a level in dBFS RMS to an RMS value in FS units (0 dBFS → 1/√2).
pub fn dbfs_to_rms(level_dbfs: f64) -> f64 {
    10f64.powf(level_dbfs / 20.0) * FRAC_1_SQRT_2
}

/// Converts an RMS value in FS units to dBFS RMS (1/√2 → 0 dBFS).
pub fn rms_to_dbfs(rms: f64) -> f64 {
    20.0 * (rms * SQRT_2).log10()
}

/// The sample-peak limit, dB re full scale, that matches an RMS ceiling: the largest crest
/// factor any generator signal is allowed ([`FILTERED_NOISE_CREST`]) times the ceiling RMS,
/// capped at full scale. Generators refuse levels whose own crest would exceed it, so an
/// output path enforcing this limit acts only on a computation error upstream.
pub fn peak_limit_db(ceiling_dbfs: f64) -> f64 {
    let peak = (dbfs_to_rms(ceiling_dbfs) * FILTERED_NOISE_CREST).min(1.0);
    (20.0 * peak.max(1e-12).log10()).min(0.0)
}

/// Highest level (dBFS RMS) at which a signal with this crest factor stays within full scale.
pub fn max_level_for_crest(crest: f64) -> f64 {
    rms_to_dbfs(1.0 / crest)
}

/// Why a generator configuration or level was refused.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GeneratorError {
    /// The requested RMS times the crest factor would exceed full scale.
    WouldClip {
        /// Requested level, dBFS RMS.
        requested_dbfs: f64,
        /// Highest level this signal can be emitted at without clipping, dBFS RMS.
        max_dbfs: f64,
    },
    /// The requested level is above the configured global maximum.
    AboveCeiling {
        /// Requested level, dBFS RMS.
        requested_dbfs: f64,
        /// Global maximum level, dBFS RMS.
        ceiling_dbfs: f64,
    },
    /// The level is NaN or infinite.
    NonFiniteLevel,
    /// The sample rate is outside 8 kHz … 768 kHz.
    InvalidSampleRate,
    /// A frequency is not finite, not positive or not below Nyquist.
    InvalidFrequency,
    /// Band-limit corners are not ordered high-pass < low-pass.
    InvalidBand,
    /// Band-limiting only applies to noise signals.
    BandLimitNotApplicable,
    /// The periodic-noise period is not a power of two in 2^10 … 2^24.
    InvalidPeriod,
    /// ESS duration or fades are invalid, or too short for one start-frequency cycle.
    InvalidSweep,
    /// A periodic-noise period cannot satisfy the requested search width and tail.
    InvalidSearchSpan,
}

impl std::fmt::Display for GeneratorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WouldClip {
                requested_dbfs,
                max_dbfs,
            } => write!(
                f,
                "{requested_dbfs:.2} dBFS would clip; maximum for this signal is {max_dbfs:.2} dBFS"
            ),
            Self::AboveCeiling {
                requested_dbfs,
                ceiling_dbfs,
            } => write!(
                f,
                "{requested_dbfs:.2} dBFS is above the global maximum {ceiling_dbfs:.2} dBFS"
            ),
            Self::NonFiniteLevel => write!(f, "level is not finite"),
            Self::InvalidSampleRate => write!(f, "sample rate out of range"),
            Self::InvalidFrequency => write!(f, "frequency must be positive and below Nyquist"),
            Self::InvalidBand => write!(f, "high-pass corner must be below low-pass corner"),
            Self::BandLimitNotApplicable => write!(f, "band-limiting applies to noise only"),
            Self::InvalidPeriod => write!(f, "period must be a power of two in 2^10..=2^24"),
            Self::InvalidSweep => write!(f, "invalid sweep duration or fades"),
            Self::InvalidSearchSpan => write!(f, "search width and tail must be non-negative"),
        }
    }
}

impl std::error::Error for GeneratorError {}

/// Exponential sine sweep parameters (Farina; synchronised per Novak et al. 2015).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EssConfig {
    /// Start frequency, Hz.
    pub start_hz: f64,
    /// End frequency, Hz (≤ Nyquist).
    pub end_hz: f64,
    /// Requested duration, s. The rate constant is rounded so that `start_hz · L` is an
    /// integer, which puts every harmonic's impulse response in phase; the actual duration
    /// is [`EssPlan::duration_s`].
    pub duration_s: f64,
    /// Half-cosine fade-in, s, so the sweep starts without a step.
    pub fade_in_s: f64,
    /// Half-cosine fade-out, s.
    pub fade_out_s: f64,
}

/// Noise and tone sources.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Signal {
    /// Uniform white noise (crest factor √3 when not band-limited).
    White,
    /// Free-running pink noise from a filtered white source.
    Pink,
    /// Pink noise that repeats exactly every `period` samples (power of two, see
    /// [`periodic_period`]); random phase, exact 1/√f magnitude per bin.
    PeriodicPink {
        /// Period in samples.
        period: usize,
    },
    /// Sine.
    Sine {
        /// Frequency, Hz.
        freq_hz: f64,
    },
    /// One exponential sine sweep, then silence.
    Ess(EssConfig),
}

/// Butterworth order of band-limit filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilterOrder {
    /// 12 dB/oct.
    Second,
    /// 24 dB/oct.
    Fourth,
}

/// Optional Butterworth high-pass and low-pass applied to noise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandLimit {
    /// High-pass corner, Hz.
    pub highpass_hz: Option<f64>,
    /// Low-pass corner, Hz.
    pub lowpass_hz: Option<f64>,
    /// Slope of both filters.
    pub order: FilterOrder,
}

impl BandLimit {
    /// Full band, no filters.
    pub const NONE: BandLimit = BandLimit {
        highpass_hz: None,
        lowpass_hz: None,
        order: FilterOrder::Second,
    };

    fn is_none(&self) -> bool {
        self.highpass_hz.is_none() && self.lowpass_hz.is_none()
    }
}

/// Everything needed to build a [`Generator`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeneratorConfig {
    /// What to emit.
    pub signal: Signal,
    /// Sample rate, Hz.
    pub sample_rate: f64,
    /// RNG seed; the same seed gives the same samples.
    pub seed: u64,
    /// Band-limiting (noise only).
    pub band: BandLimit,
    /// Initial level, dBFS RMS.
    pub level_dbfs: f64,
    /// Global maximum level, dBFS RMS; requests above it are refused.
    pub ceiling_dbfs: f64,
}

/// Smallest power-of-two period P (samples) for periodic noise with P > (W + T)·fs and
/// P ≥ `min_span_samples` (PLAN §5.4).
///
/// `search_width_s` is the full width W of the delay search interval (2 s for ±1 s) and
/// `tail_s` the significant response tail T. `min_span_samples` is the largest analysis
/// stage's window span at full rate.
pub fn periodic_period(
    search_width_s: f64,
    tail_s: f64,
    sample_rate: f64,
    min_span_samples: usize,
) -> Result<usize, GeneratorError> {
    check_rate(sample_rate)?;
    if !(search_width_s.is_finite() && tail_s.is_finite()) || search_width_s < 0.0 || tail_s < 0.0 {
        return Err(GeneratorError::InvalidSearchSpan);
    }
    // P must strictly exceed the span, so the smallest admissible P is span + 1.
    let span = ((search_width_s + tail_s) * sample_rate).ceil() as usize;
    let p = (span + 1).max(min_span_samples).max(1).next_power_of_two();
    if p > MAX_PERIOD {
        return Err(GeneratorError::InvalidSearchSpan);
    }
    Ok(p.max(MIN_PERIOD))
}

const MIN_PERIOD: usize = 1 << 10;
const MAX_PERIOD: usize = 1 << 24;

fn check_rate(fs: f64) -> Result<(), GeneratorError> {
    if fs.is_finite() && (8_000.0..=768_000.0).contains(&fs) {
        Ok(())
    } else {
        Err(GeneratorError::InvalidSampleRate)
    }
}

fn check_freq(f: f64, fs: f64) -> Result<(), GeneratorError> {
    if f.is_finite() && f > 0.0 && f < fs / 2.0 {
        Ok(())
    } else {
        Err(GeneratorError::InvalidFrequency)
    }
}

// ---------------------------------------------------------------------------------------
// RNG

/// xoshiro256++ seeded through SplitMix64: small, fast, deterministic across platforms.
#[derive(Debug, Clone)]
pub(crate) struct Rng {
    s: [u64; 4],
}

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        let mut x = seed;
        let mut next = || {
            x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = x;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        Self {
            s: [next(), next(), next(), next()],
        }
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        let s = &mut self.s;
        let result = s[0].wrapping_add(s[3]).rotate_left(23).wrapping_add(s[0]);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        result
    }

    /// Uniform in [0, 1).
    pub(crate) fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Uniform in [−√3, √3): zero mean, unit variance, bounded.
    pub(crate) fn uniform_unit_rms(&mut self) -> f64 {
        (2.0 * self.unit() - 1.0) * SQRT_3
    }
}

// ---------------------------------------------------------------------------------------
// Filters

/// Second-order section, transposed direct form II, f64 state (low corners at high rates
/// need the precision).
#[derive(Debug, Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    s: [f64; 2],
}

impl Biquad {
    /// RBJ high/low-pass (bilinear, prewarped at the corner).
    fn new(highpass: bool, f: f64, q: f64, fs: f64) -> Self {
        let w0 = TAU * f / fs;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        let (b0, b1) = if highpass {
            ((1.0 + cos) / 2.0, -(1.0 + cos))
        } else {
            ((1.0 - cos) / 2.0, 1.0 - cos)
        };
        Self {
            b: [b0 / a0, b1 / a0, b0 / a0],
            a: [-2.0 * cos / a0, (1.0 - alpha) / a0],
            s: [0.0; 2],
        }
    }

    #[inline]
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.s[0];
        self.s[0] = self.b[1] * x - self.a[0] * y + self.s[1];
        self.s[1] = self.b[2] * x - self.a[1] * y;
        y
    }

    fn response(&self, f: f64, fs: f64) -> Complex<f64> {
        let z1 = Complex::from_polar(1.0, -TAU * f / fs);
        let z2 = z1 * z1;
        let num = self.b[0] + self.b[1] * z1 + self.b[2] * z2;
        let den = 1.0 + self.a[0] * z1 + self.a[1] * z2;
        num / den
    }
}

fn band_filters(band: &BandLimit, fs: f64) -> Result<Vec<Biquad>, GeneratorError> {
    if let Some(f) = band.highpass_hz {
        check_freq(f, fs)?;
    }
    if let Some(f) = band.lowpass_hz {
        check_freq(f, fs)?;
    }
    if let (Some(h), Some(l)) = (band.highpass_hz, band.lowpass_hz)
        && h >= l
    {
        return Err(GeneratorError::InvalidBand);
    }
    let qs: &[f64] = match band.order {
        FilterOrder::Second => &[FRAC_1_SQRT_2],
        // Butterworth pole pairs: Q = 1 / (2 cos θ), θ = π/8, 3π/8.
        FilterOrder::Fourth => &[0.541_196_100_146_197, 1.306_562_964_876_376_7],
    };
    let mut out = Vec::new();
    for (corner, hp) in [(band.highpass_hz, true), (band.lowpass_hz, false)] {
        if let Some(f) = corner {
            out.extend(qs.iter().map(|&q| Biquad::new(hp, f, q, fs)));
        }
    }
    Ok(out)
}

/// First-order pole/zero section (1 − b z⁻¹) / (1 − a z⁻¹), direct form I.
#[derive(Debug, Clone, Copy)]
struct PoleZero {
    a: f64,
    b: f64,
    x1: f64,
    y1: f64,
}

/// Free-running pink filter: a ladder of real pole/zero pairs, one per octave, with each
/// zero half an octave above its pole. Between a pole and its zero the slope is −6 dB/oct
/// and flat to the next pole, which averages to −3 dB/oct with small ripple.
///
/// Matched-z placement is exact far below Nyquist but runs flat near Nyquist (the ideal
/// slope continues above fs/2, a first-order digital section cannot follow it). The two
/// topmost sections are therefore refitted per sample rate by least squares against the
/// ideal slope over the specified band; the fit lands well inside [`PINK_FLATNESS_DB`].
#[derive(Debug, Clone)]
struct PinkFilter {
    sections: Vec<PoleZero>,
}

impl PinkFilter {
    const REFIT: usize = 2;

    fn design(fs: f64) -> Self {
        let mut ab = Vec::new();
        let mut f = PINK_CORNER_HZ;
        while f < fs / 2.0 {
            ab.push(((-TAU * f / fs).exp(), (-TAU * f * SQRT_2 / fs).exp()));
            f *= 2.0;
        }
        let (lo, hi) = Self::fit_band(fs);
        let freqs: Vec<f64> = (0..200)
            .map(|i| lo * (hi / lo).powf(i as f64 / 199.0))
            .collect();
        let n_fixed = ab.len() - Self::REFIT;
        // Ideal-relative log magnitude of the fixed sections at each fit frequency.
        let fixed: Vec<f64> = freqs
            .iter()
            .map(|&f| {
                let z1 = Complex::from_polar(1.0, -TAU * f / fs);
                let mut db = 10.0 * f.log10();
                for &(a, b) in &ab[..n_fixed] {
                    db += section_db(a, b, z1);
                }
                db
            })
            .collect();
        let z1s: Vec<Complex<f64>> = freqs
            .iter()
            .map(|&f| Complex::from_polar(1.0, -TAU * f / fs))
            .collect();
        let cost = |x: &[f64; 4]| -> f64 {
            if x.iter().any(|v| v.abs() > 0.999) {
                return 1e6;
            }
            let mut sum = 0.0;
            let mut sum_sq = 0.0;
            for (d0, &z1) in fixed.iter().zip(&z1s) {
                let d = d0 + section_db(x[0], x[1], z1) + section_db(x[2], x[3], z1);
                sum += d;
                sum_sq += d * d;
            }
            let n = fixed.len() as f64;
            sum_sq / n - (sum / n) * (sum / n)
        };
        let start = [
            ab[n_fixed].0,
            ab[n_fixed].1,
            ab[n_fixed + 1].0,
            ab[n_fixed + 1].1,
        ];
        let x = nelder_mead(cost, start, 0.05, 4000);
        ab[n_fixed] = (x[0], x[1]);
        ab[n_fixed + 1] = (x[2], x[3]);
        Self {
            sections: ab
                .into_iter()
                .map(|(a, b)| PoleZero {
                    a,
                    b,
                    x1: 0.0,
                    y1: 0.0,
                })
                .collect(),
        }
    }

    fn fit_band(fs: f64) -> (f64, f64) {
        (PINK_SPEC_BAND_HZ.0, PINK_SPEC_BAND_HZ.1.min(0.4535 * fs))
    }

    #[inline]
    fn process(&mut self, mut x: f64) -> f64 {
        for s in &mut self.sections {
            let y = x - s.b * s.x1 + s.a * s.y1;
            s.x1 = x;
            s.y1 = y;
            x = y;
        }
        x
    }

    /// Frequency response; the design is verified against it.
    #[cfg(test)]
    fn response(&self, f: f64, fs: f64) -> Complex<f64> {
        let z1 = Complex::from_polar(1.0, -TAU * f / fs);
        self.sections
            .iter()
            .map(|s| (1.0 - s.b * z1) / (1.0 - s.a * z1))
            .product()
    }
}

fn section_db(a: f64, b: f64, z1: Complex<f64>) -> f64 {
    10.0 * ((1.0 - b * z1).norm_sqr() / (1.0 - a * z1).norm_sqr()).log10()
}

/// Nelder–Mead minimiser in 4 dimensions (standard coefficients), deterministic.
fn nelder_mead(f: impl Fn(&[f64; 4]) -> f64, x0: [f64; 4], step: f64, iters: usize) -> [f64; 4] {
    const N: usize = 4;
    let mut pts = [x0; N + 1];
    for (i, p) in pts.iter_mut().skip(1).enumerate() {
        p[i] += step;
    }
    let mut vals = pts.map(|p| f(&p));
    for _ in 0..iters {
        let mut idx = [0, 1, 2, 3, 4];
        idx.sort_by(|&i, &j| vals[i].total_cmp(&vals[j]));
        pts = idx.map(|i| pts[i]);
        vals = idx.map(|i| vals[i]);
        if (vals[N] - vals[0]).abs() < 1e-14 {
            break;
        }
        let mut c = [0.0; N];
        for p in &pts[..N] {
            for d in 0..N {
                c[d] += p[d] / N as f64;
            }
        }
        let lerp = |t: f64| -> [f64; N] { std::array::from_fn(|d| c[d] + t * (pts[N][d] - c[d])) };
        let xr = lerp(-1.0);
        let fr = f(&xr);
        if fr < vals[0] {
            let xe = lerp(-2.0);
            let fe = f(&xe);
            (pts[N], vals[N]) = if fe < fr { (xe, fe) } else { (xr, fr) };
        } else if fr < vals[N - 1] {
            (pts[N], vals[N]) = (xr, fr);
        } else {
            let xc = if fr < vals[N] { lerp(-0.5) } else { lerp(0.5) };
            let fc = f(&xc);
            if fc < vals[N].min(fr) {
                (pts[N], vals[N]) = (xc, fc);
            } else {
                for i in 1..=N {
                    pts[i] = std::array::from_fn(|d| pts[0][d] + 0.5 * (pts[i][d] - pts[0][d]));
                    vals[i] = f(&pts[i]);
                }
            }
        }
    }
    let best = (0..=N)
        .min_by(|&i, &j| vals[i].total_cmp(&vals[j]))
        .unwrap_or(0);
    pts[best]
}

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

// ---------------------------------------------------------------------------------------
// Level control

#[derive(Debug)]
struct LevelShared {
    /// Target RMS (FS units) as f64 bits.
    target_rms: AtomicU64,
    muted: AtomicBool,
    crest: f64,
    ceiling_dbfs: f64,
}

/// Thread-safe handle for level and mute. Setters are lock-free atomic stores; the
/// generator picks the new value up at its next [`Generator::fill`] and ramps to it over
/// [`RAMP_SECONDS`].
#[derive(Debug, Clone)]
pub struct LevelControl {
    shared: Arc<LevelShared>,
}

impl LevelControl {
    /// Sets the level in dBFS RMS, refusing levels that would clip or exceed the ceiling.
    /// A refused request leaves the current level unchanged.
    pub fn set_level_dbfs(&self, level_dbfs: f64) -> Result<(), GeneratorError> {
        let rms = validate_level(level_dbfs, self.shared.crest, self.shared.ceiling_dbfs)?;
        self.shared
            .target_rms
            .store(rms.to_bits(), Ordering::Release);
        Ok(())
    }

    /// Current target level, dBFS RMS (ignores mute).
    pub fn level_dbfs(&self) -> f64 {
        rms_to_dbfs(f64::from_bits(
            self.shared.target_rms.load(Ordering::Acquire),
        ))
    }

    /// Mutes or unmutes with a ramp.
    pub fn set_muted(&self, muted: bool) {
        self.shared.muted.store(muted, Ordering::Release);
    }

    /// Whether mute is requested.
    pub fn is_muted(&self) -> bool {
        self.shared.muted.load(Ordering::Acquire)
    }

    /// Ramps to silence over [`FADE_OUT_SECONDS`]; the stream may stop once
    /// [`Generator::is_silent`] is true.
    pub fn fade_out(&self) {
        self.set_muted(true);
    }

    /// Highest level this signal can be emitted at without clipping, dBFS RMS.
    pub fn max_level_dbfs(&self) -> f64 {
        max_level_for_crest(self.shared.crest)
    }
}

fn validate_level(level_dbfs: f64, crest: f64, ceiling_dbfs: f64) -> Result<f64, GeneratorError> {
    if !level_dbfs.is_finite() {
        return Err(GeneratorError::NonFiniteLevel);
    }
    if level_dbfs > ceiling_dbfs {
        return Err(GeneratorError::AboveCeiling {
            requested_dbfs: level_dbfs,
            ceiling_dbfs,
        });
    }
    let rms = dbfs_to_rms(level_dbfs);
    // Allow rounding at exactly full scale (a 0 dBFS sine peaks at 1.0).
    if rms * crest > 1.0 + 1e-9 {
        return Err(GeneratorError::WouldClip {
            requested_dbfs: level_dbfs,
            max_dbfs: max_level_for_crest(crest),
        });
    }
    Ok(rms)
}

// ---------------------------------------------------------------------------------------
// Generator

#[derive(Debug)]
enum Source {
    Noise {
        rng: Rng,
        pink: Option<PinkFilter>,
        band: Vec<Biquad>,
        /// 1 / RMS of the filter chain for unit-RMS input.
        norm: f64,
    },
    Table {
        table: Vec<f32>,
        pos: usize,
    },
    Sine {
        phase: f64,
        inc: f64,
    },
    Ess {
        plan: EssPlan,
        pos: usize,
    },
}

impl Source {
    /// Next unit-RMS sample.
    #[inline]
    fn next(&mut self) -> f64 {
        match self {
            Source::Noise {
                rng,
                pink,
                band,
                norm,
            } => {
                let mut x = rng.uniform_unit_rms();
                if let Some(p) = pink {
                    x = p.process(x);
                }
                for b in band.iter_mut() {
                    x = b.process(x);
                }
                x * *norm
            }
            Source::Table { table, pos } => {
                let v = table[*pos];
                *pos += 1;
                if *pos == table.len() {
                    *pos = 0;
                }
                f64::from(v)
            }
            Source::Sine { phase, inc } => {
                let v = phase.sin() * SQRT_2;
                *phase += *inc;
                if *phase >= TAU {
                    *phase -= TAU;
                }
                v
            }
            Source::Ess { plan, pos } => {
                let v = plan.sample(*pos) * SQRT_2;
                if *pos < plan.len {
                    *pos += 1;
                }
                v
            }
        }
    }
}

/// Real-time signal generator. Build with [`Generator::new`] off the audio thread, then
/// call [`Generator::fill`] from the callback.
#[derive(Debug)]
pub struct Generator {
    source: Source,
    control: LevelControl,
    crest: f64,
    ramp_len: u32,
    gain: f64,
    gain_target: f64,
    step: f64,
    remaining: u32,
    clipped: u64,
}

impl Generator {
    /// Builds the generator. Allocates and designs filters; refuses invalid parameters and
    /// levels that would clip.
    pub fn new(cfg: &GeneratorConfig) -> Result<Self, GeneratorError> {
        let fs = cfg.sample_rate;
        check_rate(fs)?;
        // +∞ is a valid ceiling ("no global maximum"); NaN or −∞ is not.
        if cfg.ceiling_dbfs.is_nan() || cfg.ceiling_dbfs == f64::NEG_INFINITY {
            return Err(GeneratorError::NonFiniteLevel);
        }
        let noise = matches!(
            cfg.signal,
            Signal::White | Signal::Pink | Signal::PeriodicPink { .. }
        );
        if !noise && !cfg.band.is_none() {
            return Err(GeneratorError::BandLimitNotApplicable);
        }
        let (source, crest) = match cfg.signal {
            Signal::White | Signal::Pink => {
                let pink = matches!(cfg.signal, Signal::Pink).then(|| PinkFilter::design(fs));
                let band = band_filters(&cfg.band, fs)?;
                let (l1, l2) = chain_norms(pink.clone(), &band);
                let exact_crest = SQRT_3 * l1 / l2;
                (
                    Source::Noise {
                        rng: Rng::new(cfg.seed),
                        pink,
                        band,
                        norm: 1.0 / l2,
                    },
                    exact_crest.min(FILTERED_NOISE_CREST),
                )
            }
            Signal::PeriodicPink { period } => {
                let (table, crest) = periodic_pink_table(period, fs, cfg.seed, &cfg.band)?;
                (Source::Table { table, pos: 0 }, crest)
            }
            Signal::Sine { freq_hz } => {
                check_freq(freq_hz, fs)?;
                (
                    Source::Sine {
                        phase: 0.0,
                        inc: TAU * freq_hz / fs,
                    },
                    SQRT_2,
                )
            }
            Signal::Ess(ess) => (
                Source::Ess {
                    plan: EssPlan::new(&ess, fs)?,
                    pos: 0,
                },
                SQRT_2,
            ),
        };
        let rms = validate_level(cfg.level_dbfs, crest, cfg.ceiling_dbfs)?;
        let control = LevelControl {
            shared: Arc::new(LevelShared {
                target_rms: AtomicU64::new(rms.to_bits()),
                muted: AtomicBool::new(false),
                crest,
                ceiling_dbfs: cfg.ceiling_dbfs,
            }),
        };
        let ramp_len = ((RAMP_SECONDS * fs).round() as u32).max(1);
        // A sweep carries its own fade-in and its inverse filter assumes the exact level
        // from the first sample; everything else fades in from silence.
        let start_gain = if matches!(source, Source::Ess { .. }) {
            rms
        } else {
            0.0
        };
        Ok(Self {
            source,
            control,
            crest,
            ramp_len,
            gain: start_gain,
            gain_target: start_gain,
            step: 0.0,
            remaining: 0,
            clipped: 0,
        })
    }

    /// Handle for level and mute changes from any thread.
    pub fn level_control(&self) -> LevelControl {
        self.control.clone()
    }

    /// Crest factor (peak / RMS) used for clip refusal.
    pub fn crest_factor(&self) -> f64 {
        self.crest
    }

    /// Highest level this signal can be emitted at without clipping, dBFS RMS.
    pub fn max_level_dbfs(&self) -> f64 {
        max_level_for_crest(self.crest)
    }

    /// Ramp length in samples ([`RAMP_SECONDS`] at the configured rate).
    pub fn ramp_samples(&self) -> u32 {
        self.ramp_len
    }

    /// Current output gain (FS RMS of the emitted signal before muting ramps finish).
    pub fn current_gain(&self) -> f64 {
        self.gain
    }

    /// Samples saturated at full scale since construction. Non-zero only if filtered noise
    /// exceeded [`FILTERED_NOISE_CREST`].
    pub fn clipped_samples(&self) -> u64 {
        self.clipped
    }

    /// Starts a 20 ms fade to silence (same as muting).
    pub fn fade_out(&self) {
        self.control.fade_out();
    }

    /// True when the output is exactly zero and will stay so until unmuted: muted with the
    /// ramp finished, or a sweep that has ended.
    pub fn is_silent(&self) -> bool {
        let ramp_done = self.remaining == 0 && self.gain == 0.0 && self.control.is_muted();
        ramp_done || self.is_finished()
    }

    /// True once a one-shot sweep has played to its end.
    pub fn is_finished(&self) -> bool {
        matches!(&self.source, Source::Ess { plan, pos } if *pos >= plan.len)
    }

    /// Renders the next `out.len()` samples. No allocation, no locks.
    pub fn fill(&mut self, out: &mut [f32]) {
        let target = if self.control.is_muted() {
            0.0
        } else {
            f64::from_bits(self.control.shared.target_rms.load(Ordering::Acquire))
        };
        if target.to_bits() != self.gain_target.to_bits() {
            self.gain_target = target;
            self.remaining = self.ramp_len;
            self.step = (target - self.gain) / f64::from(self.ramp_len);
        }
        for o in out.iter_mut() {
            if self.remaining > 0 {
                self.remaining -= 1;
                self.gain = if self.remaining == 0 {
                    self.gain_target
                } else {
                    self.gain + self.step
                };
            }
            let v = self.source.next() * self.gain;
            *o = if v.abs() > 1.0 {
                self.clipped += 1;
                v.signum() as f32
            } else {
                v as f32
            };
        }
    }
}

/// L1 and L2 norms of the impulse response of the noise chain (pink filter, then band
/// filters), run until the tail no longer contributes.
fn chain_norms(mut pink: Option<PinkFilter>, band: &[Biquad]) -> (f64, f64) {
    let mut band = band.to_vec();
    let (mut l1, mut e) = (0.0, 0.0);
    let mut x = 1.0;
    const BLOCK: usize = 4096;
    for _block in 0..10_000 {
        let (mut b1, mut be) = (0.0, 0.0);
        for _ in 0..BLOCK {
            let mut y = x;
            x = 0.0;
            if let Some(p) = &mut pink {
                y = p.process(y);
            }
            for b in &mut band {
                y = b.process(y);
            }
            b1 += y.abs();
            be += y * y;
        }
        l1 += b1;
        e += be;
        if be <= 1e-18 * e && b1 <= 1e-12 * l1 {
            break;
        }
    }
    (l1, e.sqrt())
}

/// One period of pink noise built in the frequency domain: magnitude 1/√max(f, corner)
/// times the band-limit response, uniform random phase, no DC or Nyquist. Returns the
/// period scaled to unit RMS and its exact crest factor.
fn periodic_pink_table(
    period: usize,
    fs: f64,
    seed: u64,
    band: &BandLimit,
) -> Result<(Vec<f32>, f64), GeneratorError> {
    if !period.is_power_of_two() || !(MIN_PERIOD..=MAX_PERIOD).contains(&period) {
        return Err(GeneratorError::InvalidPeriod);
    }
    // Filters applied as their steady-state response at each bin, which is exactly what a
    // periodic signal sees and keeps the period exact.
    let filters = band_filters(band, fs)?;
    let mut rng = Rng::new(seed);
    let mut planner = RealFftPlanner::<f64>::new();
    let inv = planner.plan_fft_inverse(period);
    let mut spec = inv.make_input_vec();
    let bin_hz = fs / period as f64;
    for (k, s) in spec.iter_mut().enumerate() {
        if k == 0 || k == period / 2 {
            *s = Complex::new(0.0, 0.0);
            continue;
        }
        let f = k as f64 * bin_hz;
        let mut h = Complex::from_polar(1.0 / f.max(PINK_CORNER_HZ).sqrt(), TAU * rng.unit());
        for b in &filters {
            h *= b.response(f, fs);
        }
        *s = h;
    }
    let mut x = inv.make_output_vec();
    let _ = inv.process(&mut spec, &mut x);
    let rms = (x.iter().map(|v| v * v).sum::<f64>() / period as f64).sqrt();
    let table: Vec<f32> = x.iter().map(|v| (v / rms) as f32).collect();
    // Crest of the stored (rounded) table, so the clip check covers what is emitted.
    let rms = (table.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / period as f64).sqrt();
    let peak = table.iter().fold(0.0f64, |m, &v| m.max(f64::from(v).abs()));
    Ok((table, peak / rms))
}

#[cfg(test)]
mod tests;
