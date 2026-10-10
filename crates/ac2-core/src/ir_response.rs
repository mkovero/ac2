//! The frequency response of an impulse response given as samples (an imported IR file).
//!
//! An IR of `N` samples at `fs` is a finite sequence, so its DTFT is known exactly at every
//! frequency; the FFT zero-padded to at least twice its length samples that DTFT at bins
//! `fs / n` apart, and each log-grid column reads the mean over its edges of the
//! piecewise-linear interpolant through the bins (power for the magnitude, the complex value
//! for the phase), as a sweep's linear response does ([`crate::sweep`]). Zero padding adds
//! points, not resolution: the window the response is cut with sets the resolution cell,
//! `fs / L` for `L` samples, and columns narrower than that cell are marked unresolved
//! ([`crate::resolution`]).
//!
//! # Window
//!
//! [`IrWindow::Whole`] takes the file as it is (a rectangular window over every sample):
//! what an analyzer exported is often already windowed, and a second window would change
//! it. [`IrWindow::Gate`] cuts `pre` before and `post` after the peak, with a half-Hann rise
//! over `pre` and a half-Hann fall over the last tenth of `post` (the sweep's linear window),
//! to keep a room's reflections out of a speaker's response.
//!
//! # Delay
//!
//! [`Align::Peak`] refers the phase to the peak of `|h|`, located to a fraction of a sample
//! on the band-limited interpolant: the bulk delay (the file's lead-in, the flight time) is
//! removed and the phase shows the response's own shape, as a transfer function's phase
//! does with its measured delay removed. [`Align::Start`] keeps sample 0 as time zero (two
//! IRs exported against one timing reference keep their relative arrival).

use std::f64::consts::TAU;

use num_complex::Complex64;

use crate::grid::LogGrid;
use crate::ir_view::{FLOOR_DB, ImpulseResponse};
use crate::sweep::{column_mean, fft_forward, fine_peak, taper, wrap_deg};

/// Smallest FFT length the response is read from: below it the columns of a short IR sit
/// between few bins and the interpolant between them is coarse.
const MIN_FFT: usize = 1 << 16;

/// The part of the IR the response is computed from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IrWindow {
    /// Every sample, unwindowed.
    Whole,
    /// `pre_s` before and `post_s` after the peak, tapered.
    Gate {
        /// Before the peak, seconds (≥ 0).
        pre_s: f64,
        /// After the peak, seconds (> 0).
        post_s: f64,
    },
}

/// What time zero of the response is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    /// The peak of `|h|` (bulk delay removed).
    Peak,
    /// Sample 0 of the file.
    Start,
}

/// How to analyse an IR.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IrSpec {
    /// Columns of the response.
    pub grid: LogGrid,
    /// Window.
    pub window: IrWindow,
    /// Time zero.
    pub align: Align,
}

/// An IR's frequency response on a log grid.
#[derive(Debug, Clone, PartialEq)]
pub struct IrResponse {
    /// Peak of `|h|`, seconds re sample 0 (fractional).
    pub peak_s: f64,
    /// Time zero of the phase re sample 0: `peak_s` ([`Align::Peak`]) or 0.
    pub delay_s: f64,
    /// Column centres, Hz.
    pub frequencies: Vec<f64>,
    /// Magnitude, dB (20·lg|H|); NaN above the last column the FFT reaches.
    pub magnitude_db: Vec<f64>,
    /// Phase re `delay_s`, degrees, wrapped to ±180; NaN where the magnitude is.
    pub phase_deg: Vec<f64>,
    /// Hilbert envelope of every sample, dB, floored at [`FLOOR_DB`].
    pub etc_db: Vec<f64>,
    /// Window start re sample 0 and its length, seconds.
    pub window_s: (f64, f64),
    /// Resolution cell of the window, Hz.
    pub cell_hz: f64,
    /// Column ranges narrower than the cell (Hz, ascending, disjoint).
    pub unresolved: Vec<(f64, f64)>,
}

/// Why an IR has no response.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IrResponseError {
    /// Fewer than 2 samples.
    TooShort,
    /// The sample rate is not positive and finite.
    BadSampleRate,
    /// A sample is NaN or infinite.
    NonFinite,
    /// Every sample is zero.
    Silent,
    /// The gate is not positive and finite.
    BadGate,
}

impl std::fmt::Display for IrResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TooShort => "an impulse response needs at least 2 samples",
            Self::BadSampleRate => "the sample rate must be positive and finite",
            Self::NonFinite => "a sample is not a finite number",
            Self::Silent => "every sample is zero",
            Self::BadGate => "the gate must be finite, ≥ 0 before the peak and > 0 after it",
        })
    }
}

impl std::error::Error for IrResponseError {}

/// Index of the largest `|h|` (the first of equals).
pub fn peak_index(h: &[f64]) -> usize {
    h.iter()
        .enumerate()
        .fold((0, -1.0f64), |(bi, bv), (i, v)| {
            if v.abs() > bv { (i, v.abs()) } else { (bi, bv) }
        })
        .0
}

/// The response of `h` sampled at `fs`.
pub fn analyse(h: &[f64], fs: f64, spec: &IrSpec) -> Result<IrResponse, IrResponseError> {
    if h.len() < 2 {
        return Err(IrResponseError::TooShort);
    }
    if !(fs.is_finite() && fs > 0.0) {
        return Err(IrResponseError::BadSampleRate);
    }
    if h.iter().any(|v| !v.is_finite()) {
        return Err(IrResponseError::NonFinite);
    }
    if h.iter().all(|v| *v == 0.0) {
        return Err(IrResponseError::Silent);
    }
    let d = peak_index(h);
    let at = |i: i64| {
        usize::try_from(i)
            .ok()
            .and_then(|i| h.get(i))
            .copied()
            .unwrap_or(0.0)
    };
    let peak = d as f64 + fine_peak(at, d as i64);

    // The windowed segment: its first sample and its samples.
    let (start, seg): (i64, Vec<f64>) = match spec.window {
        IrWindow::Whole => (0, h.to_vec()),
        IrWindow::Gate { pre_s, post_s } => {
            if !(pre_s.is_finite() && post_s.is_finite() && pre_s >= 0.0 && post_s > 0.0) {
                return Err(IrResponseError::BadGate);
            }
            let pre = (pre_s * fs).round() as usize;
            let post = ((post_s * fs).round() as usize).max(2);
            let w = taper(pre + post, pre, (post / 10).max(1));
            let start = d as i64 - pre as i64;
            let seg = w
                .iter()
                .enumerate()
                .map(|(i, wi)| at(start + i as i64) * wi)
                .collect();
            (start, seg)
        }
    };
    let len = seg.len();
    let n = (2 * len.next_power_of_two()).max(MIN_FFT);
    let s = fft_forward(&seg, n);
    let bin = fs / n as f64;
    let delay = match spec.align {
        Align::Peak => peak,
        Align::Start => 0.0,
    };
    // Bin k of the segment carries e^{−j2πk·start/n} for a sample at `start`; time zero at
    // `delay` (re sample 0) is `delay − start` samples into the segment.
    let lead = delay - start as f64;
    let rot = |k: usize| s[k] * Complex64::from_polar(1.0, TAU * k as f64 * lead / n as f64);
    let freqs = spec.grid.frequencies();
    let mut magnitude_db = vec![f64::NAN; freqs.len()];
    let mut phase_deg = vec![f64::NAN; freqs.len()];
    for i in 0..freqs.len() {
        let (lo, hi) = spec.grid.edges(i);
        let (a, b) = (lo / bin, hi / bin);
        if b + 1.0 >= s.len() as f64 {
            continue;
        }
        let p = column_mean(|k| s[k].norm_sqr(), a, b);
        let z = column_mean(rot, a, b);
        if p > 0.0 {
            magnitude_db[i] = 10.0 * p.log10();
            phase_deg[i] = wrap_deg(z.arg().to_degrees());
        }
    }
    let cell = fs / len as f64;
    let nyquist = 0.5 * fs;
    let unresolved =
        crate::resolution::unresolved_ranges(&spec.grid, |f| (f <= nyquist).then_some(cell));
    let etc_db = ImpulseResponse {
        samples: h.to_vec(),
        sample_rate: fs,
        inserted_delay_s: 0.0,
    }
    .envelope()
    .into_iter()
    .map(|e| {
        if e > 0.0 {
            (20.0 * e.log10()).max(FLOOR_DB)
        } else {
            FLOOR_DB
        }
    })
    .collect();
    Ok(IrResponse {
        peak_s: peak / fs,
        delay_s: delay / fs,
        frequencies: freqs,
        magnitude_db,
        phase_deg,
        etc_db,
        window_s: (start as f64 / fs, len as f64 / fs),
        cell_hz: cell,
        unresolved,
    })
}

#[cfg(test)]
mod tests;
