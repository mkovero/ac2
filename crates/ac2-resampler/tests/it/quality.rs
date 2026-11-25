//! The `measurement()` preset against analytic sines: the input is a sine sampled (and
//! rounded to f32) at integer times, the reference is the same sine evaluated in f64 at each
//! output position. Never compared with another resampler.

use std::f64::consts::PI;
use std::sync::Arc;

use ac2_resampler::{Kernel, Pos, Resampler, Step};

const FS: f64 = 48_000.0;
const START: u64 = 100;
const PHASE: f64 = 0.3;

const STEPS: [f64; 9] = [
    1.0,
    1.0 + 1e-6,
    1.0 - 1e-6,
    1.0 + 1e-5,
    1.0 - 1e-5,
    1.0 + 1e-4,
    1.0 - 1e-4,
    1.0 + 1e-3,
    1.0 - 1e-3,
];

/// What one tone, offset and step measured.
struct Measured {
    /// Largest sample error re the tone amplitude, dB.
    peak_error_db: f64,
    /// Fitted output amplitude re the ideal, dB.
    gain_db: f64,
    /// Fitted output phase re the ideal, degrees.
    phase_deg: f64,
}

/// Resamples a unit sine at `freq` from `START + frac` in `step`s, `n` outputs.
fn measure(kernel: &Arc<Kernel>, freq: f64, frac: f64, step: f64, n: usize) -> Measured {
    let w = 2.0 * PI * freq / FS;
    let n_in = START as usize + n + n / 500 + 2 * kernel.taps();
    let input: Vec<f32> = (0..n_in)
        .map(|i| (w * i as f64 + PHASE).sin() as f32)
        .collect();
    let mut r = Resampler::new(kernel.clone(), n_in);
    r.push(&input);
    let mut out = vec![0.0f32; n];
    r.pull(
        Pos::new(START, frac).unwrap(),
        Step::new(step).unwrap(),
        &mut out,
    )
    .unwrap();

    let (mut peak, mut ss, mut sc, mut cc, mut ys, mut yc) = (0.0f64, 0.0, 0.0, 0.0, 0.0, 0.0);
    for (i, &y) in out.iter().enumerate() {
        let theta = w * (START as f64 + frac + i as f64 * step) + PHASE;
        let (s, c) = theta.sin_cos();
        let y = f64::from(y);
        peak = peak.max((y - s).abs());
        ss += s * s;
        sc += s * c;
        cc += c * c;
        ys += y * s;
        yc += y * c;
    }
    // Least squares y ≈ a·sin θ + b·cos θ: the ideal is a = 1, b = 0.
    let det = ss * cc - sc * sc;
    let a = (ys * cc - yc * sc) / det;
    let b = (yc * ss - ys * sc) / det;
    Measured {
        peak_error_db: 20.0 * peak.log10(),
        gain_db: 20.0 * a.hypot(b).log10(),
        phase_deg: b.atan2(a).to_degrees(),
    }
}

fn fracs(count: usize) -> impl Iterator<Item = f64> {
    // Offsets spread over [0, 1) without landing on 0, where the kernel is not used.
    (0..count).map(move |i| (i as f64 + 0.37) / count as f64)
}

/// Worst peak error and gain/phase deviation over `freqs × STEPS × fracs`, asserting the
/// targets; prints the worst error per frequency × step.
fn check(freqs: &[f64], n_fracs: usize, n: usize) {
    let kernel = Arc::new(Kernel::measurement());
    let (mut worst, mut gain, mut phase) = (f64::NEG_INFINITY, 0.0f64, 0.0f64);
    println!("worst peak error, dB re tone, over {n_fracs} offsets; steps {STEPS:?}");
    for &freq in freqs {
        let mut row = Vec::new();
        for step in STEPS {
            let mut cell = f64::NEG_INFINITY;
            for frac in fracs(n_fracs) {
                let m = measure(&kernel, freq, frac, step, n);
                assert!(
                    m.peak_error_db <= -110.0,
                    "{freq} Hz, step {step}, offset {frac}: error {:.1} dB",
                    m.peak_error_db
                );
                assert!(m.gain_db.abs() <= 0.001, "{freq} Hz: gain {} dB", m.gain_db);
                // 1e-4 rad is the phase a −80 dB quadrature error gives; far looser than the
                // peak error bound above, which already covers phase.
                assert!(
                    m.phase_deg.abs() <= 1e-4_f64.to_degrees(),
                    "{freq} Hz: {} deg",
                    m.phase_deg
                );
                cell = cell.max(m.peak_error_db);
                gain = gain.max(m.gain_db.abs());
                phase = phase.max(m.phase_deg.abs());
            }
            worst = worst.max(cell);
            row.push(format!("{cell:7.1}"));
        }
        println!("{freq:8.0} Hz {}", row.join(" "));
    }
    println!("worst {worst:.1} dB, |gain| ≤ {gain:.2e} dB, |phase| ≤ {phase:.2e} deg");
}

#[test]
fn tones_meet_targets_on_a_coarse_grid() {
    check(&[20.0, 1_000.0, 10_000.0, 18_000.0, 20_000.0], 3, 384);
}

#[test]
fn slow_tones_meet_targets_20_hz_to_20_khz() {
    let mut freqs: Vec<f64> = (0..=30)
        .map(|i| 20.0 * 1000f64.powf(i as f64 / 30.0))
        .collect();
    freqs.extend([17_000.0, 18_000.0, 19_000.0, 19_500.0, 19_900.0]);
    check(&freqs, 16, 1024);
}

/// The floor under the kernel's error: an exact copy (offset 0, step 1) only shows the f32
/// rounding of the input sine, which every output carries too.
#[test]
fn f32_rounding_floor_is_far_below_target() {
    let kernel = Arc::new(Kernel::measurement());
    let mut floor = f64::NEG_INFINITY;
    for freq in [20.0, 1_000.0, 20_000.0] {
        let w = 2.0 * PI * freq / FS;
        let input: Vec<f32> = (0..4096)
            .map(|i| (w * i as f64 + PHASE).sin() as f32)
            .collect();
        let mut r = Resampler::new(kernel.clone(), input.len());
        r.push(&input);
        let mut out = vec![0.0; 2048];
        r.pull(Pos::from_index(START), Step::ONE, &mut out).unwrap();
        for (i, &y) in out.iter().enumerate() {
            let ideal = (w * (START as usize + i) as f64 + PHASE).sin();
            floor = floor.max(20.0 * (f64::from(y) - ideal).abs().log10());
        }
    }
    println!("f32 rounding floor {floor:.1} dB");
    assert!(floor < -145.0, "{floor}");
}
