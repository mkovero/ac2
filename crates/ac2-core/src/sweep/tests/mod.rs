//! Synthetic systems with known harmonic levels, recorded the way the daemon records: one
//! continuous capture with the sweeps (and their silence) somewhere in it, a loopback
//! reference with its own latency and gain, the measurement path with another delay.

use std::f64::consts::{LN_2, PI, TAU};

use num_complex::Complex64;

use super::spectral::{band_power, db10, fft_inverse, noise_power};
use super::*;
use crate::generator::dbfs_to_rms;

const FS: f64 = 48_000.0;
const LEVEL: f64 = -20.0;
/// Loopback: latency and gain of the reference path.
const REF_DELAY: usize = 17;
const REF_GAIN: f64 = 1.2;
/// Measurement path: delay and gain after the system.
const MIC_DELAY: usize = 240;
const MIC_GAIN: f64 = 0.5;

fn ess(f1: f64, f2: f64, t: f64) -> EssConfig {
    let l = t / (f2 / f1).ln();
    EssConfig {
        start_hz: f1,
        end_hz: f2,
        duration_s: t,
        fade_in_s: l * LN_2 / 6.0,
        fade_out_s: l * LN_2 / 24.0,
    }
}

fn spec(ess: EssConfig) -> SweepSpec {
    SweepSpec {
        ess,
        level_dbfs: LEVEL,
        sample_rate: FS,
        max_order: DEFAULT_MAX_ORDER,
        gate_s: None,
        tail_s: None,
        grid: LogGrid {
            ppo: 48,
            k_min: -240,
            k_max: 239,
        },
        lf_harmonics: LfHarmonics::Standard,
    }
}

/// What the generator emits: `lead` s of silence, then `repeats` × (sweep + post-roll), then
/// a little more silence.
fn emitted(spec: &SweepSpec, repeats: usize, lead: f64) -> Vec<f64> {
    let t = SweepTiming::new(spec).expect("timing");
    let amp = dbfs_to_rms(spec.level_dbfs) * std::f64::consts::SQRT_2;
    let mut x = vec![0.0; (lead * FS) as usize];
    for _ in 0..repeats {
        x.extend((0..t.sweep_samples()).map(|n| amp * t.plan.sample(n)));
        x.extend(std::iter::repeat_n(0.0, t.post_roll_samples(FS)));
    }
    x.extend(std::iter::repeat_n(0.0, (0.05 * FS) as usize));
    x
}

fn delayed(x: &[f64], d: usize, gain: f64) -> Vec<f64> {
    let mut y = vec![0.0; x.len()];
    for (i, v) in x.iter().enumerate() {
        if i + d < y.len() {
            y[i + d] = gain * v;
        }
    }
    y
}

/// Deterministic Gaussian noise (xorshift + Box–Muller).
struct Noise(u64);

impl Noise {
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn gauss(&mut self) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * u.ln()).sqrt() * (TAU * v).cos()
    }

    fn add(&mut self, x: &mut [f64], rms: f64) {
        x.iter_mut().for_each(|v| *v += rms * self.gauss());
    }
}

fn poly(x: &[f64], a2: f64, a3: f64) -> Vec<f64> {
    x.iter().map(|v| v + a2 * v * v + a3 * v * v * v).collect()
}

/// One-pole DC blocker at `fc`, as every converter path has: an even-order term's DC part
/// (the sweep's squared envelope) would otherwise reach the record unattenuated.
fn dc_blocked(x: &[f64], fc: f64) -> Vec<f64> {
    let a = 1.0 / (1.0 + TAU * fc / FS);
    let mut y = vec![0.0; x.len()];
    let (mut px, mut py) = (0.0, 0.0);
    for (o, &v) in y.iter_mut().zip(x) {
        py = a * (py + v - px);
        px = v;
        *o = py;
    }
    y
}

/// RBJ second-order low-pass, Q = 1/√2.
struct LowPass {
    b: [f64; 3],
    a: [f64; 2],
}

impl LowPass {
    fn new(fc: f64) -> Self {
        let w = TAU * fc / FS;
        let alpha = w.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let c = w.cos();
        let a0 = 1.0 + alpha;
        Self {
            b: [(1.0 - c) / 2.0 / a0, (1.0 - c) / a0, (1.0 - c) / 2.0 / a0],
            a: [-2.0 * c / a0, (1.0 - alpha) / a0],
        }
    }

    fn run(&self, x: &[f64]) -> Vec<f64> {
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        x.iter()
            .map(|&v| {
                let y = self.b[0] * v + self.b[1] * x1 + self.b[2] * x2
                    - self.a[0] * y1
                    - self.a[1] * y2;
                (x2, x1, y2, y1) = (x1, v, y1, y);
                y
            })
            .collect()
    }

    fn gain(&self, f: f64) -> f64 {
        let z = Complex64::from_polar(1.0, -TAU * f / FS);
        let num = self.b[0] + self.b[1] * z + self.b[2] * z * z;
        let den = 1.0 + self.a[0] * z + self.a[1] * z * z;
        (num / den).norm()
    }
}

/// Analytic harmonic amplitudes of `x + a2·x² + a3·x³` at amplitude `a`, re the fundamental.
fn analytic(a: f64, a2: f64, a3: f64) -> (f64, f64, f64) {
    let fund = a + 0.75 * a3 * a.powi(3);
    (
        fund / a,
        0.5 * a2 * a * a / fund,
        0.25 * a3 * a.powi(3) / fund,
    )
}

fn amp() -> f64 {
    dbfs_to_rms(LEVEL) * std::f64::consts::SQRT_2
}

/// Largest |measured − expected| over valid columns with fundamental in [lo, hi]; panics if a
/// column there is not valid.
fn max_err(
    r: &SweepAnalysis,
    c: &HarmonicCurve,
    lo: f64,
    hi: f64,
    want: impl Fn(f64) -> f64,
) -> f64 {
    let mut worst = 0.0f64;
    let mut n = 0;
    for (i, &f) in r.frequencies.iter().enumerate() {
        if f < lo || f > hi {
            continue;
        }
        let (l, fl) = (c.level_db[i], c.floor_db[i]);
        assert!(
            is_valid(l, fl),
            "H{} at {f:.0} Hz: {l:.1} dB, floor {fl:.1}",
            c.order
        );
        worst = worst.max((l - want(f)).abs());
        n += 1;
    }
    assert!(n > 20, "too few columns in {lo}..{hi}");
    worst
}

fn record(
    spec: &SweepSpec,
    repeats: usize,
    system: impl Fn(&[f64]) -> Vec<f64>,
    noise_rms: f64,
    seed: u64,
) -> (Vec<f64>, Vec<f64>) {
    let x = emitted(spec, repeats, 0.37);
    let reference = delayed(&x, REF_DELAY, REF_GAIN);
    let mut mic = delayed(&system(&x), MIC_DELAY, MIC_GAIN);
    Noise(seed).add(&mut mic, noise_rms);
    (reference, mic)
}

mod harmonics;
mod ideal;
mod noise;
mod response;
