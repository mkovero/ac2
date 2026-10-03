//! Swept-sine (Farina) analysis: the linear response and harmonic distortion of a system
//! from synchronised exponential sweeps recorded on a reference (loopback) and a measurement
//! input. Design: `docs/design/sweep-distortion.md`.
//!
//! # Deconvolution
//!
//! Each repeat is deconvolved by regularised spectral division of the measurement record by
//! the reference record of the same capture, `H = M·R* / (|R|² + ε)`. Dividing by the
//! measured reference cancels every latency and the converters' own response, so t = 0 is
//! "when the loopback heard it" (the transfer function's time base). The reference is a clean
//! sweep, so the division separates harmonics exactly as Farina's inverse filter does: with
//! `f1·L` an integer, `sin(k·φ(t)) = sin φ(t + L·ln k)`, i.e. the k-th harmonic of the sweep is
//! the sweep advanced by `L·ln k`, and its impulse response appears at `−L·ln k`. Repeats are
//! averaged as complex spectra.
//!
//! # Windows
//!
//! Orders `k` and `k+1` lie `L·ln((k+1)/k)` apart; the gap shrinks with `k`, so the highest
//! order analysed sets one window (10 % of the gap above it before `t_k`, 90 % of the gap
//! below it after) used for every order, for the fundamental's distortion reference and for
//! the noise estimate. Equal windows give equal noise and time resolution, so ratios compare
//! like with like.
//!
//! # Distortion and validity
//!
//! Power is averaged over 1/24 octave ([`DISTORTION_BAND_OCT`]). Harmonic `k` at fundamental
//! `f` is `P_k(k·f) / P_1(f)`; THD is the power sum of the orders in band over `P_1(f)`. The
//! noise floor is the same window cut from the deconvolved silence after the response; a
//! point counts when it is [`FLOOR_MARGIN_DB`] above that floor ([`is_valid`]).

use std::f64::consts::{LN_2, PI, TAU};

use num_complex::Complex64;
use realfft::RealFftPlanner;

use crate::generator::{EssConfig, EssInverse, EssPlan, GeneratorError, dbfs_to_rms};
use crate::grid::LogGrid;
use crate::ir_view::ImpulseResponse;

/// Highest harmonic order analysed by default (H2 … H5).
pub const DEFAULT_MAX_ORDER: u8 = 5;
/// Highest harmonic order the analysis accepts.
pub const MAX_ORDER: u8 = 9;
/// Fraction of the gap to the next-higher order a harmonic window starts before `t_k`.
pub const PRE_FRACTION: f64 = 0.1;
/// A distortion point is valid when it is this far above the noise in its window: the noise
/// then adds at most 1.25 dB (10·log10(4/3)).
pub const FLOOR_MARGIN_DB: f64 = 6.0;
/// Width of the power average around each frequency of a distortion curve, octaves.
pub const DISTORTION_BAND_OCT: f64 = 1.0 / 24.0;
/// The power average spans at least this many resolution cells (1/window) of the
/// harmonic window: one cell of noise is exponentially distributed and exceeds four times
/// its mean 2 % of the time, three cells averaged almost never.
pub const DISTORTION_MIN_CELLS: f64 = 3.0;
/// The noise floor is averaged over this many octaves (and at least twice the cells of a
/// distortion point): noise is smooth in frequency, and a steady floor keeps one noisy
/// estimate from being compared with another.
pub const FLOOR_BAND_OCT: f64 = 1.0 / 3.0;
/// Longest harmonic window, seconds: it still resolves fundamentals down to 2/W = 20 Hz.
pub const MAX_WINDOW_S: f64 = 0.1;
/// Audio kept before each sweep's onset when the repeats are cut apart, seconds.
pub const PRE_ROLL_S: f64 = 0.1;
/// Shortest silence after each sweep, seconds.
pub const MIN_POST_ROLL_S: f64 = 1.0;
/// Harmonics are analysed up to this fraction of the sample rate (anti-alias filters roll
/// off above it).
pub const HARMONIC_FS_FRACTION: f64 = 0.45;
/// A reference whose sweep is weaker than this re the emitted level is refused, dB.
pub const MIN_REFERENCE_DB: f64 = -40.0;
/// Most points of the stored IR; longer spans are decimated peak-preserving.
pub const IR_POINTS: usize = 16_384;
/// Regularisation of the spectral division, relative to the reference's peak bin power.
const REGULARISATION: f64 = 1e-6;
/// Earliest arrival searched, seconds before the reference.
const EARLIEST_ARRIVAL_S: f64 = 0.02;
/// The noise window ends this long before the end of the post-roll.
const NOISE_MARGIN_S: f64 = 0.01;
/// Samples at or above this magnitude count as clipped.
const CLIP: f64 = 0.999;

/// What was played and how to analyse it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SweepSpec {
    /// The sweep as emitted.
    pub ess: EssConfig,
    /// Emitted level, dBFS RMS of the constant-envelope part.
    pub level_dbfs: f64,
    /// Sample rate, Hz.
    pub sample_rate: f64,
    /// Highest harmonic order analysed (2 … [`MAX_ORDER`]).
    pub max_order: u8,
    /// Linear-response gate after the arrival, seconds; `None` = up to the noise window.
    pub gate_s: Option<f64>,
    /// Grid of the reported curves.
    pub grid: LogGrid,
}

/// Why an analysis was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum SweepError {
    /// The sweep parameters are invalid.
    Sweep(GeneratorError),
    /// `max_order` outside 2 … [`MAX_ORDER`].
    BadOrder,
    /// The gate is not positive and finite.
    BadGate,
    /// No repeat, or the two inputs differ in length.
    BadRecording,
    /// The reference carries no sweep (or a much weaker one than emitted).
    NoReference {
        /// Strongest sweep found re the emitted level, dB.
        level_db: f64,
    },
    /// Fewer sweeps than repeats were found, or the recording stops before a sweep's
    /// post-roll ends.
    Incomplete {
        /// Sweeps found whole.
        found: usize,
        /// Sweeps expected.
        expected: usize,
    },
    /// The measurement input is silent.
    NoSignal,
}

impl std::fmt::Display for SweepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sweep(e) => write!(f, "sweep: {e}"),
            Self::BadOrder => write!(f, "harmonic order must be 2 … {MAX_ORDER}"),
            Self::BadGate => write!(f, "the gate must be positive"),
            Self::BadRecording => write!(f, "the recording is empty or its inputs differ"),
            Self::NoReference { level_db } => write!(
                f,
                "the reference input carries no sweep (strongest {level_db:.1} dB re emitted): \
                 check the loopback cable and that the reference output plays the sweep"
            ),
            Self::Incomplete { found, expected } => write!(
                f,
                "found {found} of {expected} sweeps with their silence after them"
            ),
            Self::NoSignal => write!(f, "the measurement input is silent"),
        }
    }
}

impl std::error::Error for SweepError {}

impl From<GeneratorError> for SweepError {
    fn from(e: GeneratorError) -> Self {
        Self::Sweep(e)
    }
}

/// Timing derived from a [`SweepSpec`]: the synchronised sweep, its windows and the silence
/// each repeat needs after it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SweepTiming {
    /// The sweep as generated.
    pub plan: EssPlan,
    /// Harmonic window before `t_k`, seconds.
    pub pre_s: f64,
    /// Harmonic window after `t_k`, seconds.
    pub post_s: f64,
    /// Silence after each sweep, seconds.
    pub post_roll_s: f64,
}

impl SweepTiming {
    /// Validates `spec` and derives the timing.
    pub fn new(spec: &SweepSpec) -> Result<Self, SweepError> {
        if !(2..=MAX_ORDER).contains(&spec.max_order) {
            return Err(SweepError::BadOrder);
        }
        if spec.gate_s.is_some_and(|g| !(g.is_finite() && g > 0.0)) {
            return Err(SweepError::BadGate);
        }
        if !spec.level_dbfs.is_finite() {
            return Err(SweepError::Sweep(GeneratorError::NonFiniteLevel));
        }
        let plan = EssPlan::new(&spec.ess, spec.sample_rate)?;
        let k = f64::from(spec.max_order);
        let l = plan.rate_s;
        let pre_s = PRE_FRACTION * l * ((k + 1.0) / k).ln();
        let post_s = (1.0 - PRE_FRACTION) * l * (k / (k - 1.0)).ln();
        // The noise in a window grows with its length while the sweep's energy per hertz
        // grows with L: capping the window lets a longer sweep lower the floor.
        let shrink = (MAX_WINDOW_S / (pre_s + post_s)).min(1.0);
        let (pre_s, post_s) = (pre_s * shrink, post_s * shrink);
        let post_roll_s = MIN_POST_ROLL_S.max(4.0 * (pre_s + post_s));
        Ok(Self {
            plan,
            pre_s,
            post_s,
            post_roll_s,
        })
    }

    /// Sweep length, samples.
    pub fn sweep_samples(&self) -> usize {
        self.plan.len
    }

    /// Silence after each sweep, samples.
    pub fn post_roll_samples(&self, fs: f64) -> usize {
        (self.post_roll_s * fs).ceil() as usize
    }

    /// One repeat (sweep + silence), samples.
    pub fn period_samples(&self, fs: f64) -> usize {
        self.sweep_samples() + self.post_roll_samples(fs)
    }

    /// Length of the harmonic window, seconds.
    pub fn window_s(&self) -> f64 {
        self.pre_s + self.post_s
    }

    /// Time of the k-th harmonic's impulse response re the linear one, seconds (negative).
    pub fn harmonic_time_s(&self, k: u8) -> f64 {
        -self.plan.rate_s * f64::from(k).ln()
    }
}

/// One harmonic order's distortion curve.
#[derive(Debug, Clone, PartialEq)]
pub struct HarmonicCurve {
    /// Order (2 = second harmonic).
    pub order: u8,
    /// Level re the fundamental at each grid column's fundamental frequency, dB; NaN where
    /// this order is not measured (outside its band).
    pub level_db: Vec<f64>,
    /// Noise in the same window, dB re the fundamental; NaN where not measured.
    pub floor_db: Vec<f64>,
}

/// Result of [`analyse_recording`].
#[derive(Debug, Clone, PartialEq)]
pub struct SweepAnalysis {
    /// Sample rate, Hz.
    pub sample_rate: f64,
    /// Rate constant L of the sweep, s.
    pub rate_s: f64,
    /// Actual sweep duration, s.
    pub duration_s: f64,
    /// Repeats averaged.
    pub repeats: usize,
    /// Arrival of the linear response re the reference, s.
    pub arrival_s: f64,
    /// The reference's sweep re the emitted level (loopback gain), dB.
    pub reference_db: f64,
    /// Onset of the first sweep in the recording, s (output → reference latency plus
    /// whatever preceded the sweep in the recording).
    pub first_onset_s: f64,
    /// Harmonic window before and after `t_k`, s.
    pub harmonic_window_s: (f64, f64),
    /// Linear window before and after the arrival, s.
    pub linear_window_s: (f64, f64),
    /// Start of the noise window re the arrival, s.
    pub noise_window_s: f64,
    /// Grid column frequencies (fundamental), Hz.
    pub frequencies: Vec<f64>,
    /// Fundamental response magnitude, dB re the reference; NaN outside the sweep's band.
    pub magnitude_db: Vec<f64>,
    /// Fundamental phase, degrees in (−180, 180], referred to the arrival; NaN outside.
    pub phase_deg: Vec<f64>,
    /// H2 … H`max_order`.
    pub harmonics: Vec<HarmonicCurve>,
    /// Total harmonic distortion (power sum of the orders in band) re the fundamental, dB.
    pub thd_db: Vec<f64>,
    /// Power sum of the orders' noise floors re the fundamental, dB.
    pub thd_floor_db: Vec<f64>,
    /// Impulse response from the highest order's window to the end of the linear window,
    /// decimated to at most [`IR_POINTS`] (signed extreme per bucket).
    pub ir: Vec<f64>,
    /// Hilbert envelope of the same span, dB (maximum per bucket).
    pub ir_etc_db: Vec<f64>,
    /// Time of `ir[0]` re the arrival, s.
    pub ir_t0_s: f64,
    /// Spacing of `ir`, s.
    pub ir_dt_s: f64,
    /// A sample of either input reached full scale.
    pub clipped: bool,
}

/// Whether a distortion point counts: [`FLOOR_MARGIN_DB`] above its noise floor.
pub fn is_valid(level_db: f64, floor_db: f64) -> bool {
    level_db.is_finite() && (!floor_db.is_finite() || level_db >= floor_db + FLOOR_MARGIN_DB)
}

/// dB re fundamental as percent.
pub fn db_to_percent(db: f64) -> f64 {
    100.0 * 10f64.powf(db / 20.0)
}

fn fft_forward(x: &[f64], n: usize) -> Vec<Complex64> {
    let mut planner = RealFftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(n);
    let mut buf = vec![0.0; n];
    let m = x.len().min(n);
    buf[..m].copy_from_slice(&x[..m]);
    let mut out = fwd.make_output_vec();
    // Lengths match the plan, so the transform cannot fail.
    let _ = fwd.process(&mut buf, &mut out);
    out
}

fn fft_inverse(mut spec: Vec<Complex64>, n: usize) -> Vec<f64> {
    let mut planner = RealFftPlanner::<f64>::new();
    let inv = planner.plan_fft_inverse(n);
    spec[0].im = 0.0;
    if let Some(last) = spec.last_mut() {
        last.im = 0.0;
    }
    let mut out = inv.make_output_vec();
    let _ = inv.process(&mut spec, &mut out);
    let s = 1.0 / n as f64;
    out.iter_mut().for_each(|v| *v *= s);
    out
}

/// Onsets (sample index of the first sweep sample) of `count` sweeps in `reference`, and the
/// strongest sweep's level re the emitted one, dB.
fn locate(
    reference: &[f64],
    spec: &SweepSpec,
    timing: &SweepTiming,
    count: usize,
) -> Result<(Vec<usize>, f64), SweepError> {
    let fs = spec.sample_rate;
    let inv = EssInverse::new(&spec.ess, fs, spec.level_dbfs)?;
    let m = timing.sweep_samples();
    // The matched-filter peak a unit loopback of the emitted sweep produces.
    let amp = dbfs_to_rms(spec.level_dbfs) * std::f64::consts::SQRT_2;
    let ideal: Vec<f64> = (0..m).map(|n| amp * timing.plan.sample(n)).collect();
    let unit_peak = inv
        .deconvolve(&ideal)
        .iter()
        .fold(0.0f64, |a, v| a.max(v.abs()));
    let y = inv.deconvolve(reference);
    let mut mag: Vec<f64> = y.iter().map(|v| v.abs()).collect();
    let period = timing.period_samples(fs);
    let mut peaks = Vec::new();
    for _ in 0..count {
        let (i, v) =
            mag.iter().enumerate().fold(
                (0, 0.0f64),
                |(bi, bv), (i, v)| if *v > bv { (i, *v) } else { (bi, bv) },
            );
        if v.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            break;
        }
        peaks.push((i, v));
        let lo = i.saturating_sub(period / 2);
        let hi = (i + period / 2).min(mag.len());
        mag[lo..hi].iter_mut().for_each(|x| *x = 0.0);
    }
    let strongest = peaks.iter().fold(0.0f64, |a, p| a.max(p.1));
    let level_db = if unit_peak > 0.0 && strongest > 0.0 {
        20.0 * (strongest / unit_peak).log10()
    } else {
        f64::NEG_INFINITY
    };
    if level_db < MIN_REFERENCE_DB {
        return Err(SweepError::NoReference { level_db });
    }
    // A matched-filter peak far below the strongest is a sidelobe or a stray burst, not a
    // repeat of the same sweep.
    let mut onsets: Vec<usize> = peaks
        .iter()
        .filter(|p| p.1 >= strongest * 0.25 && p.0 >= m - 1)
        .map(|p| p.0 + 1 - m)
        .collect();
    onsets.sort_unstable();
    let whole = onsets
        .iter()
        .filter(|&&o| o + period <= reference.len())
        .count();
    if onsets.len() < count || whole < count {
        return Err(SweepError::Incomplete {
            found: whole.min(onsets.len()),
            expected: count,
        });
    }
    Ok((onsets, level_db))
}

/// Mean power of `s` (bins of an FFT with `bin_hz` spacing) over `octaves` around `f`, but
/// over at least `min_hz`; the interpolated bin power where the band holds no bin.
fn band_power(s: &[f64], bin_hz: f64, f: f64, octaves: f64, min_hz: f64) -> f64 {
    let half = 2f64.powf(octaves / 2.0);
    let (mut lo_hz, mut hi_hz) = (f / half, f * half);
    if hi_hz - lo_hz < min_hz {
        let c = 0.5 * (lo_hz + hi_hz);
        lo_hz = (c - 0.5 * min_hz).max(0.0);
        hi_hz = c + 0.5 * min_hz;
    }
    let lo = (lo_hz / bin_hz).ceil() as usize;
    let hi = ((hi_hz / bin_hz).floor() as usize).min(s.len().saturating_sub(1));
    if lo <= hi {
        s[lo..=hi].iter().sum::<f64>() / (hi - lo + 1) as f64
    } else {
        interp(s, f / bin_hz)
    }
}

fn interp(s: &[f64], x: f64) -> f64 {
    let i = x.floor() as usize;
    if i + 1 >= s.len() {
        return s.last().copied().unwrap_or(0.0);
    }
    let t = x - i as f64;
    s[i] * (1.0 - t) + s[i + 1] * t
}

/// Half-Hann rise over `rise` samples, flat, half-Hann fall over the last `fall` samples.
fn taper(len: usize, rise: usize, fall: usize) -> Vec<f64> {
    (0..len)
        .map(|i| {
            let mut w = 1.0;
            if i < rise {
                w *= 0.5 - 0.5 * (PI * (i as f64 + 0.5) / rise as f64).cos();
            }
            let from_end = len - 1 - i;
            if from_end < fall {
                w *= 0.5 - 0.5 * (PI * (from_end as f64 + 0.5) / fall as f64).cos();
            }
            w
        })
        .collect()
}

/// Circular segment of `h` starting at signed index `start`, times `w`.
fn segment(h: &[f64], start: i64, w: &[f64]) -> Vec<f64> {
    let n = h.len() as i64;
    w.iter()
        .enumerate()
        .map(|(i, wi)| h[(start + i as i64).rem_euclid(n) as usize] * wi)
        .collect()
}

fn power_spectrum(x: &[f64], n: usize) -> Vec<f64> {
    fft_forward(x, n).iter().map(Complex64::norm_sqr).collect()
}

fn db10(p: f64) -> f64 {
    10.0 * p.log10()
}

/// Analyses a recording of `repeats` sweeps (each followed by its post-roll of silence) on
/// the reference and the measurement input, sample-aligned (one capture clock).
pub fn analyse_recording(
    spec: &SweepSpec,
    reference: &[f64],
    measurement: &[f64],
    repeats: usize,
) -> Result<SweepAnalysis, SweepError> {
    let timing = SweepTiming::new(spec)?;
    if repeats == 0 || reference.len() != measurement.len() || reference.is_empty() {
        return Err(SweepError::BadRecording);
    }
    let fs = spec.sample_rate;
    let clipped = reference.iter().chain(measurement).any(|v| v.abs() >= CLIP);
    let (onsets, reference_db) = locate(reference, spec, &timing, repeats)?;
    let m = timing.sweep_samples();
    let pre_roll = (PRE_ROLL_S * fs).round() as usize;
    let post_roll = timing.post_roll_samples(fs);
    let cuts: Vec<(usize, usize)> = onsets
        .iter()
        .map(|&o| (o.saturating_sub(pre_roll), o + m + post_roll))
        .collect();
    let longest = cuts.iter().map(|(a, b)| b - a).max().unwrap_or(1);
    let n = 2 * longest.next_power_of_two();

    // Complex mean of the per-repeat spectral divisions.
    let mut acc = vec![Complex64::new(0.0, 0.0); n / 2 + 1];
    for &(a, b) in &cuts {
        let r = fft_forward(&reference[a..b], n);
        let mm = fft_forward(&measurement[a..b], n);
        let peak = r.iter().fold(0.0f64, |p, z| p.max(z.norm_sqr()));
        let eps = REGULARISATION * peak;
        for ((h, rz), mz) in acc.iter_mut().zip(&r).zip(&mm) {
            *h += mz * rz.conj() / (rz.norm_sqr() + eps);
        }
    }
    let scale = 1.0 / cuts.len() as f64;
    acc.iter_mut().for_each(|z| *z *= scale);
    let h = fft_inverse(acc, n);

    // Arrival: the strongest sample from slightly before the reference to half the
    // post-roll after it.
    let at = |i: i64| h[i.rem_euclid(n as i64) as usize];
    let earliest = -((EARLIEST_ARRIVAL_S * fs).round() as i64);
    let latest = (post_roll / 2) as i64;
    let (d, peak) = (earliest..latest).fold((0i64, 0.0f64), |(bi, bv), i| {
        let v = at(i).abs();
        if v > bv { (i, v) } else { (bi, bv) }
    });
    if peak.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
        return Err(SweepError::NoSignal);
    }

    let l = timing.plan.rate_s;
    let k_max = spec.max_order;
    let pre_n = ((timing.pre_s * fs).round() as usize).max(1);
    let post_n = ((timing.post_s * fs).round() as usize).max(1);
    let w_len = pre_n + post_n;
    let w = taper(w_len, pre_n, (post_n / 5).max(1));
    let nw = 4 * w_len.next_power_of_two();
    let bin_w = fs / nw as f64;
    let t_k = |k: u8| d - (l * f64::from(k).ln() * fs).round() as i64;

    // Fundamental (distortion reference) and every order in the same window.
    let spectra: Vec<Vec<f64>> = (1..=k_max)
        .map(|k| power_spectrum(&segment(&h, t_k(k) - pre_n as i64, &w), nw))
        .collect();
    // Noise: the same window ending just before the end of the post-roll, which every
    // repeat's record covers fully.
    let noise_start = ((timing.post_roll_s - NOISE_MARGIN_S) * fs).floor() as i64 - w_len as i64;
    let noise = power_spectrum(&segment(&h, noise_start, &w), nw);

    let freqs = spec.grid.frequencies();
    let (f1, f2) = (spec.ess.start_hz, spec.ess.end_hz);
    let f_lo = (f1 * (spec.ess.fade_in_s / l).exp()).max(2.0 / timing.window_s());
    let f_top = (f2 * (-spec.ess.fade_out_s / l).exp()).min(HARMONIC_FS_FRACTION * fs);
    let mut harmonics: Vec<HarmonicCurve> = (2..=k_max)
        .map(|order| HarmonicCurve {
            order,
            level_db: vec![f64::NAN; freqs.len()],
            floor_db: vec![f64::NAN; freqs.len()],
        })
        .collect();
    let min_hz = DISTORTION_MIN_CELLS / timing.window_s();
    let mut thd_db = vec![f64::NAN; freqs.len()];
    let mut thd_floor_db = vec![f64::NAN; freqs.len()];
    for (i, &f) in freqs.iter().enumerate() {
        if f < f_lo {
            continue;
        }
        let p1 = band_power(&spectra[0], bin_w, f, DISTORTION_BAND_OCT, min_hz);
        if p1.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            continue;
        }
        let (mut sum, mut floor_sum, mut any) = (0.0, 0.0, false);
        for hc in &mut harmonics {
            let kf = f64::from(hc.order) * f;
            if kf > f_top {
                continue;
            }
            let pk = band_power(
                &spectra[usize::from(hc.order) - 1],
                bin_w,
                kf,
                DISTORTION_BAND_OCT,
                min_hz,
            );
            let nk = band_power(&noise, bin_w, kf, FLOOR_BAND_OCT, 2.0 * min_hz);
            hc.level_db[i] = db10(pk / p1);
            hc.floor_db[i] = db10(nk / p1);
            sum += pk;
            floor_sum += nk;
            any = true;
        }
        if any {
            thd_db[i] = db10(sum / p1);
            thd_floor_db[i] = db10(floor_sum / p1);
        }
    }

    // Linear response: from just after H2's window to the gate (default: the noise window).
    let lin_pre = ((PRE_FRACTION * l * LN_2 * fs).round() as usize).max(1);
    let lin_end = match spec.gate_s {
        Some(g) => (d + (g * fs).round() as i64).min(noise_start),
        None => noise_start,
    };
    let lin_end = lin_end.max(d + 2);
    let lin_post = (lin_end - d) as usize;
    let lin_len = lin_pre + lin_post;
    let lw = taper(lin_len, lin_pre, (lin_post / 10).max(1));
    let nl = 2 * lin_len.next_power_of_two();
    let s1 = fft_forward(&segment(&h, d - lin_pre as i64, &lw), nl);
    let bin_l = fs / nl as f64;
    // Refer the phase to the arrival: the segment starts `lin_pre` samples before it.
    let rot = |k: usize| {
        let z = s1[k];
        z * Complex64::from_polar(1.0, TAU * k as f64 * lin_pre as f64 / nl as f64)
    };
    let mut magnitude_db = vec![f64::NAN; freqs.len()];
    let mut phase_deg = vec![f64::NAN; freqs.len()];
    for (i, &f) in freqs.iter().enumerate() {
        if f < f1 || f > f2 {
            continue;
        }
        let (e_lo, e_hi) = spec.grid.edges(i);
        let lo = (e_lo / bin_l).ceil() as usize;
        let hi = ((e_hi / bin_l).floor() as usize).min(s1.len() - 1);
        let (p, z) = if lo <= hi {
            let p = (lo..=hi).map(|k| s1[k].norm_sqr()).sum::<f64>() / (hi - lo + 1) as f64;
            let z: Complex64 = (lo..=hi).map(rot).sum();
            (p, z)
        } else {
            let x = f / bin_l;
            let k = x.floor() as usize;
            if k + 1 >= s1.len() {
                continue;
            }
            let t = x - k as f64;
            let z = rot(k) * (1.0 - t) + rot(k + 1) * t;
            (z.norm_sqr(), z)
        };
        if p > 0.0 {
            magnitude_db[i] = db10(p);
            phase_deg[i] = wrap_deg(z.arg().to_degrees());
        }
    }

    // IR from the highest order's window to the end of the linear window.
    let ir_start = t_k(k_max) - pre_n as i64;
    let span: Vec<f64> = (ir_start..lin_end).map(at).collect();
    let env = ImpulseResponse {
        samples: span.clone(),
        sample_rate: fs,
        inserted_delay_s: 0.0,
    }
    .envelope();
    let bucket = span.len().div_ceil(IR_POINTS).max(1);
    let ir: Vec<f64> = span
        .chunks(bucket)
        .map(|c| {
            c.iter()
                .fold(0.0f64, |a, &v| if v.abs() > a.abs() { v } else { a })
        })
        .collect();
    let ir_etc_db: Vec<f64> = env
        .chunks(bucket)
        .map(|c| {
            let m = c.iter().fold(0.0f64, |a, &v| a.max(v));
            if m > 0.0 {
                (20.0 * m.log10()).max(crate::ir_view::FLOOR_DB)
            } else {
                crate::ir_view::FLOOR_DB
            }
        })
        .collect();

    Ok(SweepAnalysis {
        sample_rate: fs,
        rate_s: l,
        duration_s: timing.plan.duration_s(),
        repeats: cuts.len(),
        arrival_s: d as f64 / fs,
        reference_db,
        first_onset_s: onsets[0] as f64 / fs,
        harmonic_window_s: (pre_n as f64 / fs, post_n as f64 / fs),
        linear_window_s: (lin_pre as f64 / fs, lin_post as f64 / fs),
        noise_window_s: (noise_start - d) as f64 / fs,
        frequencies: freqs,
        magnitude_db,
        phase_deg,
        harmonics,
        thd_db,
        thd_floor_db,
        ir,
        ir_etc_db,
        ir_t0_s: (ir_start - d) as f64 / fs,
        ir_dt_s: bucket as f64 / fs,
        clipped,
    })
}

fn wrap_deg(d: f64) -> f64 {
    let w = (d + 180.0).rem_euclid(360.0) - 180.0;
    if w == -180.0 { 180.0 } else { w }
}

#[cfg(test)]
mod tests;
