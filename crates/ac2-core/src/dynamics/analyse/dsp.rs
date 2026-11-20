//! Envelope arithmetic of the dynamics analysis: the mixers, the one-period boxcar of the
//! fundamental, the 4-period Hann of the harmonics, and the thump low-pass.

use std::f64::consts::{PI, TAU};

use num_complex::Complex64;

use crate::weighting::Biquad;

/// Harmonics read besides the fundamental: `k = 2 ..= 5`.
pub(super) const HARMONICS: usize = 4;

/// Running sums are recomputed from scratch this often (samples): adding and subtracting
/// terms of order 1 for minutes would otherwise walk the sum by rounding.
const RESYNC: u32 = 4096;

/// Powers of `e^{−j2π/(4P)}`. Every mixer of the analysis is one: the carrier
/// (`e^{−j2πv/P}`), its harmonics, and the Hann window's two shifted copies
/// (`e^{−j2π(4k ∓ 1)v/(4P)}`). A mixer's phase depends only on the absolute sample index
/// modulo 4P, so blocks, steps and the streaming and per-step passes all share one phase.
#[derive(Debug, Clone)]
pub(super) struct Mixers {
    p: usize,
    tab: Vec<Complex64>,
}

impl Mixers {
    pub(super) fn new(p: usize) -> Self {
        let n = 4 * p;
        Self {
            p,
            tab: (0..n)
                .map(|m| Complex64::from_polar(1.0, -TAU * m as f64 / n as f64))
                .collect(),
        }
    }

    /// The carrier period, samples.
    pub(super) fn period(&self) -> usize {
        self.p
    }

    /// `e^{−j2π q v / (4P)}`.
    #[inline]
    pub(super) fn at(&self, q: usize, v: u64) -> Complex64 {
        let n = self.tab.len();
        self.tab[(q * (v % n as u64) as usize) % n]
    }

    /// The carrier mixer `e^{−j2πv/P}`.
    #[inline]
    pub(super) fn carrier(&self, v: u64) -> Complex64 {
        self.at(4, v)
    }
}

/// Samples addressed by absolute index: `x[i]` is sample `abs0 + i`.
#[derive(Debug, Clone, Copy)]
pub(super) struct Signal<'a> {
    pub(super) x: &'a [f32],
    pub(super) abs0: u64,
}

impl Signal<'_> {
    #[inline]
    pub(super) fn get(&self, v: u64) -> f64 {
        f64::from(self.x[(v - self.abs0) as usize])
    }
}

/// The fundamental's complex envelope at `u`, scaled so its magnitude is the carrier's peak
/// amplitude: the signal mixed down and averaged over one period centred on `u` — `P + 1`
/// samples with half weight at both ends, so the window is symmetric about `u` (no group
/// delay) and still exactly one period long, which cancels the carrier's image and every
/// harmonic of a steady tone.
pub(super) fn centred_at(s: Signal<'_>, u: u64, mx: &Mixers) -> Complex64 {
    let h = (mx.p / 2) as u64;
    let mut acc = Complex64::new(0.0, 0.0);
    for v in u - h..=u + h {
        let w = if v == u - h || v == u + h { 0.5 } else { 1.0 };
        acc += mx.carrier(v) * (w * s.get(v));
    }
    acc * (2.0 / mx.p as f64)
}

/// [`centred_at`] sliding one sample at a time. It holds no reference to the audio, which
/// every call passes, so a pass can stop between blocks and resume on the same sums.
#[derive(Debug, Clone)]
pub(super) struct Boxcar {
    u: u64,
    /// Full-weight sum over `[u − P/2, u + P/2]`.
    sum: Complex64,
    since: u32,
}

impl Boxcar {
    pub(super) fn new(s: Signal<'_>, mx: &Mixers, u: u64) -> Self {
        let mut b = Self {
            u,
            sum: Complex64::new(0.0, 0.0),
            since: 0,
        };
        b.sync(s, mx);
        b
    }

    #[inline]
    fn term(s: Signal<'_>, mx: &Mixers, v: u64) -> Complex64 {
        mx.carrier(v) * s.get(v)
    }

    fn sync(&mut self, s: Signal<'_>, mx: &Mixers) {
        let h = (mx.p / 2) as u64;
        self.sum = (self.u - h..=self.u + h)
            .map(|v| Self::term(s, mx, v))
            .sum();
        self.since = 0;
    }

    /// The envelope at the current sample.
    #[inline]
    pub(super) fn value(&self, s: Signal<'_>, mx: &Mixers) -> Complex64 {
        let h = (mx.p / 2) as u64;
        (self.sum - 0.5 * (Self::term(s, mx, self.u - h) + Self::term(s, mx, self.u + h)))
            * (2.0 / mx.p as f64)
    }

    #[inline]
    pub(super) fn advance(&mut self, s: Signal<'_>, mx: &Mixers) {
        let h = (mx.p / 2) as u64;
        self.sum += Self::term(s, mx, self.u + h + 1) - Self::term(s, mx, self.u - h);
        self.u += 1;
        self.since += 1;
        if self.since >= RESYNC {
            self.sync(s, mx);
        }
    }
}

/// The complex envelopes of harmonics 2 … 5 under a Hann window of 4 periods centred on the
/// current sample, scaled to peak amplitude, sliding one sample at a time.
///
/// A centred Hann of length 4P is `w(m) = ½ + ¼(e^{jθm} + e^{−jθm})`, `θ = 2π/(4P)`, for
/// `m = v − u ∈ [−2P, 2P)` (zero at both ends), so the windowed mix is three plain running
/// sums: one at `k·f_c` and one a quarter of the carrier on either side. A Hann of 4 periods
/// puts the fundamental 4 bins from harmonic 2 and every other harmonic, on its zeros: a
/// steady fundamental leaks nothing, and a moving one far less than through a boxcar.
#[derive(Debug, Clone)]
pub(super) struct HannBank {
    u: u64,
    /// Per harmonic: the sums at `4k`, `4k − 1` and `4k + 1` quarter-carriers.
    sums: [[Complex64; 3]; HARMONICS],
    /// Table index of each mixer at `u − 2P`, which is also its index at `u + 2P`: the two
    /// samples entering and leaving the window are 4P apart, a whole period of every mixer.
    idx: [[usize; 3]; HARMONICS],
    since: u32,
}

/// The mixers' steps through the table per sample: `4k`, `4k − 1`, `4k + 1`.
const fn steps(j: usize) -> [usize; 3] {
    let q = 4 * (j + 2);
    [q, q - 1, q + 1]
}

impl HannBank {
    pub(super) fn new(s: Signal<'_>, mx: &Mixers, u: u64) -> Self {
        let mut b = Self {
            u,
            sums: [[Complex64::new(0.0, 0.0); 3]; HARMONICS],
            idx: [[0; 3]; HARMONICS],
            since: 0,
        };
        b.sync(s, mx);
        b
    }

    fn sync(&mut self, s: Signal<'_>, mx: &Mixers) {
        let two_p = 2 * mx.p as u64;
        let n = mx.tab.len() as u64;
        let first = self.u - two_p;
        for j in 0..HARMONICS {
            for (m, q) in steps(j).into_iter().enumerate() {
                self.idx[j][m] = ((q as u64 * (first % n)) % n) as usize;
                self.sums[j][m] = (first..self.u + two_p)
                    .map(|v| mx.at(q, v) * s.get(v))
                    .sum();
            }
        }
        self.since = 0;
    }

    /// Peak amplitude of harmonics 2 … 5 around the current sample.
    #[inline]
    pub(super) fn value(&self, mx: &Mixers) -> [f64; HARMONICS] {
        // The window's Σw is 2P; a cosine of amplitude A mixes to A/2 · Σw = A·P.
        let rot = mx.at(1, self.u);
        let p = mx.p as f64;
        self.sums
            .map(|s| ((0.5 * s[0] + 0.25 * rot * s[1] + 0.25 * rot.conj() * s[2]) / p).norm())
    }

    #[inline]
    pub(super) fn advance(&mut self, s: Signal<'_>, mx: &Mixers) {
        let two_p = 2 * mx.p as u64;
        let n = mx.tab.len();
        let d = s.get(self.u + two_p) - s.get(self.u - two_p);
        for j in 0..HARMONICS {
            for (m, q) in steps(j).into_iter().enumerate() {
                let i = self.idx[j][m];
                self.sums[j][m] += mx.tab[i] * d;
                let next = i + q;
                self.idx[j][m] = if next >= n { next - n } else { next };
            }
        }
        self.u += 1;
        self.since += 1;
        if self.since >= RESYNC {
            self.sync(s, mx);
        }
    }
}

/// An 8th-order Butterworth low-pass at `fc`: four RBJ sections prewarped at the corner, one
/// per pole pair, `Q = 1 / (2 cos((2i + 1)π/16))`.
pub(super) fn butterworth_lowpass_8(fc: f64, fs: f64) -> [Biquad; 4] {
    let w0 = TAU * fc / fs;
    let (sin, cos) = w0.sin_cos();
    std::array::from_fn(|i| {
        let q = 1.0 / (2.0 * ((2 * i + 1) as f64 * PI / 16.0).cos());
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        let b0 = (1.0 - cos) / 2.0 / a0;
        Biquad::new(
            [b0, (1.0 - cos) / a0, b0],
            [-2.0 * cos / a0, (1.0 - alpha) / a0],
        )
    })
}

/// Runs `x` through a cascade.
#[inline]
pub(super) fn cascade(f: &mut [Biquad; 4], x: f64) -> f64 {
    f.iter_mut().fold(x, |y, b| b.process_sample(y))
}

/// Median; NaN sorts last, so a few NaN do not make it NaN.
pub(super) fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let m = v.len() / 2;
    let (_, x, _) = v.select_nth_unstable_by(m, f64::total_cmp);
    *x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(p: usize, n: usize, k: usize, amp: f64, phase: f64) -> Vec<f32> {
        (0..n)
            .map(|v| (amp * (TAU * (k * v) as f64 / p as f64 + phase).sin()) as f32)
            .collect()
    }

    #[test]
    fn boxcar_reads_the_peak_amplitude_and_ignores_harmonics() {
        let p = 48;
        let mx = Mixers::new(p);
        let mut x = tone(p, 4000, 1, 0.5, 0.3);
        for (a, b) in x.iter_mut().zip(tone(p, 4000, 3, 0.1, 1.0)) {
            *a += b;
        }
        let s = Signal { x: &x, abs0: 1000 };
        let mut b = Boxcar::new(s, &mx, 1100);
        for u in 1100..4000 {
            let direct = centred_at(s, u, &mx);
            assert!((b.value(s, &mx) - direct).norm() < 1e-9);
            assert!((direct.norm() - 0.5).abs() < 1e-6, "{}", direct.norm());
            b.advance(s, &mx);
        }
    }

    #[test]
    fn hann_bank_reads_harmonics_without_fundamental_leakage() {
        let p = 48;
        let mx = Mixers::new(p);
        let mut x = tone(p, 4000, 1, 1.0, 0.3);
        for (a, b) in x.iter_mut().zip(tone(p, 4000, 3, 0.01, 1.0)) {
            *a += b;
        }
        let s = Signal { x: &x, abs0: 0 };
        let mut h = HannBank::new(s, &mx, 500);
        for _ in 0..5000 - 500 - 4 * p - 1 {
            let v = h.value(&mx);
            assert!(v[0] < 1e-7 && v[2] < 1e-7 && v[3] < 1e-7, "{v:?}");
            assert!((v[1] - 0.01).abs() < 1e-7, "{v:?}");
            h.advance(s, &mx);
            if h.u + 2 * p as u64 >= 4000 {
                break;
            }
        }
    }

    #[test]
    fn lowpass_is_3db_down_at_the_corner_and_48db_per_octave() {
        let fs = 48_000.0;
        let f = butterworth_lowpass_8(200.0, fs);
        let db = |hz: f64| {
            20.0 * f
                .iter()
                .map(|b| b.response(hz, fs))
                .fold(Complex64::new(1.0, 0.0), |a, b| a * b)
                .norm()
                .log10()
        };
        assert!((db(200.0) + 3.01).abs() < 0.05, "{}", db(200.0));
        assert!(db(10.0).abs() < 0.01);
        assert!((db(800.0) + 96.3).abs() < 1.5, "{}", db(800.0));
    }
}
