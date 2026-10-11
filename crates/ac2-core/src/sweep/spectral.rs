//! Spectral helpers: transforms, band averages, windows and the fine peak.

use std::f64::consts::{PI, TAU};

use num_complex::Complex64;
use realfft::RealFftPlanner;

use super::{FINE_HALF, FINE_UPSAMPLE};

pub(crate) fn fft_forward(x: &[f64], n: usize) -> Vec<Complex64> {
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

pub(super) fn fft_inverse(mut spec: Vec<Complex64>, n: usize) -> Vec<f64> {
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

/// Mean power of `s` (bins of an FFT with `bin_hz` spacing) over `octaves` around `f`, but
/// over at least `min_hz`; the interpolated bin power where the band holds no bin.
pub(super) fn band_power(s: &[f64], bin_hz: f64, f: f64, octaves: f64, min_hz: f64) -> f64 {
    let (lo_hz, hi_hz) = band(f, octaves, min_hz);
    mean_power(s, bin_hz, lo_hz, hi_hz, f)
}

/// The band `octaves` wide around `f`, widened to `min_hz` if narrower, Hz.
pub(super) fn band(f: f64, octaves: f64, min_hz: f64) -> (f64, f64) {
    let half = 2f64.powf(octaves / 2.0);
    let (mut lo_hz, mut hi_hz) = (f / half, f * half);
    if hi_hz - lo_hz < min_hz {
        let c = 0.5 * (lo_hz + hi_hz);
        lo_hz = (c - 0.5 * min_hz).max(0.0);
        hi_hz = c + 0.5 * min_hz;
    }
    (lo_hz, hi_hz)
}

/// Mean of `s` over the bins in `lo_hz..=hi_hz`; with none there, `s` interpolated at `f`.
pub(super) fn mean_power(s: &[f64], bin_hz: f64, lo_hz: f64, hi_hz: f64, f: f64) -> f64 {
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

/// `spec` (bins `bin_hz` apart) through a fourth-order Butterworth high-pass at `fc`, the
/// analog prototype evaluated on each bin (minimum phase, so causal).
///
/// Below the emitted start the reference carries no sweep, and a loopback's own DC-blocking
/// high-pass takes the little the fade-in leaves there down to the regularisation, so the
/// division turns over abruptly a few hertz up. That edge rings for about the inverse of its
/// width, a zero-phase low-frequency swell around the arrival that still holds −40 dB re
/// the fundamental half a second away, where H2's window and the noise windows lie. The
/// harmonic windows are 0.1 s at most with edges of a few tens of milliseconds, so their
/// side lobes carry that swell some 25 dB down into the lowest harmonic bands, where it
/// reads as H2 up to 20 dB above the path's own, or as a floor as high. No analysed
/// harmonic needs content below the lowest analysed fundamental (H2's bands start at twice
/// it), so the harmonic windows read the response without it.
///
/// The filter must be causal: below the analysed band the response is large (the path's own
/// roll-off rings there), and a zero-phase filter spreads what its transition passes evenly
/// before and after the arrival. Every harmonic window lies before the arrival, at −L·ln k,
/// and on a 5.5 s sweep H2's is within the ringing of a transition a few hertz wide; a causal
/// filter rings only after the arrival. Fourth order: a steeper one rings far enough into the
/// post-roll to lift the noise windows.
pub(super) fn high_passed(spec: &[Complex64], bin_hz: f64, fc: f64) -> Vec<Complex64> {
    const ORDER: usize = 4;
    let wc = TAU * fc;
    spec.iter()
        .enumerate()
        .map(|(k, &z)| {
            if k == 0 {
                return Complex64::new(0.0, 0.0);
            }
            let s = Complex64::new(0.0, TAU * k as f64 * bin_hz);
            (0..ORDER / 2).fold(z, |acc, i| {
                // pole pair at angle θ from the imaginary axis: s² + 2·sin θ·ωc·s + ωc²
                let th = PI * (2 * i + 1) as f64 / (2 * ORDER) as f64;
                acc * s * s / (s * s + s * (2.0 * th.sin() * wc) + wc * wc)
            })
        })
        .collect()
}

/// Half-Hann rise over `rise` samples, flat, half-Hann fall over the last `fall` samples.
pub(crate) fn taper(len: usize, rise: usize, fall: usize) -> Vec<f64> {
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
pub(super) fn segment(h: &[f64], start: i64, w: &[f64]) -> Vec<f64> {
    let n = h.len() as i64;
    w.iter()
        .enumerate()
        .map(|(i, wi)| h[(start + i as i64).rem_euclid(n) as usize] * wi)
        .collect()
}

/// Power spectrum averaged over `count` adjacent windows `w` of `h` ending at `end`. The
/// power one window estimates in a resolution cell is exponentially distributed (its spread
/// equals its mean), and 1/f noise leaves only a few cells in a band at low frequencies, so a
/// single window's floor swings by several dB between runs; the mean of `count` independent
/// windows cuts the spread by about √count.
pub(super) fn noise_power(h: &[f64], end: i64, count: usize, w: &[f64], nw: usize) -> Vec<f64> {
    let len = w.len() as i64;
    let mut acc = vec![0.0; nw / 2 + 1];
    for j in 0..count as i64 {
        let p = power_spectrum(&segment(h, end - (j + 1) * len, w), nw);
        acc.iter_mut().zip(&p).for_each(|(a, v)| *a += v);
    }
    acc.iter_mut().for_each(|a| *a /= count.max(1) as f64);
    acc
}

pub(super) fn power_spectrum(x: &[f64], n: usize) -> Vec<f64> {
    fft_forward(x, n).iter().map(Complex64::norm_sqr).collect()
}

pub(super) fn db10(p: f64) -> f64 {
    10.0 * p.log10()
}

/// Fractional offset of the peak of |h| from the whole sample `d`: `h[d−FINE_HALF …
/// d+FINE_HALF]` interpolated band-limited (zero-padded DFT; the sweep stops below Nyquist, so the samples
/// define the peak), then a parabolic vertex on the fine grid.
pub(crate) fn fine_peak(h: impl Fn(i64) -> f64, d: i64) -> f64 {
    let n = 2 * FINE_HALF + 1;
    let mut x: Vec<Complex64> = (0..n)
        .map(|i| Complex64::new(h(d - FINE_HALF as i64 + i as i64), 0.0))
        .collect();
    let mut planner = rustfft::FftPlanner::<f64>::new();
    planner.plan_fft_forward(n).process(&mut x);
    // `n` is odd: bins 0..=n/2 are the positive frequencies, the rest the negative ones, and
    // there is no Nyquist bin to split.
    let big = n * FINE_UPSAMPLE;
    let half = n / 2;
    let mut y = vec![Complex64::new(0.0, 0.0); big];
    y[..=half].copy_from_slice(&x[..=half]);
    y[big - (n - half - 1)..].copy_from_slice(&x[half + 1..]);
    planner.plan_fft_inverse(big).process(&mut y);
    let mag = |j: usize| y[j].re.abs();
    let c = FINE_HALF * FINE_UPSAMPLE;
    let j =
        (c - FINE_UPSAMPLE..=c + FINE_UPSAMPLE).fold(c, |b, j| if mag(j) > mag(b) { j } else { b });
    let (y0, y1, y2) = (mag(j - 1), mag(j), mag(j + 1));
    let den = y0 - 2.0 * y1 + y2;
    let vertex = if den < 0.0 {
        0.5 * (y0 - y2) / den
    } else {
        0.0
    };
    (j as f64 + vertex - c as f64) / FINE_UPSAMPLE as f64
}

/// Mean over `[a, b]` (bin units, `b + 1` inside the spectrum) of the piecewise-linear
/// interpolant through the bins `v(k)`; its value at the centre when the span is empty.
///
/// A column's phase is that of the spectrum averaged over its band, so the average has to be
/// centred on the column. Summing only the whole bins inside the edges centres it on those
/// bins instead, up to half a bin off, and a path with group delay τ then reads its phase
/// wrong by 360°·τ·Δf: with the default gate (≈ 0.9 s, bins ≈ 0.37 Hz apart) and a 1 ms
/// high-pass delay at 20 Hz that is up to ≈ 0.07°, and since the 48-per-octave columns beat
/// against the bin grid it is a ripple about 1 Hz long that the neighbour difference turns
/// into a group delay swinging ±60 %. The interpolant's integral over the exact edges is
/// centred on the column whatever the bins, and a column narrower than a bin reads the
/// interpolated value at its centre.
pub(crate) fn column_mean<T>(v: impl Fn(usize) -> T, a: f64, b: f64) -> T
where
    T: Copy + std::ops::Add<Output = T> + std::ops::Mul<f64, Output = T>,
{
    let lerp = |k: usize, x: f64| v(k) * (1.0 - (x - k as f64)) + v(k + 1) * (x - k as f64);
    if b - a <= 0.0 {
        let x = 0.5 * (a + b);
        return lerp(x.floor() as usize, x);
    }
    let mut x = a;
    let k0 = a.floor() as usize;
    let mut acc = lerp(k0, x) * 0.0;
    while x < b {
        let k = x.floor() as usize;
        let x1 = (k as f64 + 1.0).min(b);
        // The interpolant is linear on [k, k+1]: the trapezoid is its exact integral.
        acc = acc + (lerp(k, x) + lerp(k, x1)) * (0.5 * (x1 - x));
        x = x1;
    }
    acc * (1.0 / (b - a))
}

pub(crate) fn wrap_deg(d: f64) -> f64 {
    let w = (d + 180.0).rem_euclid(360.0) - 180.0;
    if w == -180.0 { 180.0 } else { w }
}
