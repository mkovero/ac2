//! Filters for the noise sources: band-limiting biquads and the pink-noise filter.

use std::f64::consts::{FRAC_1_SQRT_2, SQRT_2, TAU};

use realfft::num_complex::Complex;

use super::{
    BandLimit, FilterOrder, GeneratorError, PINK_CORNER_HZ, PINK_SPEC_BAND_HZ, check_freq,
};

// ---------------------------------------------------------------------------------------
// Filters

/// Second-order section, transposed direct form II, f64 state (low corners at high rates
/// need the precision).
#[derive(Debug, Clone, Copy)]
pub(super) struct Biquad {
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
    pub(super) fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.s[0];
        self.s[0] = self.b[1] * x - self.a[0] * y + self.s[1];
        self.s[1] = self.b[2] * x - self.a[1] * y;
        y
    }

    pub(super) fn response(&self, f: f64, fs: f64) -> Complex<f64> {
        let z1 = Complex::from_polar(1.0, -TAU * f / fs);
        let z2 = z1 * z1;
        let num = self.b[0] + self.b[1] * z1 + self.b[2] * z2;
        let den = 1.0 + self.a[0] * z1 + self.a[1] * z2;
        num / den
    }
}

pub(super) fn band_filters(band: &BandLimit, fs: f64) -> Result<Vec<Biquad>, GeneratorError> {
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
/// ideal slope over the specified band; the fit lands well inside
/// [`PINK_FLATNESS_DB`](super::PINK_FLATNESS_DB).
#[derive(Debug, Clone)]
pub(super) struct PinkFilter {
    sections: Vec<PoleZero>,
}

impl PinkFilter {
    const REFIT: usize = 2;

    pub(super) fn design(fs: f64) -> Self {
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

    pub(super) fn fit_band(fs: f64) -> (f64, f64) {
        (PINK_SPEC_BAND_HZ.0, PINK_SPEC_BAND_HZ.1.min(0.4535 * fs))
    }

    #[inline]
    pub(super) fn process(&mut self, mut x: f64) -> f64 {
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
    pub(super) fn response(&self, f: f64, fs: f64) -> Complex<f64> {
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
