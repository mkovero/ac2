//! MTW cost per second of input (three-stage ladder, 48 and 96 kHz) with a whole-sample and
//! a fractional delay, and the cost of one delay change that keeps every stage's averages
//! (rotating each held block). Timing is meaningless in debug builds, so the measurement is
//! opt-in:
//! `cargo test --release -p ac2-core --test mtw_timing -- --ignored --nocapture`.

use ac2_core::grid::LogGrid;
use ac2_core::mtw::{Averaging, DepthPolicy, Ladder, Mtw, MtwConfig, SampleGate};
use std::hint::black_box;
use std::time::Instant;

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

fn engine(fs: f64, delay: f64) -> Mtw {
    Mtw::new(MtwConfig {
        sample_rate_hz: fs,
        ladder: Ladder::Standard,
        averaging: Averaging::Fifo { blocks: 16 },
        depth: DepthPolicy::EqualConfidence,
        grid: LogGrid::covering(48, 20.0, 20_000.0),
        delay_samples: delay,
    })
    .expect("engine")
}

#[test]
#[ignore = "timing; run with --release --ignored"]
fn mtw_cost_per_second() {
    for fs in [48_000.0, 96_000.0] {
        let secs = 20usize;
        let n = secs * fs as usize;
        let x = noise(n);
        let y: Vec<f64> = x.iter().map(|v| 0.5 * v).collect();
        for delay in [100.0, 100.3] {
            let mut m = engine(fs, delay);
            let t = Instant::now();
            for (i, (cx, cy)) in x.chunks(1024).zip(y.chunks(1024)).enumerate() {
                black_box(
                    m.push((i * 1024) as u64, cx, cy, SampleGate::Accept)
                        .expect("push"),
                );
            }
            let per_s = t.elapsed().as_secs_f64() / secs as f64;
            println!(
                "{fs} Hz delay {delay}: {:.3} ms per second of input",
                per_s * 1e3
            );
            // A delay change that keeps every stage: rotate the held averages.
            let t = Instant::now();
            let reps = 50;
            for k in 0..reps {
                black_box(m.set_delay(delay + if k % 2 == 0 { 0.3 } else { 0.0 }));
            }
            println!(
                "{fs} Hz: one kept delay change {:.3} ms",
                t.elapsed().as_secs_f64() * 1e3 / reps as f64
            );
        }
    }
}
