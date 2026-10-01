//! IEC 61672 A/C/Z frequency weighting filters (PLAN.md §5.3).
//!
//! The analytic weightings of IEC 61672-1 Annex E are products of real poles: a double pole
//! at f₁ ≈ 20.6 Hz and a double pole at f₄ ≈ 12.2 kHz (both weightings), plus single poles at
//! f₂ ≈ 107.7 Hz and f₃ ≈ 737.9 Hz (A only), with zeros at DC.
//!
//! The digital filters split that product in two:
//!
//! - **Low part** (f₁, f₂, f₃ high-pass corners): bilinear transform. These corners sit far
//!   below Nyquist, where the bilinear frequency warping is negligible, and above them the
//!   sections are flat, so the warping of the upper band does not matter.
//! - **High part** (f₄ double pole): the bilinear transform would put a zero at Nyquist and
//!   read several dB low at 16–20 kHz at 44.1/48 kHz. Instead the poles are placed by the
//!   matched-z mapping z = e^{sT} and a second-order numerator is chosen so that the
//!   magnitude equals the analytic one exactly at DC, 12 kHz and 20 kHz (scaled down for low
//!   rates). The squared magnitude of a second-order FIR is a cosine polynomial in ω, so
//!   matching three points is a linear solve followed by a spectral factorisation that keeps
//!   the zeros inside the unit circle.
//!
//! The cascade is normalised so the digital response is exactly 0 dB at 1 kHz.
//!
//! Measured deviation from Annex E over 10 Hz–20 kHz (see the `weighting` tests, which assert
//! these bounds): 44.1 kHz ≤ 0.26 dB, 48 kHz ≤ 0.13 dB, 96 kHz ≤ 0.002 dB (largest near 17 kHz), all well inside
//! the IEC 61672-1 Table 3 class 1 acceptance limits.

use num_complex::Complex64;
use std::f64::consts::TAU;
use std::fmt;

/// Frequency weighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Weighting {
    /// IEC 61672-1 A weighting.
    A,
    /// IEC 61672-1 C weighting.
    C,
    /// Zero weighting: flat, the filter is a passthrough.
    Z,
}

/// IEC 61672-1 Annex E pole frequencies f₁…f₄ in Hz.
///
/// fr = 1 kHz, fL = 10^1.5 Hz, fH = 10^3.9 Hz, D = √½, fA = 10^2.45 Hz;
/// b = [fr² + fL²fH²/fr² − D(fL² + fH²)] / (1 − D), c = fL²fH²,
/// f₁,₄² = (−b ∓ √(b² − 4c)) / 2, f₂,₃ = fA (3 ∓ √5) / 2.
pub fn iec61672_poles_hz() -> [f64; 4] {
    let fr = 1000.0_f64;
    let fl = 10f64.powf(1.5);
    let fh = 10f64.powf(3.9);
    let d = 0.5f64.sqrt();
    let fa = 10f64.powf(2.45);
    let b = (fr * fr + fl * fl * fh * fh / (fr * fr) - d * (fl * fl + fh * fh)) / (1.0 - d);
    let c = fl * fl * fh * fh;
    let disc = (b * b - 4.0 * c).sqrt();
    let f1 = ((-b - disc) / 2.0).sqrt();
    let f4 = ((-b + disc) / 2.0).sqrt();
    let s5 = 5f64.sqrt();
    [f1, fa * (3.0 - s5) / 2.0, fa * (3.0 + s5) / 2.0, f4]
}

impl Weighting {
    /// Unnormalised Annex E magnitude in dB (before the 1 kHz normalisation constant).
    fn raw_db(self, f: f64) -> f64 {
        let [f1, f2, f3, f4] = iec61672_poles_hz();
        let f2sq = f * f;
        match self {
            Weighting::A => {
                let num = f4 * f4 * f2sq * f2sq;
                let den = (f2sq + f1 * f1)
                    * (f2sq + f2 * f2).sqrt()
                    * (f2sq + f3 * f3).sqrt()
                    * (f2sq + f4 * f4);
                20.0 * (num / den).log10()
            }
            Weighting::C => {
                let num = f4 * f4 * f2sq;
                let den = (f2sq + f1 * f1) * (f2sq + f4 * f4);
                20.0 * (num / den).log10()
            }
            Weighting::Z => 0.0,
        }
    }

    /// Analytic weighting in dB at frequency `f` (IEC 61672-1 Annex E), exactly 0 dB at 1 kHz.
    pub fn analytic_db(self, f: f64) -> f64 {
        self.raw_db(f) - self.raw_db(1000.0)
    }
}

/// Second-order IIR section, transposed direct form II, f64 state.
///
/// Transfer function (b₀ + b₁z⁻¹ + b₂z⁻²) / (1 + a₁z⁻¹ + a₂z⁻²).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Biquad {
    /// Numerator b₀, b₁, b₂.
    pub b: [f64; 3],
    /// Denominator a₁, a₂ (a₀ = 1).
    pub a: [f64; 2],
    s1: f64,
    s2: f64,
}

impl Biquad {
    /// Section with the given coefficients and zero state.
    pub fn new(b: [f64; 3], a: [f64; 2]) -> Self {
        Self {
            b,
            a,
            s1: 0.0,
            s2: 0.0,
        }
    }

    /// Filters one sample.
    #[inline]
    pub fn process_sample(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.s1;
        self.s1 = self.b[1] * x - self.a[0] * y + self.s2;
        self.s2 = self.b[2] * x - self.a[1] * y;
        y
    }

    /// Clears the state.
    pub fn reset(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }

    /// Complex frequency response at `f` Hz for sample rate `fs`.
    pub fn response(&self, f: f64, fs: f64) -> Complex64 {
        let z1 = Complex64::from_polar(1.0, -TAU * f / fs);
        let z2 = z1 * z1;
        (self.b[0] + self.b[1] * z1 + self.b[2] * z2) / (1.0 + self.a[0] * z1 + self.a[1] * z2)
    }
}

/// Weighting filter construction failure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WeightingError {
    /// The rate is too low to represent the weighting's upper corner (f₄ ≈ 12.2 kHz).
    UnsupportedRate {
        /// Requested sample rate in Hz.
        fs: f64,
        /// Lowest supported rate in Hz.
        min_fs: f64,
    },
}

impl fmt::Display for WeightingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WeightingError::UnsupportedRate { fs, min_fs } => write!(
                f,
                "sample rate {fs} Hz is below the {min_fs} Hz needed for A/C weighting"
            ),
        }
    }
}

impl std::error::Error for WeightingError {}

/// Lowest rate for which A/C weighting is designed. Below it f₄ is too close to Nyquist for a
/// second-order section to follow the analytic roll-off.
pub const MIN_WEIGHTING_FS: f64 = 32_000.0;

/// A, C or Z weighting filter at a fixed sample rate.
#[derive(Debug, Clone, PartialEq)]
pub struct WeightingFilter {
    weighting: Weighting,
    fs: f64,
    sections: Vec<Biquad>,
    gain: f64,
}

/// Bilinear first-order high-pass s / (s + ω) without pre-warping, as (b₀, b₁, p) for
/// b₀(1 − z⁻¹) / (1 + p z⁻¹).
fn bilinear_hp1(fc: f64, fs: f64) -> (f64, f64) {
    let w = TAU * fc;
    let k = 2.0 * fs;
    (k / (k + w), (w - k) / (k + w))
}

/// Biquad made of two bilinear first-order high-pass sections at `fa` and `fb`.
fn bilinear_hp2(fa: f64, fb: f64, fs: f64) -> Biquad {
    let (ga, pa) = bilinear_hp1(fa, fs);
    let (gb, pb) = bilinear_hp1(fb, fs);
    let g = ga * gb;
    Biquad::new([g, -2.0 * g, g], [pa + pb, pa * pb])
}

/// Double real pole at `fc` (analytic ωc² / (s + ωc)²): matched-z poles plus a numerator
/// matched in magnitude at DC, `f_mid` and `f_top` (see the module docs).
fn matched_lp2(fc: f64, fs: f64, f_mid: f64, f_top: f64) -> Biquad {
    let w0 = TAU * fc / fs;
    let a1 = -2.0 * (-w0).exp();
    let a2 = (-2.0 * w0).exp();
    // Target |B(e^{jω})|² = |H(f)|²·|A(e^{jω})|² = B0 + B1 cos ω + B2 cos 2ω at three points.
    let row = |f: f64| {
        let w = TAU * f / fs;
        let z1 = Complex64::from_polar(1.0, -w);
        let den = (1.0 + a1 * z1 + a2 * z1 * z1).norm_sqr();
        let h = fc * fc / (f * f + fc * fc);
        ([1.0, w.cos(), (2.0 * w).cos()], h * h * den)
    };
    let rows = [row(0.0), row(f_mid), row(f_top)];
    let [b0c, b1c, b2c] = solve3(
        [rows[0].0, rows[1].0, rows[2].0],
        [rows[0].1, rows[1].1, rows[2].1],
    );
    // B0 + B1 c + B2 (2c² − 1) = 0 in c = cos ω; each root c gives a zero r with
    // (1 − r z⁻¹)(1 − r z) ∝ (c − (1 + r²)/(2r)), i.e. r = c − √(c² − 1), taken inside |z| ≤ 1.
    let roots = quadratic_roots(2.0 * b2c, b1c, b0c - b2c);
    let mut num = [
        Complex64::new(1.0, 0.0),
        Complex64::new(0.0, 0.0),
        Complex64::new(0.0, 0.0),
    ];
    for c in roots.into_iter().flatten() {
        let mut r = c - (c * c - 1.0).sqrt();
        if r.norm() > 1.0 {
            r = 1.0 / r;
        }
        // Multiply num by (1 − r z⁻¹).
        num = [num[0], num[1] - r * num[0], num[2] - r * num[1]];
    }
    let b = [num[0].re, num[1].re, num[2].re];
    // Unity DC gain; the cascade is normalised at 1 kHz afterwards anyway.
    let dc = (1.0 + a1 + a2) / (b[0] + b[1] + b[2]);
    Biquad::new([b[0] * dc, b[1] * dc, b[2] * dc], [a1, a2])
}

/// Roots of a·x² + b·x + c (complex); `None` entries for a degenerate leading coefficient.
fn quadratic_roots(a: f64, b: f64, c: f64) -> [Option<Complex64>; 2] {
    if a.abs() <= f64::EPSILON * (b.abs() + c.abs()) {
        if b == 0.0 {
            return [None, None];
        }
        return [Some(Complex64::new(-c / b, 0.0)), None];
    }
    let disc = Complex64::new(b * b - 4.0 * a * c, 0.0).sqrt();
    // Numerically stable pairing: q = −(b + sign(b)·√disc)/2.
    let q = if b >= 0.0 {
        -(b + disc) / 2.0
    } else {
        -(b - disc) / 2.0
    };
    [Some(q / a), Some(c / q)]
}

/// Solves a 3×3 linear system by Cramer's rule (well conditioned for the points used).
fn solve3(m: [[f64; 3]; 3], y: [f64; 3]) -> [f64; 3] {
    let det = |m: &[[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let d = det(&m);
    let mut out = [0.0; 3];
    for (col, o) in out.iter_mut().enumerate() {
        let mut mc = m;
        for row in 0..3 {
            mc[row][col] = y[row];
        }
        *o = det(&mc) / d;
    }
    out
}

impl WeightingFilter {
    /// Designs the weighting filter for sample rate `fs`.
    ///
    /// Z weighting accepts any positive rate; A and C need `fs ≥` [`MIN_WEIGHTING_FS`].
    pub fn new(weighting: Weighting, fs: f64) -> Result<Self, WeightingError> {
        let mut filter = Self {
            weighting,
            fs,
            sections: Vec::new(),
            gain: 1.0,
        };
        if weighting == Weighting::Z {
            return Ok(filter);
        }
        if fs.is_nan() || fs < MIN_WEIGHTING_FS {
            return Err(WeightingError::UnsupportedRate {
                fs,
                min_fs: MIN_WEIGHTING_FS,
            });
        }
        let [f1, f2, f3, f4] = iec61672_poles_hz();
        let f_top = 20_000.0f64.min(0.45 * fs);
        let f_mid = 12_000.0f64.min(0.6 * f_top);
        filter.sections.push(bilinear_hp2(f1, f1, fs));
        if weighting == Weighting::A {
            filter.sections.push(bilinear_hp2(f2, f3, fs));
        }
        filter.sections.push(matched_lp2(f4, fs, f_mid, f_top));
        filter.gain = 1.0 / filter.response(1000.0).norm();
        Ok(filter)
    }

    /// Which weighting this filter implements.
    pub fn weighting(&self) -> Weighting {
        self.weighting
    }

    /// Sample rate the filter was designed for.
    pub fn fs(&self) -> f64 {
        self.fs
    }

    /// Filters one sample.
    #[inline]
    pub fn process_sample(&mut self, x: f64) -> f64 {
        let mut y = x * self.gain;
        for s in &mut self.sections {
            y = s.process_sample(y);
        }
        y
    }

    /// Filters a block in place.
    pub fn process(&mut self, block: &mut [f64]) {
        for x in block {
            *x = self.process_sample(*x);
        }
    }

    /// Clears the filter state.
    pub fn reset(&mut self) {
        for s in &mut self.sections {
            s.reset();
        }
    }

    /// Complex response of the digital filter at `f` Hz.
    pub fn response(&self, f: f64) -> Complex64 {
        self.sections
            .iter()
            .fold(Complex64::new(self.gain, 0.0), |h, s| {
                h * s.response(f, self.fs)
            })
    }

    /// Magnitude of the digital filter at `f` Hz in dB.
    pub fn response_db(&self, f: f64) -> f64 {
        20.0 * self.response(f).norm().log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_testkit::golden::GoldenSet;

    const RATES: [f64; 3] = [44_100.0, 48_000.0, 96_000.0];

    /// IEC 61672-1:2013 Table 3 class 1 acceptance limits (−, +) at the nominal frequencies
    /// 10 Hz … 20 kHz, in the order of the golden set.
    const CLASS1: [(f64, f64); 34] = [
        (f64::NEG_INFINITY, 3.0),
        (f64::NEG_INFINITY, 2.5),
        (-4.0, 2.0),
        (-2.0, 2.0),
        (-1.5, 2.0),
        (-1.5, 1.5),
        (-1.0, 1.0),
        (-1.0, 1.0),
        (-1.0, 1.0),
        (-1.0, 1.0),
        (-1.0, 1.0),
        (-1.0, 1.0),
        (-1.0, 1.0),
        (-1.0, 1.0),
        (-1.0, 1.0),
        (-1.0, 1.0),
        (-1.0, 1.0),
        (-1.0, 1.0),
        (-1.0, 1.0),
        (-1.0, 1.0),
        (-0.7, 0.7),
        (-1.0, 1.0),
        (-1.0, 1.0),
        (-1.0, 1.0),
        (-1.0, 1.0),
        (-1.0, 1.0),
        (-1.0, 1.0),
        (-1.5, 1.5),
        (-2.0, 1.5),
        (-2.5, 1.5),
        (-3.0, 2.0),
        (-5.0, 2.0),
        (-16.0, 2.5),
        (f64::NEG_INFINITY, 3.0),
    ];

    /// Design bound asserted per rate: largest |digital − analytic| over 10 Hz–20 kHz.
    fn design_bound_db(fs: f64) -> f64 {
        if fs < 46_000.0 {
            0.26
        } else if fs < 90_000.0 {
            0.13
        } else {
            0.002
        }
    }

    #[test]
    fn analytic_matches_golden() {
        let g = GoldenSet::load("weighting_iec61672").expect("golden");
        let f = g.f64("exact_hz").expect("f");
        let a: Vec<f64> = f.iter().map(|&f| Weighting::A.analytic_db(f)).collect();
        let c: Vec<f64> = f.iter().map(|&f| Weighting::C.analytic_db(f)).collect();
        g.assert_f64("a_weight_db", &a);
        g.assert_f64("c_weight_db", &c);
        let poles = iec61672_poles_hz();
        for (p, name) in poles.iter().zip(["f1_hz", "f2_hz", "f3_hz", "f4_hz"]) {
            assert!((p - g.scalar(name).expect("pole")).abs() < 1e-9 * p);
        }
    }

    /// Digital response vs Annex E at the exact Table 3 frequencies: inside class 1 limits,
    /// and inside the stated design bound.
    #[test]
    fn digital_response_within_class1_per_rate() {
        let g = GoldenSet::load("weighting_iec61672").expect("golden");
        let f = g.f64("exact_hz").expect("f");
        for (w, name) in [(Weighting::A, "a_weight_db"), (Weighting::C, "c_weight_db")] {
            let target = g.f64(name).expect("target");
            for fs in RATES {
                let filt = WeightingFilter::new(w, fs).expect("design");
                let mut worst = 0.0f64;
                for ((&fi, &t), &(lo, hi)) in f.iter().zip(&target).zip(&CLASS1) {
                    let err = filt.response_db(fi) - t;
                    assert!(
                        err >= lo && err <= hi,
                        "{w:?} @ {fs} Hz, {fi:.1} Hz: error {err:.3} dB outside class 1 [{lo}, {hi}]"
                    );
                    worst = worst.max(err.abs());
                }
                eprintln!("{w:?} {fs} Hz: max |error| at Table 3 frequencies = {worst:.4} dB");
                assert!(worst <= design_bound_db(fs), "{w:?} @ {fs}: {worst}");
            }
        }
    }

    /// Between nominal frequencies the limits are the larger of the two neighbours
    /// (IEC 61672-1 5.5.7); check a dense grid against the analytic weighting.
    #[test]
    fn dense_grid_within_design_bound() {
        for w in [Weighting::A, Weighting::C] {
            for fs in RATES {
                let filt = WeightingFilter::new(w, fs).expect("design");
                let mut worst = (0.0f64, 0.0);
                for i in 0..=2000 {
                    let f = 10.0 * 2000f64.powf(f64::from(i) / 2000.0);
                    let err = filt.response_db(f) - w.analytic_db(f);
                    if err.abs() > worst.0 {
                        worst = (err.abs(), f);
                    }
                }
                eprintln!(
                    "{w:?} {fs} Hz: max |error| 10 Hz-20 kHz = {:.4} dB at {:.0} Hz",
                    worst.0, worst.1
                );
                assert!(worst.0 <= design_bound_db(fs), "{w:?} @ {fs}: {worst:?}");
            }
        }
    }

    #[test]
    fn other_rates_stay_accurate_in_audio_band() {
        for fs in [32_000.0, 88_200.0, 176_400.0, 192_000.0] {
            for w in [Weighting::A, Weighting::C] {
                let filt = WeightingFilter::new(w, fs).expect("design");
                for i in 0..=400 {
                    let f_hi = (0.4 * fs).min(20_000.0);
                    let f = 10.0 * (f_hi / 10.0).powf(f64::from(i) / 400.0);
                    let err = filt.response_db(f) - w.analytic_db(f);
                    assert!(err.abs() < 0.3, "{w:?} @ {fs}, {f} Hz: {err}");
                }
            }
        }
    }

    #[test]
    fn rejects_low_rate_but_z_is_passthrough() {
        assert!(matches!(
            WeightingFilter::new(Weighting::A, 22_050.0),
            Err(WeightingError::UnsupportedRate { .. })
        ));
        let mut z = WeightingFilter::new(Weighting::Z, 8000.0).expect("z");
        let mut x = [0.25, -1.0, 0.5];
        z.process(&mut x);
        assert_eq!(x, [0.25, -1.0, 0.5]);
        assert_eq!(z.response_db(1234.0), 0.0);
    }

    /// Time-domain check: a steady 1 kHz sine passes at unity gain and a 100 Hz sine is
    /// attenuated by the A weighting's −19.1 dB, measured from samples.
    #[test]
    fn time_domain_sine_gain() {
        for fs in RATES {
            for (f, w) in [
                (1000.0, Weighting::A),
                (100.0, Weighting::A),
                (50.0, Weighting::C),
            ] {
                let mut filt = WeightingFilter::new(w, fs).expect("design");
                let n = (fs * 2.0) as usize;
                let settle = n / 2;
                let mut acc = 0.0;
                for i in 0..n {
                    let x = (TAU * f * i as f64 / fs).sin();
                    let y = filt.process_sample(x);
                    if i >= settle {
                        acc += y * y;
                    }
                }
                let gain_db = 10.0 * (2.0 * acc / (n - settle) as f64).log10();
                let expect = filt.response_db(f);
                assert!(
                    (gain_db - expect).abs() < 0.01,
                    "{w:?} {f} @ {fs}: {gain_db} vs {expect}"
                );
                assert!((gain_db - w.analytic_db(f)).abs() < 0.02);
            }
        }
    }
}
