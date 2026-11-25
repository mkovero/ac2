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
    let mut r = Resampler::new(kernel.clone(), n_in, 0).unwrap();
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

/// The worst of a sweep.
pub struct Worst {
    /// Peak error, dB re the tone.
    pub error_db: f64,
    /// |gain|, dB.
    pub gain_db: f64,
    /// |phase|, degrees.
    pub phase_deg: f64,
    /// The frequency of the worst peak error.
    pub at_hz: f64,
}

/// Worst peak error and gain/phase deviation over `freqs × steps × fracs`; prints the worst
/// error per frequency × step.
pub fn sweep(
    kernel: &Arc<Kernel>,
    freqs: &[f64],
    steps: &[f64],
    n_fracs: usize,
    n: usize,
) -> Worst {
    let mut w = Worst {
        error_db: f64::NEG_INFINITY,
        gain_db: 0.0,
        phase_deg: 0.0,
        at_hz: 0.0,
    };
    println!("worst peak error, dB re tone, over {n_fracs} offsets; steps {steps:?}");
    for &freq in freqs {
        let mut row = Vec::new();
        for &step in steps {
            let mut cell = f64::NEG_INFINITY;
            for frac in fracs(n_fracs) {
                let m = measure(kernel, freq, frac, step, n);
                cell = cell.max(m.peak_error_db);
                w.gain_db = w.gain_db.max(m.gain_db.abs());
                w.phase_deg = w.phase_deg.max(m.phase_deg.abs());
            }
            if cell > w.error_db {
                (w.error_db, w.at_hz) = (cell, freq);
            }
            row.push(format!("{cell:7.1}"));
        }
        println!("{freq:8.0} Hz {}", row.join(" "));
    }
    println!(
        "worst {:.1} dB at {:.0} Hz, |gain| ≤ {:.2e} dB, |phase| ≤ {:.2e} deg",
        w.error_db, w.at_hz, w.gain_db, w.phase_deg
    );
    w
}

/// Gain and phase bounds for every tier: under 2× above the measured worst (slow tier,
/// 2.9e-6 dB and 5.9e-6°), so a regression of a few dB in either fails.
fn assert_gain_phase(w: &Worst) {
    assert!(w.gain_db <= 1e-5, "gain {:.2e} dB", w.gain_db);
    assert!(w.phase_deg <= 1e-5, "phase {:.2e} deg", w.phase_deg);
}

/// The kernel's error has a ripple of a few hundred hertz period in frequency, so the grid
/// is dense where it peaks: a log grid alone missed the worst cell by 4 dB.
pub fn top_octave() -> impl Iterator<Item = f64> {
    (0..=100).map(|i| 10_000.0 + 100.0 * i as f64)
}

#[test]
fn tones_meet_targets_on_a_coarse_grid() {
    let kernel = Arc::new(Kernel::measurement());
    let w = sweep(
        &kernel,
        &[
            20.0, 1_000.0, 10_000.0, 15_900.0, 18_000.0, 19_700.0, 20_000.0,
        ],
        &STEPS,
        3,
        384,
    );
    // Measured −127.7 dB at 19700 Hz; bounds about 1.5 dB above.
    assert!(
        w.error_db <= -126.2,
        "{:.1} dB at {} Hz",
        w.error_db,
        w.at_hz
    );
    assert_gain_phase(&w);
}

#[test]
fn slow_tones_meet_targets_20_hz_to_20_khz() {
    let freqs: Vec<f64> = (0..30)
        .map(|i| 20.0 * 500f64.powf(i as f64 / 30.0))
        .chain(top_octave())
        .collect();
    let w = sweep(&Arc::new(Kernel::measurement()), &freqs, &STEPS, 8, 1024);
    // Measured −127.7 dB at 19700 Hz.
    assert!(
        w.error_db <= -126.2,
        "{:.1} dB at {} Hz",
        w.error_db,
        w.at_hz
    );
    assert_gain_phase(&w);
}

/// Low tones, where the kernel's own error is far smaller, through the tap sum (non-zero
/// offsets): what remains is the f32 rounding of the 56 inputs and of the output.
#[test]
fn f32_floor_through_the_tap_sum() {
    let kernel = Arc::new(Kernel::measurement());
    let w = sweep(&kernel, &[20.0, 100.0, 1_000.0], &STEPS, 8, 1024);
    // Measured −140.9 dB at 1 kHz.
    assert!(w.error_db <= -139.4, "{:.1} dB", w.error_db);
}
