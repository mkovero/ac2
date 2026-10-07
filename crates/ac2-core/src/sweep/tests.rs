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
        tail_s: None,
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
    assert!(
        (r.reference_db - 20.0 * REF_GAIN.log10()).abs() < 0.02,
        "{}",
        r.reference_db
    );
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
        if !(50.0..=6000.0).contains(&f) {
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
    assert!(long.post_roll_s >= 4.0 * long.window_s() - 1e-9);
    // A long sweep's window is capped (the floor then falls with L) and rises over a third
    // of it before t_k.
    assert!((long.window_s() - MAX_WINDOW_S).abs() < 1e-12);
    assert!((long.pre_s - MAX_WINDOW_S / 3.0).abs() < 1e-12);
    assert!((db_to_percent(-40.0) - 1.0).abs() < 1e-12);
    assert!(is_valid(-40.0, -46.0) && !is_valid(-40.0, -45.0));
}

#[test]
fn the_lowest_columns_read_the_harmonic_level() {
    // A capped window (L·ln(6/5) > 100 ms), so the octave above the lowest resolved
    // fundamental, 2/W = 20 Hz, lies inside the sweep.
    let (a2, a3) = (0.2, 1.265);
    let s = SweepSpec {
        grid: LogGrid {
            ppo: 48,
            k_min: -288,
            k_max: 239,
        },
        ..spec(ess(10.0, 20_000.0, 5.5))
    };
    let t = SweepTiming::new(&s).expect("timing");
    assert!((t.window_s() - MAX_WINDOW_S).abs() < 1e-12);
    let (reference, mic) = record(&s, 1, |x| dc_blocked(&poly(x, a2, a3), 1.0), 1e-5, 5);
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    let (_, h2, h3) = analytic(amp(), a2, a3);
    let (h2_db, h3_db) = (20.0 * h2.log10(), 20.0 * h3.log10());
    let f_lo = 2.0 / t.window_s();
    let e2 = max_err(&r, &r.harmonics[0], f_lo, 2.0 * f_lo, |_| h2_db);
    let e3 = max_err(&r, &r.harmonics[1], f_lo, 2.0 * f_lo, |_| h3_db);
    eprintln!("lowest octave: H2 max error {e2:.3} dB, H3 max error {e3:.3} dB");
    assert!(e2 <= 0.5 && e3 <= 0.5);
}

#[test]
fn the_sweep_starts_below_the_asked_band_and_harmonics_read_from_its_start() {
    let (a2, a3) = (0.2, 1.265);
    let f1 = 50.0;
    let s = spec(ess(f1, 6000.0, 3.0));
    let t = SweepTiming::new(&s).expect("timing");
    // At least two octaves lower, a whole number of cycles per L, at the asked rate.
    let e = t.emitted;
    assert!(e.start_hz <= f1 / ONSET_EXTENSION && e.start_hz > f1 / (2.0 * ONSET_EXTENSION));
    let cycles = e.start_hz * t.plan.rate_s;
    assert!((cycles - cycles.round()).abs() < 1e-9);
    let asked = EssPlan::new(&s.ess, FS).expect("plan");
    assert!((t.plan.rate_s - asked.rate_s).abs() < 1e-12);
    // Full level from the asked start; the added octaves stay below it.
    assert!((t.full_level_hz() - f1).abs() < 1e-9);
    let below = ((t.plan.rate_s * (f1 / e.start_hz).ln() * FS) as usize).saturating_sub(1);
    let peak = |r: std::ops::Range<usize>| r.map(|n| t.plan.sample(n).abs()).fold(0.0, f64::max);
    assert!(peak(0..below / 2) < 0.5 && peak(0..t.plan.len) <= 1.0);

    let (reference, mic) = record(&s, 1, |x| dc_blocked(&poly(x, a2, a3), 1.0), 1e-5, 6);
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    let (_, h2, h3) = analytic(amp(), a2, a3);
    let e2 = max_err(&r, &r.harmonics[0], f1, 2.0 * f1, |_| 20.0 * h2.log10());
    let e3 = max_err(&r, &r.harmonics[1], f1, 2.0 * f1, |_| 20.0 * h3.log10());
    eprintln!("from the asked start: H2 max error {e2:.3} dB, H3 max error {e3:.3} dB");
    assert!(e2 <= 0.5 && e3 <= 0.5);
    for (i, &f) in r.frequencies.iter().enumerate() {
        if (f1..=6000.0 / 3.0 / 1.15).contains(&f) {
            assert!(r.thd_db[i].is_finite() && r.thd_floor_db[i].is_finite());
        }
        // The linear response covers the asked band only.
        assert_eq!(
            r.magnitude_db[i].is_finite(),
            (f1..=6000.0).contains(&f),
            "{f}"
        );
    }
}

#[test]
fn a_start_at_the_floor_is_emitted_as_asked() {
    let s = spec(ess(MIN_EMITTED_START_HZ, 20_000.0, 5.0));
    let t = SweepTiming::new(&s).expect("timing");
    assert_eq!(t.emitted, s.ess);
}

/// Deterministic 1/f noise (Kellet's three-pole pinking filter on Gaussian noise).
fn pink(len: usize, seed: u64) -> Vec<f64> {
    let mut g = Noise(seed);
    let mut b = [0.0f64; 3];
    (0..len)
        .map(|_| {
            let x = g.gauss();
            b[0] = 0.99765 * b[0] + 0.099_046 * x;
            b[1] = 0.963 * b[1] + 0.296_516_4 * x;
            b[2] = 0.57 * b[2] + 1.052_691_3 * x;
            b[0] + b[1] + b[2] + 0.1848 * x
        })
        .collect()
}

#[test]
fn averaged_noise_windows_steady_the_floor() {
    // A capped window in a minimum post-roll, as a long default sweep has; the floor of a
    // low fundamental's H2 is read the way the analysis reads it.
    let w_len = (MAX_WINDOW_S * FS) as usize;
    let w = taper(w_len, w_len / 3, w_len / 7);
    let nw = 4 * w_len.next_power_of_two();
    let end = ((MIN_POST_ROLL_S - NOISE_MARGIN_S) * FS) as i64;
    let count = ((MIN_POST_ROLL_S - NOISE_MARGIN_S - NOISE_REGION_START * MIN_POST_ROLL_S)
        / MAX_WINDOW_S) as usize;
    assert!(count >= 3, "{count} windows");
    let min_hz = 2.0 * DISTORTION_MIN_CELLS / MAX_WINDOW_S;
    let floor = |count: usize, seed: u64| {
        let h = pink((MIN_POST_ROLL_S * FS) as usize, seed);
        let p = noise_power(&h, end, count, &w, nw);
        db10(band_power(
            &p,
            FS / nw as f64,
            2.0 * 25.0,
            FLOOR_BAND_OCT,
            min_hz,
        ))
    };
    let spread = |count: usize| {
        let v: Vec<f64> = (1..=32).map(|seed| floor(count, seed)).collect();
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        (v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
    };
    let (one, many) = (spread(1), spread(count));
    eprintln!("noise floor spread: 1 window {one:.2} dB, {count} windows {many:.2} dB");
    assert!(many < 0.7 * one, "{one} vs {many}");
}

/// Linear convolution by FFT.
fn convolve(x: &[f64], h: &[f64]) -> Vec<f64> {
    let n = (x.len() + h.len()).next_power_of_two();
    let (a, b) = (fft_forward(x, n), fft_forward(h, n));
    let y = fft_inverse(a.iter().zip(&b).map(|(p, q)| p * q).collect(), n);
    y[..x.len()].to_vec()
}

/// A reverberant room through the whole chain (sweep, loopback, deconvolution, averaging):
/// the room parameters of the measured IR are those of the room's own IR, analysed
/// directly, within 0.5 % (decay times per band; 3 % broadband, whose band the sweep
/// limits) and 0.3 dB (C80: the onset moves by a sample with the band); the tail option
/// lengthens the silence the IR is taken from.
#[test]
fn a_room_keeps_its_parameters_through_the_sweep() {
    let t60 = 0.5;
    let mut noise = Noise(11);
    let room: Vec<f64> = (0..(0.9 * FS) as usize)
        .map(|i| {
            let t = i as f64 / FS;
            let direct = if i == 0 { 3.0 } else { 0.0 };
            direct + 0.05 * 10f64.powf(-3.0 * t / t60) * noise.gauss()
        })
        .collect();
    // The truth needs time before the arrival as the measurement has: the band filters run
    // backwards put the direct sound's band energy there.
    let padded: Vec<f64> = std::iter::repeat_n(0.0, (0.1 * FS) as usize)
        .chain(room.iter().copied())
        .collect();
    let truth = crate::room::analyse(&padded, FS, -0.1, (100.0, 8000.0));
    let s = SweepSpec {
        tail_s: Some(1.2),
        ..spec(ess(50.0, 12_000.0, 2.0))
    };
    let t = SweepTiming::new(&s).expect("timing");
    assert!((t.post_roll_s - 1.2).abs() < 1e-12);
    let (reference, mic) = record(&s, 1, |x| convolve(x, &room), 1e-7, 5);
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    assert!(r.room_end_s > 1.0 && r.room_end_s < 1.2, "{}", r.room_end_s);
    let pairs = std::iter::once((&r.room.broadband, &truth.broadband))
        .chain(r.room.octave.iter().filter_map(|m| {
            truth
                .octave
                .iter()
                .find(|x| x.centre_hz == m.centre_hz)
                .map(|x| (m, x))
        }))
        .collect::<Vec<_>>();
    assert!(pairs.len() >= 6, "{} bands", pairs.len());
    for (got, want) in pairs {
        let name = got.centre_hz.unwrap_or(0.0);
        for (g, w) in [
            (got.edt_s, want.edt_s),
            (got.t20_s, want.t20_s),
            (got.t30_s, want.t30_s),
        ] {
            let (g, w) = (g.expect("measured"), w.expect("truth"));
            // Broadband differs a little by construction: the sweep excites 50 Hz–12 kHz.
            let tol = if got.centre_hz.is_some() { 0.005 } else { 0.03 };
            assert!((g / w - 1.0).abs() < tol, "{name} Hz: {g} vs {w}");
        }
        let (g, w) = (got.c80_db.expect("C80"), want.c80_db.expect("C80"));
        assert!((g - w).abs() < 0.3, "{name} Hz C80: {g} vs {w}");
        // The onset is the arrival (the direct sound), whatever the chain's latencies.
        assert!(got.onset_s.abs() < 2e-3, "{name} Hz onset {}", got.onset_s);
    }
    assert!(matches!(
        SweepTiming::new(&SweepSpec {
            tail_s: Some(f64::NAN),
            ..s
        }),
        Err(SweepError::BadTail)
    ));
}

/// `x` delayed by `delay` samples, a fraction included, exactly: a linear phase on the DFT
/// of the zero-padded record (the padding keeps the circular shift from wrapping).
fn fractionally_delayed(x: &[f64], delay: f64, gain: f64) -> Vec<f64> {
    let n = 2 * x.len().next_power_of_two();
    let mut spec = fft_forward(x, n);
    let last = spec.len() - 1;
    for (k, z) in spec.iter_mut().enumerate() {
        // A fractional delay at Nyquist is not real; the sweep has nothing there.
        *z *= if k == last {
            Complex64::new(0.0, 0.0)
        } else {
            gain * Complex64::from_polar(1.0, -TAU * k as f64 * delay / n as f64)
        };
    }
    let mut y = fft_inverse(spec, n);
    y.truncate(x.len());
    y
}

/// Arrival error, samples, of a sweep through an exact `REF_DELAY`-relative fractional delay.
fn fractional_arrival_error(fs: f64, f2: f64, phi: f64, snr_db: Option<f64>) -> (f64, f64) {
    let s = SweepSpec {
        sample_rate: fs,
        ..spec(ess(50.0, f2, 1.0))
    };
    let t = SweepTiming::new(&s).expect("timing");
    let amp = dbfs_to_rms(s.level_dbfs) * std::f64::consts::SQRT_2;
    let mut x = vec![0.0; (0.37 * fs) as usize];
    x.extend((0..t.sweep_samples()).map(|n| amp * t.plan.sample(n)));
    x.extend(std::iter::repeat_n(
        0.0,
        t.post_roll_samples(fs) + (0.05 * fs) as usize,
    ));
    let reference = delayed(&x, REF_DELAY, REF_GAIN);
    let mut mic = fractionally_delayed(&x, MIC_DELAY as f64 + phi, MIC_GAIN);
    if let Some(snr) = snr_db {
        let rms = dbfs_to_rms(s.level_dbfs) * MIC_GAIN * 10f64.powf(-snr / 20.0);
        Noise(phi.to_bits() ^ fs.to_bits()).add(&mut mic, rms);
    }
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    let expected = (MIC_DELAY - REF_DELAY) as f64 + phi;
    let err = r.arrival_s * fs - expected;
    // A pure delay referred to its own arrival has no phase left.
    let at = |f: f64| {
        let i = r.frequencies.iter().position(|&c| c >= f).expect("column");
        r.phase_deg[i]
    };
    let phase = at(1000.0).abs().max(at(10_000.0).abs());
    (err, phase)
}

#[test]
fn a_fractional_delay_reads_as_a_fractional_arrival() {
    for (fs, f2) in [
        (48_000.0, 20_000.0),
        (96_000.0, 20_000.0),
        (96_000.0, 40_000.0),
    ] {
        for phi in [0.0, 0.1, 0.25, 0.5, 0.73] {
            let (clean, phase) = fractional_arrival_error(fs, f2, phi, None);
            let (noisy, _) = fractional_arrival_error(fs, f2, phi, Some(40.0));
            eprintln!(
                "fs {fs} f2 {f2} phi {phi}: clean {clean:+.5}, 40 dB {noisy:+.5} sample, \
                 phase {phase:.4}°"
            );
            assert!(clean.abs() < 0.005, "fs {fs} f2 {f2} phi {phi}: {clean}");
            assert!(noisy.abs() < 0.05, "fs {fs} f2 {f2} phi {phi}: {noisy}");
            assert!(phase < 0.5, "fs {fs} f2 {f2} phi {phi}: phase {phase}°");
        }
    }
}

/// RBJ second-order high-pass, Q = 1/√2: the low-frequency corner of a converter path.
struct HighPass {
    b: [f64; 3],
    a: [f64; 2],
}

impl HighPass {
    fn new(fc: f64) -> Self {
        let w = TAU * fc / FS;
        let alpha = w.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let c = w.cos();
        let a0 = 1.0 + alpha;
        Self {
            b: [(1.0 + c) / 2.0 / a0, -(1.0 + c) / a0, (1.0 + c) / 2.0 / a0],
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

    /// Phase, radians.
    fn phase(&self, f: f64) -> f64 {
        let z = Complex64::from_polar(1.0, -TAU * f / FS);
        let num = self.b[0] + self.b[1] * z + self.b[2] * z * z;
        let den = 1.0 + self.a[0] * z + self.a[1] * z * z;
        (num / den).arg()
    }

    /// Group delay, s.
    fn group_delay(&self, f: f64) -> f64 {
        let df = 1e-3;
        -(self.phase(f + df) - self.phase(f - df)) / (TAU * 2.0 * df)
    }
}

#[test]
fn low_frequency_phase_is_free_of_bin_ripple() {
    // A 3 Hz high-pass leads by ≈ 8° at 20 Hz and delays by ≈ 1.7 ms; the default gate
    // (≈ 0.9 s) makes the linear spectrum's bins wider than the 48-per-octave columns there,
    // so the phase must be read at each column's centre for the group delay to hold.
    let s = SweepSpec {
        grid: LogGrid {
            ppo: 48,
            k_min: -288,
            k_max: 239,
        },
        ..spec(ess(10.0, 20_000.0, 5.5))
    };
    let hp = HighPass::new(3.0);
    let x = emitted(&s, 1, 0.37);
    let reference = delayed(&x, REF_DELAY, REF_GAIN);
    let mut mic = fractionally_delayed(&hp.run(&x), MIC_DELAY as f64 + 3.7e-6 * FS, MIC_GAIN);
    Noise(11).add(&mut mic, 1e-5);
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    let cols: Vec<usize> = (0..r.frequencies.len())
        .filter(|&i| (16.0..=40.0).contains(&r.frequencies[i]))
        .collect();
    assert!(cols.len() > 60);
    // Residual against the analytic phase, less the line a misplaced arrival leaves.
    let err: Vec<(f64, f64)> = cols
        .iter()
        .map(|&i| {
            let f = r.frequencies[i];
            let e = wrap_deg(r.phase_deg[i] - hp.phase(f).to_degrees());
            (f, e)
        })
        .collect();
    let slope =
        err.iter().map(|(f, e)| f * e).sum::<f64>() / err.iter().map(|(f, _)| f * f).sum::<f64>();
    let ripple = err
        .iter()
        .fold(0.0f64, |a, (f, e)| a.max((e - slope * f).abs()));
    // Neighbour (central) difference of the unwrapped phase against the analytic delay.
    let mut worst_gd = 0.0f64;
    for w in cols.windows(3) {
        let (a, c, b) = (w[0], w[1], w[2]);
        let dphi = wrap_deg(r.phase_deg[b] - r.phase_deg[a]).to_radians();
        let gd = -dphi / (TAU * (r.frequencies[b] - r.frequencies[a]));
        let want = hp.group_delay(r.frequencies[c]);
        worst_gd = worst_gd.max((gd / want - 1.0).abs());
    }
    eprintln!(
        "phase ripple {ripple:.5}°, arrival slope {slope:.2e}°/Hz, group delay within {:.1} %",
        100.0 * worst_gd
    );
    assert!(ripple < 0.005, "phase ripple {ripple}°");
    assert!(
        worst_gd < 0.03,
        "group delay off by {:.1} %",
        100.0 * worst_gd
    );
}

#[test]
fn reference_level_is_the_mid_band_gain_under_hf_roll_off() {
    // A loopback whose gain falls towards the top of the band (a two-tap mean: cos(π f / fs),
    // −3 dB at fs/4): the level it passes on is its mid-band gain, not the matched-filter
    // peak, which weights the rolled-off top of the band most.
    let s = spec(ess(50.0, 20_000.0, 3.0));
    let x = emitted(&s, 1, 0.37);
    let d = delayed(&x, REF_DELAY, REF_GAIN);
    let reference: Vec<f64> = (0..d.len())
        .map(|i| 0.5 * (d[i] + if i > 0 { d[i - 1] } else { 0.0 }))
        .collect();
    let mic = delayed(&x, MIC_DELAY, MIC_GAIN);
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    // the two-tap power gain averaged per octave over the log-middle half of the full-level band
    let t = SweepTiming::new(&s).expect("timing");
    let f_full = t.full_level_hz();
    let f_top = (20_000.0 * (-s.ess.fade_out_s / t.plan.rate_s).exp()).min(0.45 * FS);
    let (lo, hi) = (
        f_full * (f_top / f_full).powf(0.25),
        f_full * (f_top / f_full).powf(0.75),
    );
    let n = 10_000;
    let mean = (0..n)
        .map(|i| {
            let f = lo * (hi / lo).powf((i as f64 + 0.5) / n as f64);
            (std::f64::consts::PI * f / FS).cos().powi(2)
        })
        .sum::<f64>()
        / n as f64;
    let want = 20.0 * REF_GAIN.log10() + 10.0 * mean.log10();
    assert!(
        want < 20.0 * REF_GAIN.log10() - 0.05,
        "the roll-off must reach the band: {want}"
    );
    assert!(
        (r.reference_db - want).abs() < 0.05,
        "reference {} dB, mid-band gain {want} dB",
        r.reference_db
    );
}

#[test]
fn a_dc_blocked_loopback_leaves_the_lowest_harmonics_clean() {
    // An interface's DC blocking high-pass (second order at 2 Hz) on the loopback takes the
    // reference's sub-sonic tail down to the regularisation; the path itself is flat to 1 Hz
    // with a small, known H2. The lowest columns must read that H2, not the low-frequency
    // swell the division leaves around the arrival.
    let s = SweepSpec {
        grid: LogGrid::covering(48, 20.0, 20_000.0),
        ..spec(ess(20.0, 20_000.0, 5.5))
    };
    let x = emitted(&s, 1, 0.37);
    let reference = delayed(&dc_blocked(&dc_blocked(&x, 2.0), 2.0), REF_DELAY, REF_GAIN);
    // H2 at −80 dB re the fundamental.
    let a2 = 2e-4 / amp();
    let mut mic = delayed(&dc_blocked(&poly(&x, a2, 0.0), 1.0), MIC_DELAY, MIC_GAIN);
    Noise(13).add(&mut mic, 1e-7);
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    let (_, h2, _) = analytic(amp(), a2, 0.0);
    let e2 = max_err(&r, &r.harmonics[0], 20.0, 40.0, |_| 20.0 * h2.log10());
    eprintln!("dc-blocked loopback, 20–40 Hz: H2 max error {e2:.2} dB");
    assert!(e2 <= 1.0, "{e2}");
}
