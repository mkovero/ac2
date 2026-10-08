//! Bench-style test: cost per input sample of an SPL meter's whole chain (mic-curve
//! correction, A/C weighting, F/S/I detectors, Leq, peak and the per-second Leq
//! integration), with and without a mic curve, at 48 and 96 kHz. Prints timings and never
//! fails on them:
//!
//! ```text
//! cargo test -p ac2-core --release --test it spl_bench:: -- --nocapture
//! ```

use std::time::Instant;

use ac2_core::mic_curve::MicCurve;
use ac2_core::mic_curve::{PartitionedFir, fir_partition};
use ac2_core::spl::{PeakWeighting, SplMeter, SplMeterConfig, TimeWeighting};
use ac2_core::weighting::Weighting;

fn mic_taps(fs: f64) -> Vec<f64> {
    let pts: Vec<(f64, f64)> = (-36..=26)
        .map(|k| 1000.0 * 2f64.powf(f64::from(k) / 6.0))
        .map(|f| {
            let hp = 10.0 * (f.powi(4) / (f.powi(4) + 30f64.powi(4))).log10();
            let shelf =
                10.0 * ((1.0 + (f / 4000.0).powi(2) * 4.0) / (1.0 + (f / 4000.0).powi(2))).log10();
            (f, hp + shelf)
        })
        .collect();
    MicCurve::from_points(&pts)
        .expect("curve")
        .normalised(1000.0)
        .design_fir(fs)
}

fn noise(n: usize) -> Vec<f64> {
    let mut s = 0x2545_f491_4f6c_dd1du64;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            0.25 * ((s >> 11) as f64 / (1u64 << 53) as f64 - 0.5)
        })
        .collect()
}

/// Quarter seconds of input per timed run, and runs: many short runs, so the best of them
/// misses the scheduler on a busy machine; one in debug builds, where the numbers mean
/// nothing and only the code path is exercised.
fn plan() -> (usize, usize) {
    if cfg!(debug_assertions) {
        (1, 1)
    } else {
        (1, 60)
    }
}

/// Best wall time of `runs` calls of `f`, after one warm-up call.
fn best_of(runs: usize, mut f: impl FnMut()) -> f64 {
    f();
    let mut best = f64::INFINITY;
    for _ in 0..runs {
        let t = Instant::now();
        f();
        best = best.min(t.elapsed().as_secs_f64());
    }
    best
}

#[test]
fn spl_chain_ns_per_sample() {
    let (quarters, runs) = plan();
    for fs in [48_000.0, 96_000.0] {
        let x = noise(fs as usize / 4 * quarters);
        for corr in [false, true] {
            let mut meter = SplMeter::new(SplMeterConfig {
                fs,
                weighting: Weighting::A,
                time_weighting: TimeWeighting::Fast,
                peak_weighting: PeakWeighting::C,
            })
            .expect("meter");
            if corr {
                meter.set_correction(Some(&mic_taps(fs)));
            }
            // Blocks of 256 frames, as a JACK period would deliver them.
            let best = best_of(runs, || {
                for b in x.chunks(256) {
                    meter.process(b, |_| {});
                }
            });
            let ns = best * 1e9 / x.len() as f64;
            println!(
                "spl chain fs {fs} correction {corr}: {ns:.1} ns/sample ({:.2} % of real time)",
                ns * fs * 1e-7
            );
            assert!(meter.levels().leq.is_finite());
        }
        let mut fir = PartitionedFir::new(&mic_taps(fs), fir_partition(fs));
        let mut y = vec![0.0; 256];
        let best = best_of(runs, || {
            for b in x.chunks(256) {
                fir.process(b, &mut y[..b.len()]);
            }
        });
        println!(
            "mic-curve FIR alone fs {fs}: {:.1} ns/sample",
            best * 1e9 / x.len() as f64
        );
    }
}
