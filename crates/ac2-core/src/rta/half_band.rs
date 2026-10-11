//! Polyphase IIR half-band decimator: design, response and streaming filter.

use num_complex::Complex64;
use std::f64::consts::PI;

/// Number of all-pass coefficients of the half-band decimator.
pub(super) const HALF_BAND_COEFS: usize = 7;

/// Guaranteed half-band stop-band attenuation (0.3 fs to 0.5 fs of its input rate) in dB;
/// the elliptic design with [`HALF_BAND_COEFS`] coefficients reaches ≈ 120.7 dB.
pub const HALF_BAND_STOP_DB: f64 = 120.0;

/// Coefficients of a polyphase IIR half-band low-pass H(z) = ½(A₀(z²) + z⁻¹A₁(z²)), each Aᵢ
/// a cascade of (c + z⁻²)/(1 + c·z⁻²) with the even-indexed coefficients in A₀ and the odd
/// ones in A₁, for transition band 0.2–0.3 of the sample rate. This is the closed-form
/// elliptic design (Valenzuela & Constantinides): the coefficients follow from the elliptic
/// modulus k of the transition band through its nome q, using rapidly converging theta-
/// function series.
pub(super) fn half_band_coefs() -> [f64; HALF_BAND_COEFS] {
    let transition = 0.1;
    let order = 2 * HALF_BAND_COEFS + 1;
    let k = ((1.0 - 2.0 * transition) * PI / 4.0).tan().powi(2);
    let kksqrt = (1.0 - k * k).powf(0.25);
    let e = 0.5 * (1.0 - kksqrt) / (1.0 + kksqrt);
    let e4 = e.powi(4);
    let q = e * (1.0 + e4 * (2.0 + e4 * (15.0 + 150.0 * e4)));
    let n = order as f64;
    let mut c = [0.0; HALF_BAND_COEFS];
    for (idx, coef) in c.iter_mut().enumerate() {
        let ci = (idx + 1) as f64;
        // Theta-series terms fall off as q^(i²); 20 terms are far past f64 resolution.
        let num: f64 = (0..20)
            .map(|i: i32| {
                let (sign, i) = ((-1.0f64).powi(i), f64::from(i));
                sign * q.powf(i * (i + 1.0)) * ((2.0 * i + 1.0) * ci * PI / n).sin()
            })
            .sum::<f64>()
            * q.powf(0.25);
        let den: f64 = 0.5
            + (1..20)
                .map(|i: i32| {
                    let (sign, i) = ((-1.0f64).powi(i), f64::from(i));
                    sign * q.powf(i * i) * (2.0 * i * ci * PI / n).cos()
                })
                .sum::<f64>();
        let ww = (num / den).powi(2);
        let x = ((1.0 - ww * k) * (1.0 - ww / k)).sqrt() / (1.0 + ww);
        *coef = (1.0 - x) / (1.0 + x);
    }
    c
}

/// Response of the half-band low-pass at `f` for input rate `fs`.
pub(super) fn half_band_response(c: &[f64; HALF_BAND_COEFS], f: f64, fs: f64) -> Complex64 {
    let z1 = Complex64::from_polar(1.0, -2.0 * PI * f / fs);
    let z2 = z1 * z1;
    let mut a = [Complex64::new(1.0, 0.0); 2];
    for (i, &ci) in c.iter().enumerate() {
        a[i % 2] *= (ci + z2) / (1.0 + ci * z2);
    }
    0.5 * (a[0] + z1 * a[1])
}

/// Response of the decimator chain in front of stage `stage` at `f` (input rate `fs`).
pub(super) fn chain_response(c: &[f64; HALF_BAND_COEFS], stage: u32, f: f64, fs: f64) -> Complex64 {
    (0..stage).fold(Complex64::new(1.0, 0.0), |h, j| {
        h * half_band_response(c, f, fs / f64::from(1u32 << j))
    })
}

/// Half-band pass-band edge as a fraction of its input rate.
pub(super) const HALF_BAND_PASS: f64 = 0.2;

/// Magnitude of the decimator chain in front of stage `stage` at `f` (input rate `fs`).
/// Decimators whose pass band contains `f` are taken as exactly 1: their deviation there
/// (< 10⁻¹⁰ dB, see `half_band_design`) is far below the class 1 check's slack, and skipping
/// them leaves at most the last decimator to evaluate for any band's check grid.
pub(super) fn chain_magnitude(c: &[f64; HALF_BAND_COEFS], stage: u32, f: f64, fs: f64) -> f64 {
    (0..stage)
        .map(|j| fs / f64::from(1u32 << j))
        .filter(|&fs_j| f > HALF_BAND_PASS * fs_j)
        .map(|fs_j| half_band_response(c, f, fs_j).norm())
        .product()
}

/// Streaming polyphase half-band decimator by two. Input samples alternate between the
/// branches: x[2m−1] into A₁, x[2m] into A₀, and each pair yields y[m].
#[derive(Debug, Clone)]
pub(super) struct HalfBand {
    c: [f64; HALF_BAND_COEFS],
    /// (previous input, previous output) of each first-order all-pass at the output rate.
    state: [(f64, f64); HALF_BAND_COEFS],
    pending: Option<f64>,
}

impl HalfBand {
    pub(super) fn new(c: [f64; HALF_BAND_COEFS]) -> Self {
        Self {
            c,
            state: [(0.0, 0.0); HALF_BAND_COEFS],
            pending: None,
        }
    }

    pub(super) fn reset(&mut self) {
        self.state = [(0.0, 0.0); HALF_BAND_COEFS];
        self.pending = None;
    }

    /// Decimates `x`, appending the outputs to `out`.
    pub(super) fn process(&mut self, x: &[f64], out: &mut Vec<f64>) {
        let mut st = self.state;
        let c = self.c;
        let mut run = |odd: f64, even: f64| {
            let mut v = [even, odd];
            for (i, (s, &ci)) in st.iter_mut().zip(&c).enumerate() {
                // (c + z⁻¹)/(1 + c·z⁻¹) at the output rate: y = c·(x − y₁) + x₁.
                let x0 = v[i % 2];
                let y = ci * (x0 - s.1) + s.0;
                *s = (x0, y);
                v[i % 2] = y;
            }
            0.5 * (v[0] + v[1])
        };
        let mut rest = x;
        if let Some(odd) = self.pending.take() {
            match rest.split_first() {
                Some((&even, tail)) => {
                    out.push(run(odd, even));
                    rest = tail;
                }
                None => {
                    self.pending = Some(odd);
                    return;
                }
            }
        }
        let pairs = rest.chunks_exact(2);
        if let [odd] = pairs.remainder() {
            self.pending = Some(*odd);
        }
        out.extend(pairs.map(|p| run(p[0], p[1])));
        self.state = st;
    }
}
