//! Synthetic systems with known harmonic levels, recorded the way the daemon records: one
//! continuous capture with the sweeps (and their silence) somewhere in it, a loopback
//! reference with its own latency and gain, the measurement path with another delay.

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
        grid: LogGrid {
            ppo: 48,
            k_min: -240,
            k_max: 239,
        },
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

#[test]
fn memoryless_polynomial_harmonics_and_thd() {
    let (a2, a3) = (0.2, 1.265);
    let s = spec(ess(50.0, 6000.0, 3.0));
    let (reference, mic) = record(&s, 1, |x| poly(x, a2, a3), 1e-5, 1);
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    let (fund, h2, h3) = analytic(amp(), a2, a3);
    let (h2_db, h3_db) = (20.0 * h2.log10(), 20.0 * h3.log10());
    assert!((h2_db + 40.0).abs() < 0.2 && (h3_db + 50.0).abs() < 0.2);

    // Arrival: the measurement path's delay re the loopback's.
    let want = (MIC_DELAY - REF_DELAY) as f64 / FS;
    assert!((r.arrival_s - want).abs() <= 0.5 / FS, "{}", r.arrival_s);
    assert!((r.reference_db - 20.0 * REF_GAIN.log10()).abs() < 0.5);
    assert_eq!(r.repeats, 1);
    assert!(!r.clipped);

    let e2 = max_err(&r, &r.harmonics[0], 100.0, 6000.0 / 2.0 / 1.15, |_| h2_db);
    let e3 = max_err(&r, &r.harmonics[1], 100.0, 6000.0 / 3.0 / 1.15, |_| h3_db);
    eprintln!("memoryless: H2 max error {e2:.3} dB, H3 max error {e3:.3} dB");
    assert!(e2 <= 0.5 && e3 <= 0.5);

    // THD: power sum of H2 and H3.
    let thd_db = 10.0 * (h2 * h2 + h3 * h3).log10();
    let mut e_thd = 0.0f64;
    for (i, &f) in r.frequencies.iter().enumerate() {
        if (100.0..=6000.0 / 3.0 / 1.15).contains(&f) {
            assert!(is_valid(r.thd_db[i], r.thd_floor_db[i]));
            e_thd = e_thd.max((r.thd_db[i] - thd_db).abs());
        }
    }
    eprintln!("memoryless: THD max error {e_thd:.3} dB");
    assert!(e_thd <= 0.5);

    // H4 and H5 do not exist: never reported as distortion.
    for c in &r.harmonics[2..] {
        let valid = c
            .level_db
            .iter()
            .zip(&c.floor_db)
            .filter(|(l, f)| is_valid(**l, **f))
            .count();
        let measured = c.level_db.iter().filter(|v| v.is_finite()).count();
        assert!(
            valid * 50 <= measured,
            "H{}: {valid} of {measured} valid",
            c.order
        );
    }

    // Fundamental: the path gain over the loopback gain, with the cubic's compression.
    let want_db = 20.0 * (MIC_GAIN * fund / REF_GAIN).log10();
    let mut e_lin = 0.0f64;
    for (i, &f) in r.frequencies.iter().enumerate() {
        if (100.0..=5000.0).contains(&f) {
            e_lin = e_lin.max((r.magnitude_db[i] - want_db).abs());
            assert!(
                r.phase_deg[i].abs() < 2.0,
                "phase {} at {f}",
                r.phase_deg[i]
            );
        }
        if f < 50.0 || f > 6000.0 {
            assert!(r.magnitude_db[i].is_nan());
        }
    }
    eprintln!("memoryless: fundamental max error {e_lin:.4} dB");
    assert!(e_lin <= 0.1);

    // Below the sweep (and inside the fade-in) and above f2/k nothing is reported.
    let at = |f: f64| r.frequencies.iter().position(|&x| x >= f).expect("column");
    assert!(r.harmonics[0].level_db[at(40.0)].is_nan());
    assert!(r.harmonics[0].level_db[at(3100.0)].is_nan());
    assert!(r.harmonics[1].level_db[at(2100.0)].is_nan());
    assert!(r.harmonics[0].level_db[at(2900.0)].is_finite());

    // The stored IR shows the linear impulse at 0 and H2 at −L·ln 2.
    let t_of = |i: usize| r.ir_t0_s + i as f64 * r.ir_dt_s;
    let (imax, _) =
        r.ir.iter().enumerate().fold(
            (0, 0.0f64),
            |b, (i, v)| if v.abs() > b.1 { (i, v.abs()) } else { b },
        );
    assert!(t_of(imax).abs() <= r.ir_dt_s + 1.0 / FS);
    assert!(r.ir.len() <= IR_POINTS);
    let h2_t = -r.rate_s * LN_2;
    let near = |t: f64| ((t - r.ir_t0_s) / r.ir_dt_s).round() as usize;
    let around = r.ir_etc_db[near(h2_t) - 3..near(h2_t) + 3]
        .iter()
        .fold(f64::NEG_INFINITY, |a, &v| a.max(v));
    let far = r.ir_etc_db[near(h2_t * 0.85)];
    assert!(
        around > far + 15.0,
        "H2 impulse {around} dB vs {far} dB between"
    );
}

#[test]
fn hammerstein_maps_harmonics_to_the_fundamental_frequency() {
    // A polynomial followed by a low-pass: harmonic k at fundamental f is attenuated by
    // |G(k·f)| while the fundamental is attenuated by |G(f)|.
    let (a2, a3) = (0.2, 1.265);
    let g = LowPass::new(2000.0);
    let s = spec(ess(50.0, 6000.0, 3.0));
    let (reference, mic) = record(&s, 1, |x| g.run(&poly(x, a2, a3)), 1e-7, 2);
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    let (_, h2, h3) = analytic(amp(), a2, a3);
    let g = &g;
    let want = |h: f64, k: f64| move |f: f64| 20.0 * (h * g.gain(k * f) / g.gain(f)).log10();
    let e2 = max_err(&r, &r.harmonics[0], 100.0, 2400.0, want(h2, 2.0));
    let e3 = max_err(&r, &r.harmonics[1], 100.0, 1600.0, want(h3, 3.0));
    eprintln!("hammerstein: H2 max error {e2:.3} dB, H3 max error {e3:.3} dB");
    assert!(e2 <= 1.0 && e3 <= 1.0);
    // The fundamental follows the low-pass.
    for (i, &f) in r.frequencies.iter().enumerate() {
        if (100.0..=5000.0).contains(&f) {
            let w = 20.0 * (MIC_GAIN * analytic(amp(), a2, a3).0 * g.gain(f) / REF_GAIN).log10();
            assert!(
                (r.magnitude_db[i] - w).abs() < 0.2,
                "{f}: {}",
                r.magnitude_db[i]
            );
        }
    }
}

#[test]
fn noise_reads_as_floor_and_repeats_lower_it() {
    let s = spec(ess(50.0, 6000.0, 1.0));
    let floor = |repeats: usize| {
        let (reference, mic) = record(&s, repeats, |x| x.to_vec(), 3e-3, 7);
        let r = analyse_recording(&s, &reference, &mic, repeats).expect("analysis");
        assert_eq!(r.repeats, repeats);
        let (mut valid, mut measured, mut gap) = (0usize, 0usize, Vec::new());
        for c in &r.harmonics {
            for (l, f) in c.level_db.iter().zip(&c.floor_db) {
                if l.is_finite() {
                    measured += 1;
                    valid += usize::from(is_valid(*l, *f));
                    gap.push(l - f);
                }
            }
        }
        // A linear system has no harmonics: the windows hold noise like the floor's.
        assert!(valid * 20 <= measured, "{valid} of {measured} valid");
        gap.sort_by(f64::total_cmp);
        let median = gap[gap.len() / 2];
        assert!(median.abs() < 1.5, "median level − floor {median}");
        let f2 = &r.harmonics[0].floor_db;
        let mut v: Vec<f64> = f2.iter().copied().filter(|x| x.is_finite()).collect();
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    let (one, four) = (floor(1), floor(4));
    eprintln!("noise floor: 1 repeat {one:.1} dB, 4 repeats {four:.1} dB");
    // Four repeats: 6 dB less noise.
    assert!((one - four - 6.0).abs() < 1.5, "{one} vs {four}");
}

#[test]
fn a_gate_shortens_the_linear_window() {
    let s = SweepSpec {
        gate_s: Some(0.005),
        ..spec(ess(100.0, 10_000.0, 1.0))
    };
    let (reference, mic) = record(&s, 1, |x| x.to_vec(), 1e-6, 3);
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    assert!((r.linear_window_s.1 - 0.005).abs() < 1.5 / FS);
    assert!(r.ir_t0_s < r.harmonic_window_s.0.mul_add(-1.0, -r.rate_s * 5f64.ln()) + 1e-3);
    let at_1k = r
        .frequencies
        .iter()
        .position(|&f| f >= 1000.0)
        .expect("column");
    assert!((r.magnitude_db[at_1k] - 20.0 * (MIC_GAIN / REF_GAIN).log10()).abs() < 0.1);
}

#[test]
fn refusals() {
    let s = spec(ess(50.0, 6000.0, 1.0));
    let (reference, mic) = record(&s, 1, |x| x.to_vec(), 0.0, 4);
    // No sweep on the reference.
    let silent = vec![0.0; reference.len()];
    assert!(matches!(
        analyse_recording(&s, &silent, &mic, 1),
        Err(SweepError::NoReference { .. })
    ));
    // Asked for two repeats, recorded one.
    assert!(matches!(
        analyse_recording(&s, &reference, &mic, 2),
        Err(SweepError::Incomplete {
            found: 1,
            expected: 2
        })
    ));
    // The recording stops inside the post-roll.
    let cut = reference.len() - SweepTiming::new(&s).expect("t").post_roll_samples(FS) / 2;
    assert!(matches!(
        analyse_recording(&s, &reference[..cut], &mic[..cut], 1),
        Err(SweepError::Incomplete { found: 0, .. })
    ));
    // A silent measurement input.
    assert_eq!(
        analyse_recording(&s, &reference, &silent, 1),
        Err(SweepError::NoSignal)
    );
    assert_eq!(
        SweepTiming::new(&SweepSpec { max_order: 1, ..s }),
        Err(SweepError::BadOrder)
    );
    assert_eq!(
        SweepTiming::new(&SweepSpec {
            gate_s: Some(0.0),
            ..s
        }),
        Err(SweepError::BadGate)
    );
}

#[test]
fn timing_follows_the_sweep() {
    let s = spec(ess(20.0, 20_000.0, 3.0));
    let t = SweepTiming::new(&s).expect("timing");
    // L rounded so that f1·L is an integer.
    let fl = 20.0 * t.plan.rate_s;
    assert!((fl - fl.round()).abs() < 1e-9);
    assert!((t.harmonic_time_s(2) + t.plan.rate_s * LN_2).abs() < 1e-12);
    // H5's window: 10 % of the H5–H6 gap before, 90 % of the H4–H5 gap after.
    let l = t.plan.rate_s;
    assert!((t.pre_s - 0.1 * l * 1.2f64.ln()).abs() < 1e-12);
    assert!((t.post_s - 0.9 * l * 1.25f64.ln()).abs() < 1e-12);
    assert!(
        (t.window_s() / l - 0.219).abs() < 1e-3,
        "{}",
        t.window_s() / l
    );
    assert_eq!(t.post_roll_s, MIN_POST_ROLL_S);
    // A long sweep needs a longer silence after it.
    let long = SweepTiming::new(&spec(ess(20.0, 20_000.0, 60.0))).expect("timing");
    assert!(long.post_roll_s > 4.0 * long.window_s() - 1e-9);
    assert!((db_to_percent(-40.0) - 1.0).abs() < 1e-12);
    assert!(is_valid(-40.0, -46.0) && !is_valid(-40.0, -45.0));
}
