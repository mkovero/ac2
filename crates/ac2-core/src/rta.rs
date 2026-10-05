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
//! # Multirate
//!
//! A band's filter only needs a sample rate a few times its upper edge, so the input is
//! halved repeatedly by half-band decimators and each band runs at the lowest rate that
//! keeps it accurate; the whole bank then costs about as much as its top two octaves at the
//! full rate. A band runs at stage k (rate fs/2^k) only if
//! - its upper edge is at most fs_k/4, so it lies well inside the decimators' pass band
//!   (flat to 0.2 of their input rate, i.e. 0.4 fs_k),
//! - its full response — decimator chain times band filter, including what aliases into it —
//!   meets the class 1 mask, and
//! - its group delay at the mid-band frequency is at most [`LATENCY_SLACK`] above that of
//!   the same band designed at the full rate, so a decimated band responds as fast as a
//!   full-rate one would (decimation filters add delay, and the narrow bands have plenty).
//!
//! Otherwise it runs at a higher rate (ultimately the full rate).
//!
//! The decimators are polyphase IIR half-bands (two branches of first-order all-pass
//! sections in z², elliptic design): pass band to 0.2 fs, stop band from 0.3 fs at
//! ≥ [`HALF_BAND_STOP_DB`] dB. Being power-complementary, their pass-band deviation is
//! below 10⁻⁹ dB, so band levels are unaffected; their non-linear phase is irrelevant to
//! band power. Any frequency at or above 0.6 fs_k lies in the stop band of some earlier
//! decimator before it can alias, so a stage-k band is attenuated by at least that much
//! there and the class 1 check only has to cover frequencies below 0.6 fs_k.
//!
//! All filtering is f64. The bands of one rate and order are run several at a time with
//! their states side by side, so the feedback latency of one band's recursion is hidden
//! behind the others' arithmetic and the compiler can use SIMD. f32 is not used even for the
//! decimated bands: their rounding error, amplified by the noise gain of high-Q sections,
//! sits about 117 dB below the band's own level for full-scale white noise at that rate
//! (≈ −140 dB FS² in a 1/24-octave band), which a 24-bit input can resolve next to a loud
//! neighbouring band; at the full rate it would be far worse. The decimated stages are also
//! the minority of the work, so f32 would buy little.
//!
//! Band power is the mean square of each band's output samples at its own rate over the
//! averaging interval. Bands at a decimated rate get one output per 2^k input samples; a
//! band that has not produced an output sample since the last reset repeats its previous
//! interval's power.
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

/// Group delay in seconds of a response at `f`, by a central difference of its phase.
fn group_delay(h: impl Fn(f64) -> Complex64, f: f64) -> f64 {
    let df = f * 1e-6;
    -(h(f + df) / h(f - df)).arg() / (2.0 * PI * 2.0 * df)
}

/// Whether a magnitude response meets the class 1 mask (reference attenuation 0 dB) at every
/// point of a dense grid of Ω = G^(e/b), e ∈ [−4.5, 4.5], below `f_max`, and is at least the
/// far stop-band minimum (70 dB) on a dense linear grid from the top of that range up to
/// `f_max`.
fn meets_class1(mag: impl Fn(f64) -> f64, fm: f64, b: u32, f_max: f64) -> bool {
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
    /// Whether the band's full response (decimators and band filter) meets the IEC 61260-1
    /// class 1 mask below Nyquist.
    pub meets_class1: bool,
    /// Decimation factor 2^k of the rate the band filter runs at (1 = full rate).
    pub decimation: u32,
}

/// Lowest and highest order tried when designing a band.
const BASE_ORDER: usize = 3;
const MAX_ORDER: usize = 6;

/// Allowed relative increase of a decimated band's mid-band group delay over the same band
/// designed at the full rate.
pub const LATENCY_SLACK: f64 = 0.1;

/// Deepest decimation stage (2^16 is far below any audio band at audio rates).
const MAX_STAGE: u32 = 16;

/// A band runs at stage k only if its upper edge is at most this fraction of fs_k; the
/// decimators are flat to 0.4 fs_k, which leaves the band's upper skirt room to fall.
const MAX_UPPER_EDGE_FRACTION: f64 = 0.25;

/// Number of all-pass coefficients of the half-band decimator.
const HALF_BAND_COEFS: usize = 7;

/// Guaranteed half-band stop-band attenuation (0.3 fs to 0.5 fs of its input rate) in dB;
/// the elliptic design with [`HALF_BAND_COEFS`] coefficients reaches ≈ 120.7 dB.
pub const HALF_BAND_STOP_DB: f64 = 120.0;

/// Coefficients of a polyphase IIR half-band low-pass H(z) = ½(A₀(z²) + z⁻¹A₁(z²)), each Aᵢ
/// a cascade of (c + z⁻²)/(1 + c·z⁻²) with the even-indexed coefficients in A₀ and the odd
/// ones in A₁, for transition band 0.2–0.3 of the sample rate. This is the closed-form
/// elliptic design (Valenzuela & Constantinides): the coefficients follow from the elliptic
/// modulus k of the transition band through its nome q, using rapidly converging theta-
/// function series.
fn half_band_coefs() -> [f64; HALF_BAND_COEFS] {
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
fn half_band_response(c: &[f64; HALF_BAND_COEFS], f: f64, fs: f64) -> Complex64 {
    let z1 = Complex64::from_polar(1.0, -2.0 * PI * f / fs);
    let z2 = z1 * z1;
    let mut a = [Complex64::new(1.0, 0.0); 2];
    for (i, &ci) in c.iter().enumerate() {
        a[i % 2] *= (ci + z2) / (1.0 + ci * z2);
    }
    0.5 * (a[0] + z1 * a[1])
}

/// Response of the decimator chain in front of stage `stage` at `f` (input rate `fs`).
fn chain_response(c: &[f64; HALF_BAND_COEFS], stage: u32, f: f64, fs: f64) -> Complex64 {
    (0..stage).fold(Complex64::new(1.0, 0.0), |h, j| {
        h * half_band_response(c, f, fs / f64::from(1u32 << j))
    })
}

/// Half-band pass-band edge as a fraction of its input rate.
const HALF_BAND_PASS: f64 = 0.2;

/// Magnitude of the decimator chain in front of stage `stage` at `f` (input rate `fs`).
/// Decimators whose pass band contains `f` are taken as exactly 1: their deviation there
/// (< 10⁻¹⁰ dB, see `half_band_design`) is far below the class 1 check's slack, and skipping
/// them leaves at most the last decimator to evaluate for any band's check grid.
fn chain_magnitude(c: &[f64; HALF_BAND_COEFS], stage: u32, f: f64, fs: f64) -> f64 {
    (0..stage)
        .map(|j| fs / f64::from(1u32 << j))
        .filter(|&fs_j| f > HALF_BAND_PASS * fs_j)
        .map(|fs_j| half_band_response(c, f, fs_j).norm())
        .product()
}

/// Streaming polyphase half-band decimator by two. Input samples alternate between the
/// branches: x[2m−1] into A₁, x[2m] into A₀, and each pair yields y[m].
#[derive(Debug, Clone)]
struct HalfBand {
    c: [f64; HALF_BAND_COEFS],
    /// (previous input, previous output) of each first-order all-pass at the output rate.
    state: [(f64, f64); HALF_BAND_COEFS],
    pending: Option<f64>,
}

impl HalfBand {
    fn new(c: [f64; HALF_BAND_COEFS]) -> Self {
        Self {
            c,
            state: [(0.0, 0.0); HALF_BAND_COEFS],
            pending: None,
        }
    }

    fn reset(&mut self) {
        self.state = [(0.0, 0.0); HALF_BAND_COEFS];
        self.pending = None;
    }

    /// Decimates `x`, appending the outputs to `out`.
    fn process(&mut self, x: &[f64], out: &mut Vec<f64>) {
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

/// Bands run side by side in one inner loop.
const LANES: usize = 4;

/// Up to [`LANES`] bands of the same rate and order. Each section is the band-pass biquad
/// with its gain factored out, b = (1, 0, −1), so the inner loop needs two multiplies per
/// section; the product of the section gains squared scales the accumulated energy.
#[derive(Debug, Clone)]
struct Group {
    /// Band index per lane; unused lanes run a harmless all-zero-feedback section on the
    /// input and are never read.
    bands: [Option<usize>; LANES],
    a1: Vec<[f64; LANES]>,
    a2: Vec<[f64; LANES]>,
    s1: Vec<[f64; LANES]>,
    s2: Vec<[f64; LANES]>,
    energy: [f64; LANES],
}

impl Group {
    fn new(lanes: &[(usize, &[Biquad])]) -> Self {
        let nsec = lanes[0].1.len();
        let mut g = Self {
            bands: [None; LANES],
            a1: vec![[0.0; LANES]; nsec],
            a2: vec![[0.0; LANES]; nsec],
            s1: vec![[0.0; LANES]; nsec],
            s2: vec![[0.0; LANES]; nsec],
            energy: [0.0; LANES],
        };
        for (l, &(band, sections)) in lanes.iter().enumerate() {
            debug_assert_eq!(sections.len(), nsec);
            debug_assert!(sections.iter().all(|s| s.b[1] == 0.0 && s.b[2] == -s.b[0]));
            g.bands[l] = Some(band);
            for (j, s) in sections.iter().enumerate() {
                g.a1[j][l] = s.a[0];
                g.a2[j][l] = s.a[1];
            }
        }
        g
    }

    fn process(&mut self, x: &[f64]) {
        match self.a1.len() {
            3 => self.run::<3>(x),
            4 => self.run::<4>(x),
            5 => self.run::<5>(x),
            6 => self.run::<6>(x),
            n => unreachable!("band order {n} outside {BASE_ORDER}..={MAX_ORDER}"),
        }
    }

    /// The cascade for a compile-time section count, with coefficients and state in local
    /// arrays so the per-sample loop works in registers.
    fn run<const S: usize>(&mut self, x: &[f64]) {
        let load = |v: &[[f64; LANES]]| -> [[f64; LANES]; S] {
            let mut a = [[0.0; LANES]; S];
            a.copy_from_slice(v);
            a
        };
        let (a1, a2) = (load(&self.a1), load(&self.a2));
        let (mut s1, mut s2) = (load(&self.s1), load(&self.s2));
        let mut e = self.energy;
        for &xi in x {
            let mut v = [xi; LANES];
            for j in 0..S {
                for l in 0..LANES {
                    // Transposed direct form II with b = (1, 0, −1).
                    let y = v[l] + s1[j][l];
                    s1[j][l] = s2[j][l] - a1[j][l] * y;
                    s2[j][l] = -v[l] - a2[j][l] * y;
                    v[l] = y;
                }
            }
            for l in 0..LANES {
                e[l] += v[l] * v[l];
            }
        }
        self.s1.copy_from_slice(&s1);
        self.s2.copy_from_slice(&s2);
        self.energy = e;
    }

    fn reset(&mut self) {
        for s in self.s1.iter_mut().chain(self.s2.iter_mut()) {
            *s = [0.0; LANES];
        }
        self.energy = [0.0; LANES];
    }
}

/// One rate of the bank: the bands that run at fs/2^k and the decimator to the next rate.
#[derive(Debug, Clone)]
struct Stage {
    groups: Vec<Group>,
    /// Output samples at this rate since the last reset of the powers.
    samples: u64,
    /// Decimator from this rate to the next; `None` for the last stage.
    down: Option<HalfBand>,
    /// This stage's input at its own rate (unused for the full-rate stage).
    buf: Vec<f64>,
}

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
    /// Decimation stage k.
    stage: u32,
    /// Unit-gain sections at fs/2^k (for the response).
    sections: Vec<Biquad>,
    /// Square of the product of the section gains, factored out of the running filter.
    gain_sq: f64,
    /// Where the band's running energy lives: (stage, group, lane).
    slot: (usize, usize, usize),
    /// Mean square of the last interval that produced output, if any.
    last_power: f64,
}

/// The lowest order in `BASE_ORDER..=MAX_ORDER` whose full response at stage `stage` meets
/// class 1, or `MAX_ORDER` if none does.
fn design_band(
    hb: &[f64; HALF_BAND_COEFS],
    fm: f64,
    (lower, upper): (f64, f64),
    b: u32,
    fs: f64,
    stage: u32,
) -> (usize, Vec<Biquad>, bool) {
    let fs_k = fs / f64::from(1u32 << stage);
    // Above 0.6 fs_k the decimator chain alone exceeds the far stop-band minimum.
    let f_max = if stage == 0 {
        0.5 * fs
    } else {
        (0.6 * fs_k).min(0.5 * fs)
    };
    let mut last = None;
    for order in BASE_ORDER..=MAX_ORDER {
        let sections = butterworth_bandpass(lower, upper, fs_k, order);
        let mag = |f: f64| {
            chain_magnitude(hb, stage, f, fs) * cascade_response(&sections, f, fs_k).norm()
        };
        let ok = meets_class1(mag, fm, b, f_max);
        if ok {
            return (order, sections, true);
        }
        last = Some((order, sections, false));
    }
    last.expect("at least one order tried")
}

/// One band of `fraction` centred at `fm`, designed to run at the full rate `fs`: the
/// bank's design without decimation (order 3 per side, raised near Nyquist until the
/// class 1 mask holds). `None` when the band's upper edge is at or above Nyquist. For
/// offline filtering of a whole record (room acoustics), where the bank's multirate saving
/// buys nothing and its decimators' phase would only add delay.
pub fn full_rate_band(fraction: BandFraction, fm: f64, fs: f64) -> Option<(BandInfo, Vec<Biquad>)> {
    let (lower, upper) = fraction.edges(fm);
    if !(fs.is_finite() && fs > 0.0) || upper >= 0.5 * fs {
        return None;
    }
    let (order, sections, ok) =
        design_band(&half_band_coefs(), fm, (lower, upper), fraction.b(), fs, 0);
    Some((
        BandInfo {
            centre_hz: fm,
            lower_hz: lower,
            upper_hz: upper,
            order,
            meets_class1: ok,
            decimation: 1,
        },
        sections,
    ))
}

/// IEC 61260-1 fractional-octave filterbank. Accumulates the mean square of each band's
/// output (band power in FS²) between reads.
#[derive(Debug, Clone)]
pub struct OctaveFilterBank {
    fs: f64,
    fraction: BandFraction,
    half_band: [f64; HALF_BAND_COEFS],
    bands: Vec<Band>,
    stages: Vec<Stage>,
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
        let hb = half_band_coefs();
        let mut bands: Vec<Band> = fraction
            .centres(f_lo, f_hi)
            .into_iter()
            .filter_map(|fm| {
                let (lower, upper) = fraction.edges(fm);
                if upper >= 0.5 * fs {
                    return None;
                }
                // Orders are only raised near Nyquist, far above the fs/8 below which a band
                // can be decimated, so the base-order full-rate design sets the delay budget.
                let full_base = butterworth_bandpass(lower, upper, fs, BASE_ORDER);
                let budget = (1.0 + LATENCY_SLACK)
                    * group_delay(|f| cascade_response(&full_base, f, fs), fm);
                let deepest = (1..=MAX_STAGE)
                    .take_while(|&k| upper <= MAX_UPPER_EDGE_FRACTION * fs / f64::from(1u32 << k))
                    .last()
                    .unwrap_or(0);
                let (stage, (order, sections, ok)) = (1..=deepest)
                    .rev()
                    .find_map(|k| {
                        let fs_k = fs / f64::from(1u32 << k);
                        let delay = |sections: &[Biquad]| {
                            group_delay(
                                |f| {
                                    chain_response(&hb, k, f, fs)
                                        * cascade_response(sections, f, fs_k)
                                },
                                fm,
                            )
                        };
                        // Latency first: it only needs the base-order design and is cheap.
                        let base = butterworth_bandpass(lower, upper, fs_k, BASE_ORDER);
                        if delay(&base) > budget {
                            return None;
                        }
                        let d = design_band(&hb, fm, (lower, upper), b, fs, k);
                        (d.2 && (d.0 == BASE_ORDER || delay(&d.1) <= budget)).then_some((k, d))
                    })
                    .unwrap_or_else(|| (0, design_band(&hb, fm, (lower, upper), b, fs, 0)));
                let gain_sq = sections.iter().map(|s| s.b[0] * s.b[0]).product();
                Some(Band {
                    info: BandInfo {
                        centre_hz: fm,
                        lower_hz: lower,
                        upper_hz: upper,
                        order,
                        meets_class1: ok,
                        decimation: 1 << stage,
                    },
                    stage,
                    sections,
                    gain_sq,
                    slot: (0, 0, 0),
                    last_power: f64::NAN,
                })
            })
            .collect();
        if bands.is_empty() {
            return Err(RtaError::NoBands);
        }
        let n_stages = bands.iter().map(|b| b.stage).max().unwrap_or(0) as usize + 1;
        let mut stages: Vec<Stage> = (0..n_stages)
            .map(|k| Stage {
                groups: Vec::new(),
                samples: 0,
                down: (k + 1 < n_stages).then(|| HalfBand::new(hb)),
                buf: Vec::new(),
            })
            .collect();
        for (k, stage) in stages.iter_mut().enumerate() {
            for order in BASE_ORDER..=MAX_ORDER {
                let members: Vec<usize> = (0..bands.len())
                    .filter(|&i| bands[i].stage as usize == k && bands[i].info.order == order)
                    .collect();
                for chunk in members.chunks(LANES) {
                    let lanes: Vec<(usize, &[Biquad])> = chunk
                        .iter()
                        .map(|&i| (i, bands[i].sections.as_slice()))
                        .collect();
                    let g = stage.groups.len();
                    stage.groups.push(Group::new(&lanes));
                    for (l, &i) in chunk.iter().enumerate() {
                        bands[i].slot = (k, g, l);
                    }
                }
            }
        }
        Ok(Self {
            fs,
            fraction,
            half_band: hb,
            bands,
            stages,
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

    /// Complex response of band `i` to a sinusoid at `f` Hz (below Nyquist), decimators
    /// included: for a band at a decimated rate the magnitude is that of whatever alias of
    /// `f` reaches the band filter, so it gives the band power a steady sine produces.
    pub fn response(&self, i: usize, f: f64) -> Complex64 {
        let band = &self.bands[i];
        let fs_k = self.fs / f64::from(1u32 << band.stage);
        chain_response(&self.half_band, band.stage, f, self.fs)
            * cascade_response(&band.sections, f, fs_k)
    }

    /// Filters a block through every band and accumulates the output energy.
    pub fn process(&mut self, block: &[f64]) {
        for k in 0..self.stages.len() {
            let (head, tail) = self.stages.split_at_mut(k + 1);
            let stage = &mut head[k];
            let x = if k == 0 { block } else { stage.buf.as_slice() };
            for g in &mut stage.groups {
                g.process(x);
            }
            stage.samples += x.len() as u64;
            if let (Some(down), Some(next)) = (&mut stage.down, tail.first_mut()) {
                next.buf.clear();
                down.process(x, &mut next.buf);
            }
        }
        self.samples += block.len() as u64;
    }

    /// Samples accumulated since the last [`OctaveFilterBank::reset_powers`].
    pub fn samples(&self) -> u64 {
        self.samples
    }

    /// Mean square of band `band` since the last reset; its previous interval's when its
    /// rate has produced no output yet.
    fn power(&self, band: &Band) -> f64 {
        let (k, g, l) = band.slot;
        let stage = &self.stages[k];
        if stage.samples == 0 {
            band.last_power
        } else {
            band.gain_sq * stage.groups[g].energy[l] / stage.samples as f64
        }
    }

    /// Mean-square output per band (FS²) since the last reset; NaN if nothing was accumulated.
    ///
    /// # Panics
    /// If `out.len() != self.len()`.
    pub fn band_powers(&self, out: &mut [f64]) {
        assert_eq!(out.len(), self.bands.len(), "output length");
        for (o, b) in out.iter_mut().zip(&self.bands) {
            *o = if self.samples == 0 {
                f64::NAN
            } else {
                self.power(b)
            };
        }
    }

    /// Starts a new averaging interval; the filter state is kept so there is no new transient.
    pub fn reset_powers(&mut self) {
        if self.samples > 0 {
            for i in 0..self.bands.len() {
                let p = self.power(&self.bands[i]);
                self.bands[i].last_power = p;
            }
        }
        for s in &mut self.stages {
            for g in &mut s.groups {
                g.energy = [0.0; LANES];
            }
            s.samples = 0;
        }
        self.samples = 0;
    }

    /// Clears filter state and accumulated power.
    pub fn reset(&mut self) {
        for s in &mut self.stages {
            for g in &mut s.groups {
                g.reset();
            }
            if let Some(d) = &mut s.down {
                d.reset();
            }
            s.samples = 0;
        }
        for b in &mut self.bands {
            b.last_power = f64::NAN;
        }
        self.samples = 0;
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

    /// Narrowest band at the highest rate, designed at the full rate: the impulse response
    /// energy (white-noise power gain) of the running filter equals the integral of the
    /// designed |H|², so f64 is numerically sound even for the highest-Q sections.
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

    /// Deterministic white noise in ±0.5 (xorshift).
    fn noise(n: usize, seed: u64) -> Vec<f64> {
        let mut s = seed | 1;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s >> 11) as f64 / (1u64 << 53) as f64 - 0.5
            })
            .collect()
    }

    /// The bank without decimation: per band the lowest order whose full-rate cascade meets
    /// class 1, each band run sample by sample through unit-gain sections.
    fn reference_bands(fr: BandFraction, fs: f64, f_lo: f64, f_hi: f64) -> Vec<Vec<Biquad>> {
        fr.centres(f_lo, f_hi)
            .into_iter()
            .filter_map(|fm| {
                let (lo, hi) = fr.edges(fm);
                if hi >= fs / 2.0 {
                    return None;
                }
                let mut sos = Vec::new();
                for order in BASE_ORDER..=MAX_ORDER {
                    sos = butterworth_bandpass(lo, hi, fs, order);
                    let mag = |f| cascade_response(&sos, f, fs).norm();
                    if meets_class1(mag, fm, fr.b(), fs / 2.0) {
                        break;
                    }
                }
                Some(sos)
            })
            .collect()
    }

    /// Mean square per band of `x[warm..]` after running `x[..warm]`.
    fn reference_powers(bands: &mut [Vec<Biquad>], x: &[f64], warm: usize) -> Vec<f64> {
        bands
            .iter_mut()
            .map(|sos| {
                let mut e = 0.0;
                for (i, &v) in x.iter().enumerate() {
                    let mut y = v;
                    for s in sos.iter_mut() {
                        y = s.process_sample(y);
                    }
                    if i >= warm {
                        e += y * y;
                    }
                }
                e / (x.len() - warm) as f64
            })
            .collect()
    }

    /// White-noise power gain (2/fs)∫|H|² df over fm/16..min(16 fm, fs/2) on a log grid.
    fn power_gain(h: impl Fn(f64) -> Complex64, fm: f64, fs: f64) -> f64 {
        let (a, b) = (fm / 16.0, (fm * 16.0).min(fs / 2.0));
        let n = 20_000;
        let r = (b / a).ln();
        (0..n)
            .map(|k| {
                let f = a * (r * (k as f64 + 0.5) / n as f64).exp();
                h(f).norm_sqr() * f * r / n as f64
            })
            .sum::<f64>()
            * 2.0
            / fs
    }

    fn bank_powers(bank: &mut OctaveFilterBank, x: &[f64], warm: usize, block: usize) -> Vec<f64> {
        bank.reset();
        for c in x[..warm].chunks(block) {
            bank.process(c);
        }
        bank.reset_powers();
        for c in x[warm..].chunks(block) {
            bank.process(c);
        }
        let mut p = vec![0.0; bank.len()];
        bank.band_powers(&mut p);
        p
    }

    /// The half-band design: pass band flat, stop band at least the guaranteed attenuation,
    /// and power-complementary (|H(f)|² + |H(fs/2 − f)|² = 1), which the two-branch all-pass
    /// structure guarantees whatever the coefficients and so checks the branch wiring.
    #[test]
    fn half_band_design() {
        let c = half_band_coefs();
        let n = 20_000;
        let mut worst_pass = 0.0f64;
        let mut worst_stop = f64::NEG_INFINITY;
        for i in 0..=n {
            let f = 0.5 * f64::from(i) / f64::from(n);
            let h = half_band_response(&c, f, 1.0);
            let db = 20.0 * h.norm().log10();
            if f <= HALF_BAND_PASS {
                worst_pass = worst_pass.max(db.abs());
            }
            if f >= 0.5 - HALF_BAND_PASS {
                worst_stop = worst_stop.max(db);
            }
            let mirror = half_band_response(&c, 0.5 - f, 1.0).norm_sqr();
            assert!((h.norm_sqr() + mirror - 1.0).abs() < 1e-12, "{f}");
        }
        assert!(worst_pass < 1e-10, "pass-band deviation {worst_pass:e} dB");
        assert!(worst_stop < -HALF_BAND_STOP_DB, "stop band {worst_stop} dB");
    }

    /// The streaming decimator realises the designed response: a pass-band sine keeps its
    /// mean square, a stop-band one comes out at least the stop-band attenuation down, and
    /// splitting the input into odd-sized blocks changes nothing.
    #[test]
    fn half_band_streaming() {
        let c = half_band_coefs();
        let n = 1 << 16;
        for (f, pass) in [(0.1, true), (0.19, true), (0.31, false), (0.45, false)] {
            let x: Vec<f64> = (0..n).map(|i| (TAU * f * i as f64).sin()).collect();
            let mut whole = Vec::new();
            HalfBand::new(c).process(&x, &mut whole);
            let mut split = Vec::new();
            let mut hb = HalfBand::new(c);
            for chunk in x.chunks(37) {
                hb.process(chunk, &mut split);
            }
            assert_eq!(whole, split);
            assert_eq!(whole.len(), n / 2);
            let tail = &whole[n / 4..];
            let ms = tail.iter().map(|v| v * v).sum::<f64>() / tail.len() as f64;
            let db = 10.0 * (ms / 0.5).log10();
            let model = 20.0 * half_band_response(&c, f, 1.0).norm().log10();
            if pass {
                // The window holds a non-integer number of periods: ~10⁻⁴ dB of scatter.
                assert!(db.abs() < 1e-3 && model.abs() < 1e-10, "{f}: {db} dB");
            } else {
                assert!(
                    db < -HALF_BAND_STOP_DB && model < -HALF_BAND_STOP_DB,
                    "{f}: {db} dB"
                );
            }
        }
    }

    /// The measured band power of a steady sine equals ½|H|² from `response`, for bands at
    /// decimated rates and at frequencies in the band, on the skirts and in the region that
    /// aliases at the band's rate.
    #[test]
    fn decimated_bands_match_their_response() {
        let fs = 48_000.0;
        let mut bank =
            OctaveFilterBank::new(BandFraction::Third, fs, 20.0, 20_000.0).expect("bank");
        let picks: Vec<usize> = (0..bank.len())
            .filter(|&i| [4, 16].contains(&bank.band(i).decimation))
            .take(2)
            .chain(
                (0..bank.len())
                    .filter(|&i| bank.band(i).decimation >= 64)
                    .take(1),
            )
            .collect();
        assert_eq!(picks.len(), 3);
        for i in picks {
            let info = bank.band(i);
            let fs_k = fs / f64::from(info.decimation);
            let freqs = [
                info.centre_hz,
                info.upper_hz,
                info.lower_hz * 0.8,
                info.upper_hz * 1.5,
                0.45 * fs_k,
                0.55 * fs_k,
                fs_k - info.centre_hz,
            ];
            for f in freqs {
                let secs = (200.0 / (info.upper_hz - info.lower_hz)).max(2.0);
                let n = (fs * secs) as usize;
                let x: Vec<f64> = (0..n).map(|k| (TAU * f * k as f64 / fs).sin()).collect();
                let p = bank_powers(&mut bank, &x, n / 2, 1000)[i];
                let model = 0.5 * bank.response(i, f).norm_sqr();
                let (db, mdb) = (10.0 * p.log10(), 10.0 * model.log10());
                // Deep in the stop band the reading is the decaying start-up transient and
                // rounding, so only its being that far down is asserted.
                if mdb > -80.0 {
                    assert!(
                        (db - mdb).abs() < 0.01,
                        "{:.1} Hz band, {f:.1} Hz: {db:.4} vs {mdb:.4} dB",
                        info.centre_hz
                    );
                } else {
                    assert!(
                        db < -77.0,
                        "{:.1} Hz band, {f:.1} Hz: {db:.2} dB",
                        info.centre_hz
                    );
                }
            }
        }
    }

    /// The multirate bank against the same bands run at the full rate (the design without
    /// decimation): white-noise power gain from the responses, for every fraction and rate,
    /// and unit gain at every mid-band frequency.
    #[test]
    fn noise_and_sine_gain_match_full_rate() {
        let mut worst = 0.0f64;
        for fs in RATES {
            for fr in FRACTIONS {
                let bank = OctaveFilterBank::new(fr, fs, 19.0, 21_000.0).expect("bank");
                let refb = reference_bands(fr, fs, 19.0, 21_000.0);
                assert_eq!(refb.len(), bank.len());
                for (i, sos) in refb.iter().enumerate() {
                    let fm = bank.band(i).centre_hz;
                    let gn = power_gain(|f| bank.response(i, f), fm, fs);
                    let gr = power_gain(|f| cascade_response(sos, f, fs), fm, fs);
                    let d = 10.0 * (gn / gr).log10();
                    worst = worst.max(d.abs());
                    assert!(d.abs() < 0.006, "{fs} 1/{} {fm:.1} Hz: {d:+.5} dB", fr.b());
                    // Unit gain at f_m, or at the digital image of the analog centre for
                    // bands whose order is raised near Nyquist (at the full rate either way).
                    let centre = bank.response(i, fm).norm();
                    let expect = cascade_response(sos, fm, fs).norm();
                    assert!((centre - expect).abs() < 1e-9, "{fs} 1/{} {fm:.1}", fr.b());
                    if bank.band(i).decimation > 1 {
                        assert!((centre - 1.0).abs() < 1e-9, "{fs} 1/{} {fm:.1}", fr.b());
                    }
                }
            }
        }
        eprintln!("worst noise power gain difference {worst:.5} dB");
    }

    /// Streaming band levels of white noise against the full-rate bank. The two filter sets
    /// differ in phase and delay, so their readings of one finite noise record differ by
    /// estimation scatter as well as by the (smaller) difference in power gain.
    #[test]
    fn noise_levels_match_full_rate() {
        let fs = 48_000.0;
        for (fr, secs, tol) in [
            (BandFraction::Third, 10.0, 0.02),
            (BandFraction::TwentyFourth, 20.0, 0.05),
        ] {
            let mut bank = OctaveFilterBank::new(fr, fs, 20.0, 20_000.0).expect("bank");
            let mut refb = reference_bands(fr, fs, 20.0, 20_000.0);
            let warm = (fs * 4.0) as usize;
            let x = noise(warm + (fs * secs) as usize, 7);
            let pn = bank_powers(&mut bank, &x, warm, 1024);
            let pr = reference_powers(&mut refb, &x, warm);
            let worst = pn
                .iter()
                .zip(&pr)
                .map(|(a, b)| 10.0 * (a / b).log10())
                .fold(0.0f64, |w, d| if d.abs() > w.abs() { d } else { w });
            eprintln!("1/{}: worst noise level difference {worst:+.4} dB", fr.b());
            assert!(worst.abs() < tol, "1/{}: {worst} dB", fr.b());
        }
    }

    /// Decimation happens, and never makes a band slower: every band's mid-band group delay
    /// (decimators included) is within the slack of the full-rate design's.
    #[test]
    fn decimation_keeps_latency() {
        for fs in RATES {
            for fr in FRACTIONS {
                let bank = OctaveFilterBank::new(fr, fs, 19.0, 21_000.0).expect("bank");
                for (i, info) in bank.bands().enumerate().filter(|(_, b)| b.decimation > 1) {
                    let full = butterworth_bandpass(info.lower_hz, info.upper_hz, fs, BASE_ORDER);
                    let reference = group_delay(|f| cascade_response(&full, f, fs), info.centre_hz);
                    let delay = group_delay(|f| bank.response(i, f), info.centre_hz);
                    assert!(
                        delay <= (1.0 + LATENCY_SLACK) * reference * (1.0 + 1e-9),
                        "{fs} 1/{} {:.1}: {delay} vs {reference}",
                        fr.b(),
                        info.centre_hz
                    );
                }
                let full_rate = bank.bands().filter(|b| b.decimation == 1).count();
                eprintln!(
                    "{fs} 1/{}: {full_rate} of {} bands at full rate",
                    fr.b(),
                    bank.len()
                );
                if fr.b() >= 3 {
                    assert!(full_rate * 2 < bank.len(), "{fs} 1/{}", fr.b());
                }
            }
        }
    }

    /// Readings do not depend on how the input is split into blocks.
    #[test]
    fn block_size_independent() {
        let fs = 48_000.0;
        let x = noise(48_000, 3);
        let mut bank =
            OctaveFilterBank::new(BandFraction::Sixth, fs, 20.0, 20_000.0).expect("bank");
        let whole = bank_powers(&mut bank, &x, 0, x.len());
        for block in [1, 3, 256, 1000] {
            assert_eq!(bank_powers(&mut bank, &x, 0, block), whole, "block {block}");
        }
    }

    /// A band whose rate produced no output in an interval repeats its previous reading; right
    /// after a reset it has none to repeat.
    #[test]
    fn short_interval_repeats_previous_power() {
        let fs = 48_000.0;
        let mut bank =
            OctaveFilterBank::new(BandFraction::Third, fs, 20.0, 20_000.0).expect("bank");
        let deep = (0..bank.len())
            .max_by_key(|&i| bank.band(i).decimation)
            .expect("band");
        let d = bank.band(deep).decimation as usize;
        assert!(d >= 64, "{d}");
        let x = noise(fs as usize, 5);
        let mut p = vec![0.0; bank.len()];
        bank.process(&x[..d / 2]);
        bank.band_powers(&mut p);
        assert!(p[deep].is_nan() && p[bank.len() - 1].is_finite());
        bank.process(&x[d / 2..]);
        bank.band_powers(&mut p);
        let before = p[deep];
        assert!(before > 0.0);
        bank.reset_powers();
        bank.process(&x[..d / 4]);
        bank.band_powers(&mut p);
        assert_eq!(p[deep], before);
    }
}
