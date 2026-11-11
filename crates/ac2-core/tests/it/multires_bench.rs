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
use ac2_core::spectrum::{Averaging, SpectrumAnalyzer, SpectrumConfig, SpectrumLevel};
use ac2_core::window::Window;

/// The daemon's display hop rule (`SpectrumConfig::display_hop`) for frames every `f`
/// samples: `f` within n/8 … n/2, else its multiple at or past n/8 or its largest divisor
/// up to n/2.
fn display_hop(f: usize) -> impl Fn(usize) -> usize {
    move |n| {
        let (lo, hi) = ((n / 8).max(1), (n / 2).max(1));
        if f < lo {
            f * lo.div_ceil(f)
        } else if f <= hi {
            f
        } else {
            (lo..=hi).rev().find(|&d| f.is_multiple_of(d)).unwrap_or(hi)
        }
    }
}

/// The hop rule before frames followed the device period: the largest power of two at or
/// below `fs / 30`, within n/8 … n/2.
fn pow2_hop(fs: f64) -> impl Fn(usize) -> usize {
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

/// Seconds per second of audio a multi-resolution spectrum at `fs` costs with `hop`, fed
/// `block` samples at a time.
fn multi_cost(fs: f64, x: &[f64], block: usize, runs: usize, hop: impl Fn(usize) -> usize) -> f64 {
    let ladder = MultiResLadder::new(fs, hop).expect("ladder");
    let mut multi = MultiResAnalyzer::new(
        ladder,
        MultiResConfig {
            window: Window::Hann,
            averaging: Averaging::Fifo { frames: 4 },
            grid: MultiResLadder::grid(fs, 48),
            level: SpectrumLevel::Tone,
        },
    )
    .expect("multi");
    let mut cols = vec![0.0; multi.columns()];
    let mut emitted = 0usize;
    let t = best_of(runs, || {
        for b in x.chunks(block) {
            if multi.push(b) != 0 {
                multi.column_power(&mut cols);
                emitted += 1;
            }
        }
    });
    std::hint::black_box((&cols, emitted));
    t * fs / x.len() as f64
}

#[test]
fn multires_vs_fixed_65536_at_96k() {
    let fs = 96_000.0;
    let (secs, runs) = plan();
    let x = noise((fs * secs) as usize);
    // A live spectrum is handed each 256-sample device period; frames every 6 of them.
    let block = 256;
    let t_multi = multi_cost(fs, &x, block, runs, display_hop(1536)) * secs;
    let t_pow2 = multi_cost(fs, &x, block, runs, pow2_hop(fs)) * secs;

    let n = 65_536;
    let mut fixed = SpectrumAnalyzer::new(SpectrumConfig {
        fs,
        n,
        hop: display_hop(1536)(n),
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
    std::hint::black_box(sink);
    let share = |t: f64| 100.0 * t / secs;
    println!(
        "96 kHz: multi-res 1/48 {:.2} % of a core (pow2 30/s hops {:.2} %), fixed 65536 {:.2} %, ratio {:.2}",
        share(t_multi),
        share(t_pow2),
        share(t_fixed),
        t_multi / t_fixed
    );
}
