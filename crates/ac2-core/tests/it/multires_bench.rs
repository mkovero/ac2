//! Bench-style test: CPU cost of a multi-resolution spectrum (six full-rate bands, columns
//! formed after every hand-off that produced a frame) against one 65 536-point spectrum
//! (its averaged power read after every frame), at 96 kHz with Hann windows and a FIFO of
//! 4 frames. Prints the share of one core per second of audio and never fails on it:
//!
//! ```text
//! cargo test -p ac2-core --release --test it multires_bench:: -- --nocapture
//! ```

use std::time::Instant;

use ac2_core::multires::{MultiResAnalyzer, MultiResConfig, MultiResLadder};
use ac2_core::spectrum::{Averaging, SpectrumAnalyzer, SpectrumConfig};
use ac2_core::window::Window;

/// The daemon's display hop rule.
fn display_hop(fs: f64) -> impl Fn(usize) -> usize {
    move |n| {
        let target = ((fs / 30.0) as usize).max(64);
        let pow2 = 1usize << (usize::BITS - 1 - target.leading_zeros());
        pow2.clamp((n / 8).max(1), (n / 2).max(1))
    }
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

/// Seconds of audio per timed run and runs (one short run in debug builds, where the
/// numbers mean nothing and only the code path is exercised).
fn plan() -> (f64, usize) {
    if cfg!(debug_assertions) {
        (1.0, 1)
    } else {
        (4.0, 8)
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
fn multires_vs_fixed_65536_at_96k() {
    let fs = 96_000.0;
    let (secs, runs) = plan();
    let x = noise((fs * secs) as usize);
    // The daemon hands the job blocks of about 10 ms.
    let block = 1024;

    let ladder = MultiResLadder::new(fs, display_hop(fs)).expect("ladder");
    let mut multi = MultiResAnalyzer::new(
        ladder,
        MultiResConfig {
            window: Window::Hann,
            averaging: Averaging::Fifo { frames: 4 },
            grid: MultiResLadder::grid(fs, 48),
        },
    )
    .expect("multi");
    let mut cols = vec![0.0; multi.columns()];
    let mut emitted = 0usize;
    let t_multi = best_of(runs, || {
        for b in x.chunks(block) {
            if multi.push(b) != 0 {
                multi.column_power(&mut cols);
                emitted += 1;
            }
        }
    });

    let n = 65_536;
    let mut fixed = SpectrumAnalyzer::new(SpectrumConfig {
        fs,
        n,
        hop: display_hop(fs)(n),
        window: Window::Hann,
        averaging: Averaging::Fifo { frames: 4 },
        peak_hold: None,
    })
    .expect("fixed");
    let mut sink = 0.0;
    let t_fixed = best_of(runs, || {
        for b in x.chunks(block) {
            if fixed.push(b) != 0
                && let Some(p) = fixed.average()
            {
                sink += p.folded()[1];
            }
        }
    });
    std::hint::black_box((sink, &cols, emitted));
    let share = |t: f64| 100.0 * t / secs;
    println!(
        "96 kHz: multi-res 1/48 {:.2} % of a core, fixed 65536 {:.2} %, ratio {:.2}",
        share(t_multi),
        share(t_fixed),
        t_multi / t_fixed
    );
}
