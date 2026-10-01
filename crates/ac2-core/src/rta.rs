//! Fractional-octave RTA: IEC 61260-1 filterbank (design Q4). The FFT-banded RTA lives in
//! [`crate::spectrum`]; both use base-10 band edges and report band power in FS².
//!
//! Each band is a digital Butterworth band-pass: the analog low-pass prototype of order N is
//! transformed to a band-pass whose −3 dB edges are the pre-warped IEC band edges, then
//! mapped by the bilinear transform, so the digital −3 dB points land exactly on the band
//! edges. The order per side is 3 (which meets class 1 with margin in the analog domain).
//! Near Nyquist the bilinear mapping compresses the lower skirt, and an order-3 band can
//! miss the class 1 minimum attenuation there (e.g. the 1/3-octave bands at 12.5 kHz and
//! above at 44.1/48 kHz). For such bands the order is raised one step at a time until the
//! response, evaluated against the class 1 mask on a dense grid below Nyquist, passes.
//! [`BandInfo::order`] reports what was used.
//!
//! The sections run at the full rate in f64. Even the narrowest supported band
//! (1/24 octave at 20 Hz, 96 kHz: poles 8·10⁻⁵ from z = 1) keeps its designed response and
//! noise bandwidth to better than 10⁻⁶ relative in f64 (see `low_band_numerics_full_rate`),
//! so no multirate decimation is used.
//!
//! Bands whose upper edge is at or above Nyquist are not built.

use crate::grid::{iec_band_centres, iec_band_edges, iec_octave_ratio};
use crate::weighting::Biquad;
use num_complex::Complex64;
use std::f64::consts::PI;
use std::fmt;

/// Bandwidth designator 1/b of a fractional-octave band set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BandFraction {
    /// 1/1 octave.
    Octave,
    /// 1/3 octave.
    Third,
    /// 1/6 octave.
    Sixth,
    /// 1/12 octave.
    Twelfth,
    /// 1/24 octave.
    TwentyFourth,
}

impl BandFraction {
    /// The designator b.
    pub fn b(self) -> u32 {
        match self {
            BandFraction::Octave => 1,
            BandFraction::Third => 3,
            BandFraction::Sixth => 6,
            BandFraction::Twelfth => 12,
            BandFraction::TwentyFourth => 24,
        }
    }

    /// Exact IEC mid-band frequencies within `[f_lo, f_hi]`.
    pub fn centres(self, f_lo: f64, f_hi: f64) -> Vec<f64> {
        iec_band_centres(self.b(), f_lo, f_hi)
    }

    /// IEC band edges of the band centred at `fm`.
    pub fn edges(self, fm: f64) -> (f64, f64) {
        iec_band_edges(fm, self.b())
    }
}

/// Octave-band breakpoints of IEC 61260-1:2014 Table 1, class 1, upper side:
/// (exponent e of Ω = G^e, minimum, maximum relative attenuation in dB). The band edge
/// e = ½ is listed twice: inside the band first, outside second.
const CLASS1_OCTAVE: [(f64, f64, f64); 10] = [
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
fn omega_fractional(b: u32, omega_octave: f64) -> f64 {
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
fn cascade_response(sections: &[Biquad], f: f64, fs: f64) -> Complex64 {
    sections
        .iter()
        .fold(Complex64::new(1.0, 0.0), |h, s| h * s.response(f, fs))
}

/// Whether a cascade meets the class 1 mask (reference attenuation 0 dB) at every point of a
/// dense grid of Ω = G^(e/b), e ∈ [−4.5, 4.5], below Nyquist.
fn meets_class1(sections: &[Biquad], fm: f64, b: u32, fs: f64) -> bool {
    let g = iec_octave_ratio();
    let steps = 720;
    (0..=steps).all(|i| {
        let e = -4.5 + 9.0 * f64::from(i) / f64::from(steps);
        let omega = if b == 1 {
            g.powf(e)
        } else {
            let o = omega_fractional(b, g.powf(e.abs()));
            if e < 0.0 { 1.0 / o } else { o }
        };
        let f = fm * omega;
        if f >= 0.5 * fs * (1.0 - 1e-9) {
            return true;
        }
        let att = -20.0 * cascade_response(sections, f, fs).norm().log10();
        let (lo, hi) = class1_limits(b, omega);
        // A hair of slack for the dense grid landing on a band edge in floating point.
        att >= lo - 1e-9 && att <= hi + 1e-9
    })
}

/// One band of the filterbank.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandInfo {
    /// Exact mid-band frequency f_m in Hz.
    pub centre_hz: f64,
    /// Lower band edge in Hz.
    pub lower_hz: f64,
    /// Upper band edge in Hz.
    pub upper_hz: f64,
    /// Butterworth order per side actually used (3 unless raised near Nyquist).
    pub order: usize,
    /// Whether the digital response meets the IEC 61260-1 class 1 mask below Nyquist.
    pub meets_class1: bool,
}

/// Lowest and highest order tried when designing a band.
const BASE_ORDER: usize = 3;
const MAX_ORDER: usize = 6;

/// Filterbank construction failure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RtaError {
    /// Sample rate is not positive and finite.
    InvalidRate(f64),
    /// No complete band lies within the requested range below Nyquist.
    NoBands,
}

impl fmt::Display for RtaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RtaError::InvalidRate(fs) => write!(f, "invalid sample rate {fs} Hz"),
            RtaError::NoBands => write!(f, "no band fits in the requested range below Nyquist"),
        }
    }
}

impl std::error::Error for RtaError {}

#[derive(Debug, Clone)]
struct Band {
    info: BandInfo,
    sections: Vec<Biquad>,
    energy: f64,
}

/// IEC 61260-1 fractional-octave filterbank. Accumulates the mean square of each band's
/// output (band power in FS²) between reads.
#[derive(Debug, Clone)]
pub struct OctaveFilterBank {
    fs: f64,
    fraction: BandFraction,
    bands: Vec<Band>,
    samples: u64,
}

impl OctaveFilterBank {
    /// Builds the bands whose exact mid-band frequency lies in `[f_lo, f_hi]` and whose upper
    /// edge is below Nyquist.
    pub fn new(fraction: BandFraction, fs: f64, f_lo: f64, f_hi: f64) -> Result<Self, RtaError> {
        if !(fs.is_finite() && fs > 0.0) {
            return Err(RtaError::InvalidRate(fs));
        }
        let b = fraction.b();
        let bands: Vec<Band> = fraction
            .centres(f_lo, f_hi)
            .into_iter()
            .filter_map(|fm| {
                let (lower, upper) = fraction.edges(fm);
                if upper >= 0.5 * fs {
                    return None;
                }
                let mut chosen = None;
                for order in BASE_ORDER..=MAX_ORDER {
                    let sections = butterworth_bandpass(lower, upper, fs, order);
                    let ok = meets_class1(&sections, fm, b, fs);
                    if ok || order == MAX_ORDER {
                        chosen = Some((order, sections, ok));
                        break;
                    }
                }
                chosen.map(|(order, sections, ok)| Band {
                    info: BandInfo {
                        centre_hz: fm,
                        lower_hz: lower,
                        upper_hz: upper,
                        order,
                        meets_class1: ok,
                    },
                    sections,
                    energy: 0.0,
                })
            })
            .collect();
        if bands.is_empty() {
            return Err(RtaError::NoBands);
        }
        Ok(Self {
            fs,
            fraction,
            bands,
            samples: 0,
        })
    }

    /// Sample rate.
    pub fn fs(&self) -> f64 {
        self.fs
    }

    /// Band fraction.
    pub fn fraction(&self) -> BandFraction {
        self.fraction
    }

    /// Number of bands.
    pub fn len(&self) -> usize {
        self.bands.len()
    }

    /// True if there are no bands (never for a constructed bank).
    pub fn is_empty(&self) -> bool {
        self.bands.is_empty()
    }

    /// Description of band `i`.
    pub fn band(&self, i: usize) -> BandInfo {
        self.bands[i].info
    }

    /// Descriptions of all bands, low to high.
    pub fn bands(&self) -> impl Iterator<Item = BandInfo> + '_ {
        self.bands.iter().map(|b| b.info)
    }

    /// Complex response of band `i` at `f` Hz.
    pub fn response(&self, i: usize, f: f64) -> Complex64 {
        cascade_response(&self.bands[i].sections, f, self.fs)
    }

    /// Filters a block through every band and accumulates the output energy.
    pub fn process(&mut self, block: &[f64]) {
        for band in &mut self.bands {
            let mut e = 0.0;
            for &x in block {
                let mut y = x;
                for s in &mut band.sections {
                    y = s.process_sample(y);
                }
                e += y * y;
            }
            band.energy += e;
        }
        self.samples += block.len() as u64;
    }

    /// Samples accumulated since the last [`OctaveFilterBank::reset_powers`].
    pub fn samples(&self) -> u64 {
        self.samples
    }

    /// Mean-square output per band (FS²) since the last reset; NaN if nothing was accumulated.
    ///
    /// # Panics
    /// If `out.len() != self.len()`.
    pub fn band_powers(&self, out: &mut [f64]) {
        assert_eq!(out.len(), self.bands.len(), "output length");
        let n = self.samples as f64;
        for (o, b) in out.iter_mut().zip(&self.bands) {
            *o = if self.samples == 0 {
                f64::NAN
            } else {
                b.energy / n
            };
        }
    }

    /// Starts a new averaging interval; the filter state is kept so there is no new transient.
    pub fn reset_powers(&mut self) {
        for b in &mut self.bands {
            b.energy = 0.0;
        }
        self.samples = 0;
    }

    /// Clears filter state and accumulated power.
    pub fn reset(&mut self) {
        for b in &mut self.bands {
            for s in &mut b.sections {
                s.reset();
            }
        }
        self.reset_powers();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_testkit::golden::GoldenSet;
    use std::f64::consts::TAU;

    const RATES: [f64; 3] = [44_100.0, 48_000.0, 96_000.0];
    const FRACTIONS: [BandFraction; 5] = [
        BandFraction::Octave,
        BandFraction::Third,
        BandFraction::Sixth,
        BandFraction::Twelfth,
        BandFraction::TwentyFourth,
    ];

    #[test]
    fn class1_mask_breakpoints() {
        let g = iec_octave_ratio();
        assert_eq!(class1_limits(1, 1.0), (-0.4, 0.4));
        let (lo, hi) = class1_limits(1, g.powf(0.375));
        assert!((lo + 0.4).abs() < 1e-12 && (hi - 1.4).abs() < 1e-9);
        let (lo, _) = class1_limits(1, g.powf(-1.0));
        assert!((lo - 16.6).abs() < 1e-9);
        assert_eq!(class1_limits(1, g.powf(5.0)), (70.0, f64::INFINITY));
        // Just outside the edge the stop-band minimum applies.
        let (lo, hi) = class1_limits(3, g.powf(1.0 / 6.0) * 1.0001);
        assert!(lo > 1.19 && hi.is_infinite());
        // Formula 9 for the one-third-octave image of the octave breakpoint Ω = G.
        let om = omega_fractional(3, g);
        let expect = 1.0 + (g.powf(1.0 / 6.0) - 1.0) / (g.sqrt() - 1.0) * (g - 1.0);
        assert!((om - expect).abs() < 1e-15);
    }

    /// Every band of every fraction at every rate meets the class 1 relative attenuation
    /// limits at the Table 1 breakpoints (mapped by Formulas 9/10) and the edges are −3 dB.
    #[test]
    fn all_bands_meet_class1_at_breakpoints() {
        let g = iec_octave_ratio();
        for fs in RATES {
            for fr in FRACTIONS {
                let b = fr.b();
                let bank = OctaveFilterBank::new(fr, fs, 19.0, 21_000.0).expect("bank");
                let mut raised = 0;
                for (i, info) in bank.bands().enumerate() {
                    assert!(info.meets_class1, "{fs} 1/{b} {:.1}", info.centre_hz);
                    if info.order > BASE_ORDER {
                        raised += 1;
                    }
                    for (e, _, _) in CLASS1_OCTAVE {
                        for side in [1.0, -1.0] {
                            let oct = g.powf(e);
                            let om = if b == 1 {
                                oct
                            } else {
                                omega_fractional(b, oct)
                            };
                            let om = if side < 0.0 { 1.0 / om } else { om };
                            let f = info.centre_hz * om;
                            if f >= fs / 2.0 {
                                continue;
                            }
                            let att = -20.0 * bank.response(i, f).norm().log10();
                            // The band edge itself is checked below (−3 dB by design).
                            let (lo, hi) = class1_limits(b, om);
                            if (e - 0.5).abs() > 1e-12 {
                                assert!(
                                    att >= lo && att <= hi,
                                    "{fs} 1/{b} {:.1} Hz Ω={om:.4}: {att:.3} not in [{lo}, {hi}]",
                                    info.centre_hz
                                );
                            }
                        }
                    }
                    for edge in [info.lower_hz, info.upper_hz] {
                        let att = -20.0 * bank.response(i, edge).norm().log10();
                        assert!((att - 3.0103).abs() < 1e-3, "edge {edge}: {att}");
                    }
                }
                eprintln!(
                    "{fs} Hz 1/{b}: {} bands, {raised} with raised order",
                    bank.len()
                );
            }
        }
    }

    #[test]
    fn bands_above_nyquist_are_not_built() {
        let bank =
            OctaveFilterBank::new(BandFraction::Third, 44_100.0, 19.0, 21_000.0).expect("bank");
        let last = bank.band(bank.len() - 1);
        assert!(last.upper_hz < 22_050.0);
        assert!(
            (last.centre_hz - 15_848.9).abs() < 1.0,
            "{}",
            last.centre_hz
        );
        let bank48 =
            OctaveFilterBank::new(BandFraction::Third, 48_000.0, 19.0, 21_000.0).expect("bank");
        assert_eq!(bank48.len(), 31);
        assert!(matches!(
            OctaveFilterBank::new(BandFraction::Octave, 8000.0, 5000.0, 8000.0),
            Err(RtaError::NoBands)
        ));
    }

    /// Cross-check of the band-pass design against scipy.signal.butter (bilinear, pre-warped
    /// edges, same order) in the `rta_butterworth_bands` golden set.
    #[test]
    fn design_matches_scipy_butter() {
        let g = GoldenSet::load("rta_butterworth_bands").expect("golden");
        let spec = g.f64("band_spec").expect("spec");
        for (i, row) in spec.chunks(5).enumerate() {
            let [fs, _b, lower, upper, order] = [row[0], row[1], row[2], row[3], row[4]];
            let sos = butterworth_bandpass(lower, upper, fs, order as usize);
            let f = g.f64(&format!("band{i}_freq_hz")).expect("freq");
            let db: Vec<f64> = f
                .iter()
                .map(|&f| 20.0 * cascade_response(&sos, f, fs).norm().log10())
                .collect();
            g.assert_f64(&format!("band{i}_mag_db"), &db);
        }
    }

    /// Narrowest band at the highest rate: the impulse response energy (white-noise power
    /// gain) of the running filter equals the integral of the designed |H|², so f64 at full
    /// rate is numerically sound and no decimation is needed.
    #[test]
    fn low_band_numerics_full_rate() {
        let fs = 96_000.0;
        let fr = BandFraction::TwentyFourth;
        let fm = fr.centres(19.0, 21.0)[0];
        let (lo, hi) = fr.edges(fm);
        let mut sos = butterworth_bandpass(lo, hi, fs, 3);
        let n = (fs * 40.0) as usize;
        let mut energy = 0.0;
        for i in 0..n {
            let mut y = if i == 0 { 1.0 } else { 0.0 };
            for s in &mut sos {
                y = s.process_sample(y);
            }
            energy += y * y;
        }
        // Σh² = (2/fs)∫₀^{fs/2} |H(f)|² df; integrate on a fine grid around the band.
        let df = 1e-4;
        let (a, b) = (fm / 4.0, fm * 4.0);
        let steps = ((b - a) / df) as usize;
        let integral: f64 = (0..steps)
            .map(|k| {
                let f = a + (k as f64 + 0.5) * df;
                cascade_response(&sos, f, fs).norm_sqr()
            })
            .sum::<f64>()
            * df;
        let expected = 2.0 * integral / fs;
        let rel = (energy / expected - 1.0).abs();
        assert!(
            rel < 1e-6,
            "energy {energy:e} vs {expected:e} (rel {rel:e})"
        );
        // Noise bandwidth of an order-3 Butterworth band-pass is (π/6)/sin(π/6) · B.
        let nbw = integral / (hi - lo);
        assert!((nbw - (PI / 6.0) / (PI / 6.0).sin()).abs() < 1e-3, "{nbw}");
    }

    /// A steady sine at a band centre reads its mean square in that band and is attenuated
    /// in the neighbours by at least the class 1 stop-band minimum.
    #[test]
    fn sine_band_power() {
        let fs = 48_000.0;
        let mut bank =
            OctaveFilterBank::new(BandFraction::Third, fs, 100.0, 10_000.0).expect("bank");
        let k = bank
            .bands()
            .position(|b| (b.centre_hz - 1000.0).abs() < 1e-6)
            .expect("1 kHz band");
        let amp = 0.5;
        let n = fs as usize;
        let x: Vec<f64> = (0..n)
            .map(|i| amp * (TAU * 1000.0 * i as f64 / fs).sin())
            .collect();
        bank.process(&x);
        bank.reset_powers();
        bank.process(&x);
        let mut p = vec![0.0; bank.len()];
        bank.band_powers(&mut p);
        let ms = amp * amp / 2.0;
        assert!((p[k] / ms - 1.0).abs() < 1e-3, "{} vs {ms}", p[k]);
        // Neighbour two bands away: Ω = G^(2/3) → beyond G^(1/2) octave-equivalent.
        let att2 = 10.0 * (ms / p[k + 2]).log10();
        assert!(att2 > 16.6, "{att2}");
    }
}
