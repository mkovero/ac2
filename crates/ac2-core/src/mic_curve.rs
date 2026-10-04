//! Microphone correction curves (PLAN.md §5.7, `docs/design/q7-calibration.md`).
//!
//! A mic curve is the microphone's magnitude response (deviation from flat, dB) at a list
//! of frequencies. ac2 corrects with its negative, normalised to 0 dB at the calibrator
//! frequency, because the sensitivity calibration read the calibrator tone uncorrected:
//! a correction of 0 dB there means nothing is counted twice.
//!
//! Two uses, never mixed:
//! - **Display correction** ([`Correction::db`], [`Correction::band_db`],
//!   [`Correction::subtract`]): TF, spectrum and RTA magnitudes have the normalised curve
//!   subtracted on their own grids. Phase is never touched.
//! - **SPL pre-weighting filter** ([`Correction::design_fir`] + [`PartitionedFir`]): a
//!   minimum-phase FIR with the correction's magnitude, run before A/C/Z weighting.
//!
//! Between points the curve is interpolated linearly in log-frequency (the way such
//! curves are measured and plotted); outside the file's range it is held at the end
//! values.

use std::fmt;
use std::sync::Arc;

use num_complex::{Complex, Complex64};
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

mod info;
pub use info::{CurveFileInfo, file_info};

/// Most points accepted from a curve file.
pub const MAX_POINTS: usize = 10_000;
/// Largest |gain| accepted. Measurement-mic deviations are a few dB; tens of dB means a
/// wrong column or an absolute-SPL file, which must not silently become a correction.
pub const MAX_GAIN_DB: f64 = 40.0;

/// Why a curve file or point list was refused. `line` is the 1-based line of a parsed
/// file, or the 1-based point number for curves built from points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicCurveFileError {
    /// Fewer than two data lines.
    TooFewPoints {
        /// Data lines found.
        found: usize,
    },
    /// More than [`MAX_POINTS`] data lines.
    TooManyPoints {
        /// Data lines found.
        found: usize,
    },
    /// The gain field is not a number.
    BadNumber {
        /// Line.
        line: usize,
    },
    /// A frequency without a gain.
    MissingGain {
        /// Line.
        line: usize,
    },
    /// Frequency ≤ 0.
    NonPositiveFrequency {
        /// Line.
        line: usize,
    },
    /// NaN or infinite frequency or gain.
    NonFinite {
        /// Line.
        line: usize,
    },
    /// |gain| > [`MAX_GAIN_DB`].
    GainOutOfRange {
        /// Line.
        line: usize,
    },
    /// Frequency not above the previous one (duplicates included).
    NotAscending {
        /// Line.
        line: usize,
    },
}

impl MicCurveFileError {
    /// The line (or point number) the error refers to.
    pub fn line(&self) -> Option<usize> {
        match *self {
            Self::TooFewPoints { .. } | Self::TooManyPoints { .. } => None,
            Self::BadNumber { line }
            | Self::MissingGain { line }
            | Self::NonPositiveFrequency { line }
            | Self::NonFinite { line }
            | Self::GainOutOfRange { line }
            | Self::NotAscending { line } => Some(line),
        }
    }
}

impl fmt::Display for MicCurveFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooFewPoints { found } => {
                write!(f, "{found} data lines; a curve needs at least 2")
            }
            Self::TooManyPoints { found } => {
                write!(f, "{found} data lines; at most {MAX_POINTS} are accepted")
            }
            Self::BadNumber { line } => write!(f, "line {line}: the gain is not a number"),
            Self::MissingGain { line } => write!(f, "line {line}: frequency without a gain"),
            Self::NonPositiveFrequency { line } => {
                write!(f, "line {line}: frequency must be above 0 Hz")
            }
            Self::NonFinite { line } => write!(f, "line {line}: value is not finite"),
            Self::GainOutOfRange { line } => write!(
                f,
                "line {line}: gain beyond ±{MAX_GAIN_DB} dB (wrong column or an absolute SPL file?)"
            ),
            Self::NotAscending { line } => {
                write!(f, "line {line}: frequency is not above the previous line's")
            }
        }
    }
}

impl std::error::Error for MicCurveFileError {}

/// A validated mic curve: strictly ascending positive frequencies, finite bounded gains.
#[derive(Debug, Clone, PartialEq)]
pub struct MicCurve {
    freq: Vec<f64>,
    gain: Vec<f64>,
}

/// Splits a data line into fields. With semicolons the fields are `;`-separated and a
/// comma is a decimal comma; otherwise whitespace and commas both separate.
fn fields(line: &str) -> Vec<String> {
    let clean = |t: &str| t.trim().trim_matches('"').trim().to_owned();
    if line.contains(';') {
        line.split(';')
            .map(|t| clean(t).replace(',', "."))
            .filter(|t| !t.is_empty())
            .collect()
    } else {
        line.split(|c: char| c.is_whitespace() || c == ',')
            .map(clean)
            .filter(|t| !t.is_empty())
            .collect()
    }
}

impl MicCurve {
    /// Parses a magnitude file (`.frd`, `.txt`, `.cal`, CSV). Liberal about layout,
    /// strict about values; see `docs/design/q7-calibration.md` §4.
    pub fn parse(bytes: &[u8]) -> Result<Self, MicCurveFileError> {
        let text = String::from_utf8_lossy(bytes);
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut pts = Vec::new();
        let mut lines = Vec::new();
        for (i, line) in text.split('\n').enumerate() {
            let line_no = i + 1;
            let f = fields(line);
            let Some(Ok(freq)) = f.first().map(|t| t.parse::<f64>()) else {
                // Headers, comments, vendor text: anything not starting with a number.
                continue;
            };
            let gain = match f.get(1) {
                None => return Err(MicCurveFileError::MissingGain { line: line_no }),
                Some(t) => t
                    .parse::<f64>()
                    .map_err(|_| MicCurveFileError::BadNumber { line: line_no })?,
            };
            pts.push((freq, gain));
            lines.push(line_no);
            if pts.len() > MAX_POINTS {
                // Count the rest only for the message.
                let rest = text
                    .split('\n')
                    .skip(line_no)
                    .filter(|l| matches!(fields(l).first().map(|t| t.parse::<f64>()), Some(Ok(_))))
                    .count();
                return Err(MicCurveFileError::TooManyPoints {
                    found: pts.len() + rest,
                });
            }
        }
        Self::validate(&pts, &lines)
    }

    /// A curve from (frequency Hz, gain dB) points, validated like a file.
    pub fn from_points(points: &[(f64, f64)]) -> Result<Self, MicCurveFileError> {
        let numbers: Vec<usize> = (1..=points.len()).collect();
        Self::validate(points, &numbers)
    }

    fn validate(points: &[(f64, f64)], lines: &[usize]) -> Result<Self, MicCurveFileError> {
        if points.len() < 2 {
            return Err(MicCurveFileError::TooFewPoints {
                found: points.len(),
            });
        }
        if points.len() > MAX_POINTS {
            return Err(MicCurveFileError::TooManyPoints {
                found: points.len(),
            });
        }
        let mut prev = 0.0f64;
        for (&(f, g), &line) in points.iter().zip(lines) {
            if !f.is_finite() || !g.is_finite() {
                return Err(MicCurveFileError::NonFinite { line });
            }
            if f <= 0.0 {
                return Err(MicCurveFileError::NonPositiveFrequency { line });
            }
            if g.abs() > MAX_GAIN_DB {
                return Err(MicCurveFileError::GainOutOfRange { line });
            }
            if f <= prev {
                return Err(MicCurveFileError::NotAscending { line });
            }
            prev = f;
        }
        Ok(Self {
            freq: points.iter().map(|p| p.0).collect(),
            gain: points.iter().map(|p| p.1).collect(),
        })
    }

    /// Number of points.
    pub fn len(&self) -> usize {
        self.freq.len()
    }

    /// Never true for a validated curve.
    pub fn is_empty(&self) -> bool {
        self.freq.is_empty()
    }

    /// Point frequencies, Hz, ascending.
    pub fn freqs(&self) -> &[f64] {
        &self.freq
    }

    /// Point gains, dB.
    pub fn gains(&self) -> &[f64] {
        &self.gain
    }

    /// Lowest point frequency.
    pub fn f_lo(&self) -> f64 {
        self.freq[0]
    }

    /// Highest point frequency.
    pub fn f_hi(&self) -> f64 {
        self.freq[self.freq.len() - 1]
    }

    /// The mic's response at `f` (dB): log-frequency linear between points, held flat
    /// outside them (also at and below 0 Hz).
    pub fn gain_db(&self, f: f64) -> f64 {
        let n = self.freq.len();
        if f.is_nan() || f <= self.freq[0] {
            return self.gain[0];
        }
        if f >= self.freq[n - 1] {
            return self.gain[n - 1];
        }
        let i = self.freq.partition_point(|&x| x <= f) - 1;
        let (f0, f1) = (self.freq[i], self.freq[i + 1]);
        let t = (f / f0).ln() / (f1 / f0).ln();
        self.gain[i] + t * (self.gain[i + 1] - self.gain[i])
    }

    /// The correction normalised to 0 dB at `f_norm` (the calibrator frequency).
    pub fn normalised(&self, f_norm: f64) -> Correction {
        Correction {
            ref_db: self.gain_db(f_norm),
            f_norm,
            curve: self.clone(),
        }
    }
}

/// A mic curve normalised at the calibrator frequency: the amount subtracted from a
/// displayed magnitude, and the SPL correction filter.
#[derive(Debug, Clone, PartialEq)]
pub struct Correction {
    curve: MicCurve,
    f_norm: f64,
    ref_db: f64,
}

/// Samples per band in [`Correction::band_db`]: the curve is piecewise linear in log f,
/// so 64 midpoints per band leave a quadrature error far below 0.001 dB.
const BAND_SAMPLES: usize = 64;
/// Design grid oversampling of the FIR length (cepstral aliasing control).
const DESIGN_OVERSAMPLING: usize = 8;
/// FIR length for rate `fs`: the power of two ≥ 340 ms (≈ 3 Hz design resolution, which
/// keeps a 2nd-order roll-off at 30 Hz within 0.01 dB from 31.5 Hz up).
pub fn fir_len(fs: f64) -> usize {
    ((0.34 * fs).ceil() as usize)
        .next_power_of_two()
        .max(fir_partition(fs))
}

/// Partition (and latency) of the run-time convolution: the power of two ≥ 5.3 ms, so
/// the cost per sample stays the same at every rate (64 partitions of a [`fir_len`] FIR).
pub fn fir_partition(fs: f64) -> usize {
    ((0.0053 * fs).round() as usize).next_power_of_two().max(64)
}

impl Correction {
    /// Normalisation frequency.
    pub fn f_norm(&self) -> f64 {
        self.f_norm
    }

    /// The underlying curve.
    pub fn curve(&self) -> &MicCurve {
        &self.curve
    }

    /// dB to subtract from a magnitude at `f` (0 at `f_norm`).
    pub fn db(&self, f: f64) -> f64 {
        self.curve.gain_db(f) - self.ref_db
    }

    /// dB to subtract from a band power over `[lo, hi]`: the power average of the
    /// correction in log-frequency, `−10·lg(mean 10^(−cₙ/10))` (exact for pink noise
    /// filling an ideal band).
    pub fn band_db(&self, lo: f64, hi: f64) -> f64 {
        if !(lo > 0.0 && hi > lo) {
            return self.db(lo.max(hi));
        }
        let (a, b) = (lo.ln(), hi.ln());
        let mean = (0..BAND_SAMPLES)
            .map(|i| {
                let f = (a + (b - a) * (i as f64 + 0.5) / BAND_SAMPLES as f64).exp();
                10f64.powf(-self.db(f) / 10.0)
            })
            .sum::<f64>()
            / BAND_SAMPLES as f64;
        -10.0 * mean.log10()
    }

    /// Subtracts the correction from `values` (dB) at `freqs`; NaN stays NaN.
    pub fn subtract(&self, freqs: &[f64], values: &mut [f32]) {
        for (v, &f) in values.iter_mut().zip(freqs) {
            *v -= self.db(f) as f32;
        }
    }

    /// Minimum-phase FIR of [`fir_len`]`(fs)` taps with magnitude `10^(−db(f)/20)`,
    /// exactly 0 dB at `f_norm`. Homomorphic design: ln|H| on a dense grid → real
    /// cepstrum → fold onto causal quefrencies → exp → impulse response; then a half-Hann
    /// taper over the last eighth and a final scale for 0 dB at the calibrator frequency.
    pub fn design_fir(&self, fs: f64) -> Vec<f64> {
        let l = fir_len(fs);
        let n = l * DESIGN_OVERSAMPLING;
        let mut planner = RealFftPlanner::<f64>::new();
        let fwd = planner.plan_fft_forward(n);
        let inv = planner.plan_fft_inverse(n);
        let ln10_20 = std::f64::consts::LN_10 / 20.0;

        let mut spec: Vec<Complex64> = (0..=n / 2)
            .map(|k| {
                let f = k as f64 * fs / n as f64;
                Complex64::new(-self.db(f) * ln10_20, 0.0)
            })
            .collect();
        let mut cep = vec![0.0; n];
        // Inverse of a real even spectrum: the real cepstrum (scaled by n).
        if inv.process(&mut spec, &mut cep).is_err() {
            return unit_impulse(l);
        }
        let scale = 1.0 / n as f64;
        let mut fold = vec![0.0; n];
        fold[0] = cep[0] * scale;
        for i in 1..n / 2 {
            fold[i] = 2.0 * cep[i] * scale;
        }
        fold[n / 2] = cep[n / 2] * scale;
        let mut z = fwd.make_output_vec();
        if fwd.process(&mut fold, &mut z).is_err() {
            return unit_impulse(l);
        }
        let last = z.len() - 1;
        for (k, v) in z.iter_mut().enumerate() {
            *v = v.exp();
            if k == 0 || k == last {
                v.im = 0.0;
            }
        }
        let mut h = vec![0.0; n];
        if inv.process(&mut z, &mut h).is_err() {
            return unit_impulse(l);
        }
        h.truncate(l);
        for v in &mut h {
            *v *= scale;
        }
        let taper = l / 8;
        for i in 0..taper {
            // Half Hann from 1 (start of the taper) towards 0 (last tap).
            let w = 0.5 * (1.0 + (std::f64::consts::PI * (i + 1) as f64 / taper as f64).cos());
            h[l - taper + i] *= w;
        }
        let g = dtft(&h, self.f_norm, fs).norm();
        if g > 0.0 && g.is_finite() {
            for v in &mut h {
                *v /= g;
            }
        }
        h
    }
}

fn unit_impulse(l: usize) -> Vec<f64> {
    let mut h = vec![0.0; l];
    h[0] = 1.0;
    h
}

/// DTFT of `h` at `f` Hz.
pub fn dtft(h: &[f64], f: f64, fs: f64) -> Complex64 {
    let w = -std::f64::consts::TAU * f / fs;
    h.iter()
        .enumerate()
        .map(|(n, &v)| Complex64::from_polar(v, w * n as f64))
        .sum()
}

/// Uniformly partitioned overlap-save FIR convolution: partitions of `partition` samples,
/// FFT size twice that. Output lags input by one partition. Allocates only at
/// construction.
///
/// The convolution runs in f32 with the spectra stored as separate real and imaginary
/// arrays, so the multiply-accumulate over the partitions — nearly all of the work — runs
/// on full SIMD lanes, and the partitioned spectra of a 96 kHz correction (64 × 513 bins
/// × 2 × 4 B ≈ 260 kB, the same again for the delay line) fit a 1 MB L2 cache. The
/// round-off this leaves is about 10⁻⁷ of the signal's RMS (−130 dB or lower), below the
/// quantisation floor of a 24-bit converter, so it cannot move a level the meter shows at
/// 0.1 dB. The output goes back to f64 before the weighting filters, whose poles near
/// z = 1 need the precision.
#[derive(Clone)]
pub struct PartitionedFir {
    b: usize,
    /// Bins per partition spectrum, `b + 1`.
    bins: usize,
    parts: usize,
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
    /// Partition spectra, partition `i` at `i·bins..(i + 1)·bins`, scaled by the inverse
    /// transform's 1 / 2b.
    h_re: Vec<f32>,
    h_im: Vec<f32>,
    /// Spectra of the newest input blocks, same layout; slot `pos` is the newest.
    x_re: Vec<f32>,
    x_im: Vec<f32>,
    pos: usize,
    input: Vec<f32>,
    fill: usize,
    out: Vec<f32>,
    time: Vec<f32>,
    spec: Vec<Complex<f32>>,
    acc_re: Vec<f32>,
    acc_im: Vec<f32>,
    scratch_fwd: Vec<Complex<f32>>,
    scratch_inv: Vec<Complex<f32>>,
}

impl fmt::Debug for PartitionedFir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PartitionedFir")
            .field("partition", &self.b)
            .field("partitions", &self.parts)
            .finish()
    }
}

/// `acc += x · h` over split complex arrays of equal length.
#[inline]
fn complex_mac(
    acc_re: &mut [f32],
    acc_im: &mut [f32],
    xr: &[f32],
    xi: &[f32],
    hr: &[f32],
    hi: &[f32],
) {
    let n = acc_re.len();
    let (acc_im, xr, xi, hr, hi) = (&mut acc_im[..n], &xr[..n], &xi[..n], &hr[..n], &hi[..n]);
    for k in 0..n {
        acc_re[k] += xr[k] * hr[k] - xi[k] * hi[k];
        acc_im[k] += xr[k] * hi[k] + xi[k] * hr[k];
    }
}

impl PartitionedFir {
    /// Convolution with taps `h` (any non-zero length) in partitions of `partition`
    /// samples (see [`fir_partition`]).
    pub fn new(h: &[f64], partition: usize) -> Self {
        let b = partition.max(1);
        let n = 2 * b;
        let bins = b + 1;
        let mut planner = RealFftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(n);
        let inv = planner.plan_fft_inverse(n);
        let parts = h.len().div_ceil(b).max(1);
        let mut scratch_fwd = fwd.make_scratch_vec();
        let mut h_re = vec![0.0; parts * bins];
        let mut h_im = vec![0.0; parts * bins];
        let mut spec = fwd.make_output_vec();
        let mut t = vec![0.0f32; n];
        let scale = 1.0 / n as f64;
        for i in 0..parts {
            t.fill(0.0);
            let src = &h[(i * b).min(h.len())..((i + 1) * b).min(h.len())];
            for (d, &s) in t.iter_mut().zip(src) {
                *d = (s * scale) as f32;
            }
            // Lengths come from the plan; the transform cannot fail.
            let _ = fwd.process_with_scratch(&mut t, &mut spec, &mut scratch_fwd);
            for (k, c) in spec.iter().enumerate() {
                h_re[i * bins + k] = c.re;
                h_im[i * bins + k] = c.im;
            }
        }
        Self {
            b,
            bins,
            parts,
            h_re,
            h_im,
            x_re: vec![0.0; parts * bins],
            x_im: vec![0.0; parts * bins],
            pos: 0,
            input: vec![0.0; n],
            fill: 0,
            out: vec![0.0; b],
            time: t,
            spec,
            acc_re: vec![0.0; bins],
            acc_im: vec![0.0; bins],
            scratch_inv: inv.make_scratch_vec(),
            scratch_fwd,
            fwd,
            inv,
        }
    }

    /// Output delay relative to the input, samples.
    pub fn latency(&self) -> usize {
        self.b
    }

    /// Filters `x` into `y` (same length): `y[n] = (h ∗ x)[n − latency]`.
    pub fn process(&mut self, x: &[f64], y: &mut [f64]) {
        let n = x.len().min(y.len());
        let mut i = 0;
        while i < n {
            let at = self.fill;
            let k = (self.b - at).min(n - i);
            for (d, &s) in self.input[self.b + at..self.b + at + k]
                .iter_mut()
                .zip(&x[i..i + k])
            {
                *d = s as f32;
            }
            for (d, &s) in y[i..i + k].iter_mut().zip(&self.out[at..at + k]) {
                *d = f64::from(s);
            }
            self.fill += k;
            i += k;
            if self.fill == self.b {
                self.block();
                self.fill = 0;
            }
        }
    }

    fn block(&mut self) {
        let (b, bins, p) = (self.b, self.bins, self.parts);
        self.time.copy_from_slice(&self.input);
        let _ =
            self.fwd
                .process_with_scratch(&mut self.time, &mut self.spec, &mut self.scratch_fwd);
        let slot = self.pos * bins;
        for (k, c) in self.spec.iter().enumerate() {
            self.x_re[slot + k] = c.re;
            self.x_im[slot + k] = c.im;
        }
        self.acc_re.fill(0.0);
        self.acc_im.fill(0.0);
        for i in 0..p {
            let s = ((self.pos + p - i) % p) * bins;
            let h = i * bins;
            complex_mac(
                &mut self.acc_re,
                &mut self.acc_im,
                &self.x_re[s..s + bins],
                &self.x_im[s..s + bins],
                &self.h_re[h..h + bins],
                &self.h_im[h..h + bins],
            );
        }
        for ((c, &re), &im) in self.spec.iter_mut().zip(&self.acc_re).zip(&self.acc_im) {
            *c = Complex::new(re, im);
        }
        // The spectrum of a real signal is real at DC and Nyquist.
        self.spec[0].im = 0.0;
        self.spec[bins - 1].im = 0.0;
        let _ =
            self.inv
                .process_with_scratch(&mut self.spec, &mut self.time, &mut self.scratch_inv);
        self.out.copy_from_slice(&self.time[b..]);
        self.input.copy_within(b.., 0);
        self.pos = (self.pos + 1) % p;
    }

    /// Back to silence.
    pub fn reset(&mut self) {
        self.x_re.fill(0.0);
        self.x_im.fill(0.0);
        self.input.fill(0.0);
        self.out.fill(0.0);
        self.fill = 0;
        self.pos = 0;
    }
}

#[cfg(test)]
mod tests;
