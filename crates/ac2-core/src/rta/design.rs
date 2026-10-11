//! Butterworth band-pass design and the response of a biquad cascade.

use crate::weighting::Biquad;
use num_complex::Complex64;
use std::f64::consts::PI;

/// Digital Butterworth band-pass between `f_lower` and `f_upper` (−3 dB points) of order
/// `order` per side, as `order` biquads with unit gain at the digital centre frequency.
///
/// # Panics
/// If the edges are not `0 < f_lower < f_upper < fs/2` or `order` is zero.
pub fn butterworth_bandpass(f_lower: f64, f_upper: f64, fs: f64, order: usize) -> Vec<Biquad> {
    assert!(
        f_lower > 0.0 && f_upper > f_lower && f_upper < fs / 2.0,
        "invalid band {f_lower}..{f_upper} at {fs}"
    );
    assert!(order > 0, "order must be positive");
    let k = 2.0 * fs;
    let wl = k * (PI * f_lower / fs).tan();
    let wu = k * (PI * f_upper / fs).tan();
    let w0sq = wl * wu;
    let bw = wu - wl;

    // Analog band-pass poles from the low-pass prototype poles p (s → (s² + ω0²)/(s·BW)).
    let mut complex_poles = Vec::with_capacity(order);
    let mut real_poles = Vec::new();
    for i in 0..order {
        let theta = PI * (2 * i + order + 1) as f64 / (2 * order) as f64;
        let p = Complex64::from_polar(1.0, theta);
        let half = p * bw / 2.0;
        let root = (half * half - w0sq).sqrt();
        for s in [half + root, half - root] {
            if s.im.abs() <= 1e-9 * s.norm() {
                real_poles.push(s.re);
            } else if s.im > 0.0 {
                complex_poles.push(s);
            }
        }
    }
    let to_z = |s: Complex64| (k + s) / (k - s);
    let mut sections = Vec::with_capacity(order);
    for s in complex_poles {
        let z = to_z(s);
        sections.push(Biquad::new([1.0, 0.0, -1.0], [-2.0 * z.re, z.norm_sqr()]));
    }
    real_poles.sort_by(f64::total_cmp);
    for pair in real_poles.chunks(2) {
        let za = to_z(Complex64::new(pair[0], 0.0)).re;
        let zb = pair
            .get(1)
            .map_or(0.0, |&s| to_z(Complex64::new(s, 0.0)).re);
        sections.push(Biquad::new([1.0, 0.0, -1.0], [-(za + zb), za * zb]));
    }
    // Each section has one zero at z = 1 (s = 0) and one at z = −1 (s = ∞); normalising each
    // to unit gain at the centre makes the cascade peak at exactly 0 dB, as the analog
    // Butterworth band-pass does at ω0.
    let fc = fs / PI * (w0sq.sqrt() / k).atan();
    for s in &mut sections {
        let g = 1.0 / s.response(fc, fs).norm();
        s.b = [s.b[0] * g, s.b[1] * g, s.b[2] * g];
    }
    sections
}

/// Response of a biquad cascade at `f`.
pub(super) fn cascade_response(sections: &[Biquad], f: f64, fs: f64) -> Complex64 {
    sections
        .iter()
        .fold(Complex64::new(1.0, 0.0), |h, s| h * s.response(f, fs))
}

/// Group delay in seconds of a response at `f`, by a central difference of its phase.
pub(super) fn group_delay(h: impl Fn(f64) -> Complex64, f: f64) -> f64 {
    let df = f * 1e-6;
    -(h(f + df) / h(f - df)).arg() / (2.0 * PI * 2.0 * df)
}
