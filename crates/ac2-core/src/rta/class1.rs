//! IEC 61260-1 class 1 acceptance mask and the check of a response against it.

use crate::grid::iec_octave_ratio;

/// Octave-band breakpoints of IEC 61260-1:2014 Table 1, class 1, upper side:
/// (exponent e of Ω = G^e, minimum, maximum relative attenuation in dB). The band edge
/// e = ½ is listed twice: inside the band first, outside second.
pub(super) const CLASS1_OCTAVE: [(f64, f64, f64); 10] = [
    (0.0, -0.4, 0.4),
    (0.125, -0.4, 0.5),
    (0.25, -0.4, 0.7),
    (0.375, -0.4, 1.4),
    (0.5, -0.4, 5.3),
    (0.5, 1.2, f64::INFINITY),
    (1.0, 16.6, f64::INFINITY),
    (2.0, 40.5, f64::INFINITY),
    (3.0, 60.0, f64::INFINITY),
    (4.0, 70.0, f64::INFINITY),
];

/// Maps an octave-band normalised frequency Ω_h(1/1) ≥ 1 to the 1/b-octave one
/// (IEC 61260-1 Formula 9).
pub(super) fn omega_fractional(b: u32, omega_octave: f64) -> f64 {
    let g = iec_octave_ratio();
    1.0 + (g.powf(1.0 / (2.0 * f64::from(b))) - 1.0) / (g.sqrt() - 1.0) * (omega_octave - 1.0)
}

/// IEC 61260-1 class 1 acceptance limits (minimum, maximum) on relative attenuation in dB
/// at normalised frequency Ω = f/f_m for a 1/b-octave filter. Limits between breakpoints
/// are interpolated linearly in lg Ω (Formula 11); infinite maxima stay infinite. At a band
/// edge exactly, the in-band limits apply.
pub fn class1_limits(b: u32, omega: f64) -> (f64, f64) {
    // Lower side mirrors the upper side (Formula 10).
    let om = if omega < 1.0 { 1.0 / omega } else { omega };
    let g = iec_octave_ratio();
    let x = |e: f64| {
        let o = g.powf(e);
        if b == 1 { o } else { omega_fractional(b, o) }
    };
    let edge = x(0.5);
    if om <= edge * (1.0 + 1e-12) {
        let pts = &CLASS1_OCTAVE[..5];
        interpolate(pts, om, x)
    } else {
        let pts = &CLASS1_OCTAVE[5..];
        if om >= x(4.0) {
            return (70.0, f64::INFINITY);
        }
        interpolate(pts, om, x)
    }
}

fn interpolate(pts: &[(f64, f64, f64)], om: f64, x: impl Fn(f64) -> f64) -> (f64, f64) {
    for w in pts.windows(2) {
        let (ea, mina, maxa) = w[0];
        let (eb, minb, maxb) = w[1];
        let (oa, ob) = (x(ea), x(eb));
        if om <= ob * (1.0 + 1e-12) {
            let t = ((om / oa).log10() / (ob / oa).log10()).clamp(0.0, 1.0);
            let lerp = |a: f64, b: f64| {
                if a.is_infinite() || b.is_infinite() {
                    f64::INFINITY
                } else {
                    a + (b - a) * t
                }
            };
            return (lerp(mina, minb), lerp(maxa, maxb));
        }
    }
    let (_, min, max) = pts[pts.len() - 1];
    (min, max)
}

/// Whether a magnitude response meets the class 1 mask (reference attenuation 0 dB) at every
/// point of a dense grid of Ω = G^(e/b), e ∈ [−4.5, 4.5], below `f_max`, and is at least the
/// far stop-band minimum (70 dB) on a dense linear grid from the top of that range up to
/// `f_max`.
pub(super) fn meets_class1(mag: impl Fn(f64) -> f64, fm: f64, b: u32, f_max: f64) -> bool {
    let g = iec_octave_ratio();
    let steps = 720;
    let passes = |f: f64| {
        let omega = f / fm;
        let att = -20.0 * mag(f).log10();
        let (lo, hi) = class1_limits(b, omega);
        // A hair of slack for the dense grid landing on a band edge in floating point.
        att >= lo - 1e-9 && att <= hi + 1e-9
    };
    let omega_at = |e: f64| {
        if b == 1 {
            g.powf(e)
        } else {
            let o = omega_fractional(b, g.powf(e.abs()));
            if e < 0.0 { 1.0 / o } else { o }
        }
    };
    let f_limit = f_max * (1.0 - 1e-9);
    let near = (0..=steps).all(|i| {
        let e = -4.5 + 9.0 * f64::from(i) / f64::from(steps);
        let f = fm * omega_at(e);
        f >= f_limit || passes(f)
    });
    let top = fm * omega_at(4.5);
    let far_steps = 240;
    near && (top >= f_limit
        || (1..=far_steps).all(|i| {
            let f = top + (f_limit - top) * f64::from(i) / f64::from(far_steps);
            passes(f)
        }))
}
