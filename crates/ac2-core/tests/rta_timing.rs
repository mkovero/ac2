//! Filterbank cost per input sample for each band fraction at 48 and 96 kHz, 20 Hz–20 kHz.
//! Timing is meaningless in debug builds, so the measurement is opt-in:
//! `cargo test --release -p ac2-core --test rta_timing -- --ignored --nocapture`.

use ac2_core::rta::{BandFraction, OctaveFilterBank};
use std::hint::black_box;
use std::time::Instant;

const FRACTIONS: [BandFraction; 5] = [
    BandFraction::Octave,
    BandFraction::Third,
    BandFraction::Sixth,
    BandFraction::Twelfth,
    BandFraction::TwentyFourth,
];

/// Deterministic full-band test signal (xorshift noise in ±0.5).
fn noise(n: usize) -> Vec<f64> {
    let mut s: u64 = 0x9e37_79b9_7f4a_7c15;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        })
        .collect()
}

#[test]
#[ignore = "timing; run in release with --ignored --nocapture"]
fn rta_ns_per_sample() {
    const BLOCK: usize = 1024;
    for fs in [48_000.0, 96_000.0] {
        let x = noise(BLOCK * 64);
        for fr in FRACTIONS {
            let mut bank = OctaveFilterBank::new(fr, fs, 20.0, 20_000.0).expect("bank");
            // One second of warm-up, then the best of 25 runs of 0.2 s each: the minimum is
            // the least disturbed by other load on the machine.
            let blocks = (fs as usize / BLOCK).max(1);
            for b in 0..blocks {
                let o = (b % 64) * BLOCK;
                bank.process(&x[o..o + BLOCK]);
            }
            let mut best = f64::INFINITY;
            for _ in 0..25 {
                let t = Instant::now();
                for b in 0..blocks / 5 {
                    let o = (b % 64) * BLOCK;
                    bank.process(black_box(&x[o..o + BLOCK]));
                }
                let ns = t.elapsed().as_nanos() as f64 / (blocks / 5 * BLOCK) as f64;
                best = best.min(ns);
            }
            let mut p = vec![0.0; bank.len()];
            bank.band_powers(&mut p);
            black_box(&p);
            println!(
                "fs {fs:>6} 1/{:<2} bands {:>3}: {best:8.1} ns/sample ({:.1}% of one core)",
                fr.b(),
                bank.len(),
                best * fs * 1e-7
            );
        }
    }
}
