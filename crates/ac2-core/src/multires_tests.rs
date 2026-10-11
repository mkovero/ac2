use super::*;
use std::f64::consts::TAU;

/// The daemon's hop rule (largest power of two ≤ fs/30, clamped to n/8 … n/2).
fn display_hop(fs: f64) -> impl Fn(usize) -> usize {
    move |n| {
        let target = ((fs / 30.0) as usize).max(64);
        let pow2 = 1usize << (usize::BITS - 1 - target.leading_zeros());
        pow2.clamp((n / 8).max(1), (n / 2).max(1))
    }
}

fn analyzer(fs: f64, ppo: u32, window: Window, averaging: Averaging) -> MultiResAnalyzer {
    let ladder = MultiResLadder::new(fs, display_hop(fs)).expect("ladder");
    MultiResAnalyzer::new(
        ladder,
        MultiResConfig {
            window,
            averaging,
            grid: MultiResLadder::grid(fs, ppo),
        },
    )
    .expect("analyzer")
}

struct Noise(u64);
impl Noise {
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        ((self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    fn gauss(&mut self) -> f64 {
        let (u1, u2) = (self.uniform(), self.uniform());
        (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()
    }
}

fn db(p: f64) -> f64 {
    10.0 * p.log10()
}

#[test]
fn ladder_keeps_lf_detail_and_short_hf_window_at_common_rates() {
    for (fs, n_max) in [
        (44_100.0, 32_768),
        (48_000.0, 32_768),
        (96_000.0, 65_536),
        (192_000.0, 131_072),
    ] {
        let l = MultiResLadder::new(fs, display_hop(fs)).expect("ladder");
        let ns: Vec<usize> = l.bands.iter().map(|b| b.n).collect();
        assert_eq!(ns[0], n_max, "{fs}");
        assert_eq!(ns.len(), BANDS);
        assert!(l.bands[0].bin_hz <= LF_BIN_HZ * (1.0 + 1e-12), "{fs}");
        let top = l.bands[BANDS - 1].window_s(fs);
        assert!(top < 0.024, "{fs}: top window {top}");
        for w in l.bands.windows(2) {
            assert_eq!(w[0].n, 2 * w[1].n);
            assert!((w[1].anchor_hz / w[0].anchor_hz - 2.0).abs() < 1e-12);
        }
    }
    // At 96 kHz the anchors are 2·κ(48)·fs/N: 203 Hz … 6.5 kHz.
    let l = MultiResLadder::new(96_000.0, display_hop(96_000.0)).expect("ladder");
    assert!(
        (l.bands[0].anchor_hz - 202.9).abs() < 0.1,
        "{}",
        l.bands[0].anchor_hz
    );
    assert!((l.bands[5].anchor_hz - 6492.0).abs() < 2.0);
    let hops: Vec<usize> = l.bands.iter().map(|b| b.hop).collect();
    assert_eq!(hops, [8192, 4096, 2048, 2048, 2048, 1024]);
}

#[test]
fn weights_sum_to_one_and_are_continuous() {
    let l = MultiResLadder::new(48_000.0, display_hop(48_000.0)).expect("ladder");
    let mut prev: Option<[f64; BANDS]> = None;
    let mut f = 10.0;
    while f < 24_000.0 {
        let mut w = [0.0; BANDS];
        for bw in l.weights(f).into_iter().flatten() {
            w[bw.band] += bw.weight;
        }
        assert!((w.iter().sum::<f64>() - 1.0).abs() < 1e-12, "{f}");
        if let Some(p) = prev {
            for (a, b) in p.iter().zip(&w) {
                assert!((a - b).abs() < 0.01, "weights jump at {f}: {p:?} → {w:?}");
            }
        }
        prev = Some(w);
        f *= 1.0005;
    }
}

#[test]
fn every_column_is_resolved_at_the_design_density_above_half_the_first_anchor() {
    let l = MultiResLadder::new(96_000.0, display_hop(96_000.0)).expect("ladder");
    let grid = MultiResLadder::grid(96_000.0, 48);
    let ranges = l.unresolved_ranges(&grid);
    assert_eq!(ranges.len(), 1, "{ranges:?}");
    let edge = l.bands[0].anchor_hz / 2.0;
    assert!(
        ranges[0].1 <= edge * 1.02 && ranges[0].1 >= edge / 1.02,
        "{ranges:?}"
    );
}

/// Sines half an octave apart from 31.5 Hz to 20 kHz, 0.5 FS peak (tone power 0.25):
/// with a flat-top window the curve's peak at each tone reads it within the window's
/// scalloping, in a single band and across every blend; with Hann within Hann's 1.42 dB.
#[test]
fn sines_read_their_level_across_every_crossover() {
    let fs = 48_000.0;
    let freqs: Vec<f64> = (0..)
        .map(|k| 31.5 * 2f64.powf(f64::from(k) / 2.0))
        .take_while(|f| *f < 20_000.0)
        .collect();
    let len = 32_768 + 4096;
    let x: Vec<f64> = (0..len)
        .map(|i| {
            let t = i as f64 / fs;
            freqs
                .iter()
                .enumerate()
                .map(|(j, f)| 0.5 * (TAU * f * t + j as f64).sin())
                .sum()
        })
        .collect();
    for (window, tol_db) in [(Window::FlatTop, 0.02), (Window::Hann, 1.45)] {
        let mut a = analyzer(fs, 48, window, Averaging::Off);
        a.push(&x);
        let mut out = vec![0.0; a.columns()];
        assert!(a.column_power(&mut out));
        let grid = a.config().grid;
        for f in &freqs {
            let c = (0..grid.len())
                .find(|&c| {
                    let (lo, hi) = grid.edges(c);
                    (lo..hi).contains(f)
                })
                .expect("column");
            // The curve's peak at the tone: a tone just inside a column's edge has its
            // peak bin's centre in the neighbouring column.
            let peak = out[c.saturating_sub(1)..(c + 2).min(out.len())]
                .iter()
                .fold(f64::NEG_INFINITY, |a, b| a.max(*b));
            let err = db(peak / 0.25);
            assert!(err.abs() <= tol_db, "{window:?} {f:.1} Hz: {err:+.3} dB");
        }
    }
}

/// White noise of variance σ² read through long FIFOs: every column's reading over the
/// noise bandwidth the ladder states (`2 · (2σ²/fs) · B(f)` in tone power) is 0 dB within
/// the statistics, and the reading has no step at any anchor.
#[test]
fn white_noise_reads_the_stated_bandwidth_without_steps() {
    let fs = 24_000.0;
    let sigma = 0.1;
    let mut a = analyzer(fs, 96, Window::Hann, Averaging::Fifo { frames: 640 });
    let n0 = a.ladder().bands[0];
    let len = n0.n + 640 * n0.hop;
    let mut rng = Noise(0x0AC2_3417);
    let x: Vec<f64> = (0..len).map(|_| sigma * rng.gauss()).collect();
    for chunk in x.chunks(4800) {
        a.push(chunk);
    }
    let mut out = vec![0.0; a.columns()];
    assert!(a.column_power(&mut out));
    let grid = a.config().grid;
    let enbw = crate::window::WindowGains::of(&Window::Hann.coefficients(1024)).enbw_bins();
    let ratio: Vec<f64> = (0..grid.len())
        .map(|c| {
            let f = grid.frequency(c);
            let expect = 2.0 * (2.0 * sigma * sigma / fs) * a.ladder().noise_bandwidth_hz(f, enbw);
            db(out[c] / expect)
        })
        .collect();
    // Mean over sixth-octave groups (16 columns at 1/96) from 30 Hz to 10 kHz.
    let groups: Vec<(f64, f64)> = (0..grid.len())
        .step_by(16)
        .filter(|&c| grid.frequency(c) > 30.0 && grid.frequency(c + 15) < 10_000.0)
        .map(|c| {
            let m = ratio[c..c + 16].iter().sum::<f64>() / 16.0;
            (grid.frequency(c + 8), m)
        })
        .collect();
    for (f, m) in &groups {
        assert!(m.abs() < 0.45, "{f:.0} Hz: {m:+.2} dB");
    }
    for b in &a.ladder().bands {
        let at = |f: f64| {
            (0..grid.len())
                .min_by(|&i, &j| {
                    (grid.frequency(i) / f)
                        .ln()
                        .abs()
                        .total_cmp(&(grid.frequency(j) / f).ln().abs())
                })
                .expect("column")
        };
        let c = at(b.anchor_hz);
        if c < 8 || c + 8 > grid.len() || b.anchor_hz > 10_000.0 {
            continue;
        }
        let below = ratio[c - 8..c].iter().sum::<f64>() / 8.0;
        let above = ratio[c..c + 8].iter().sum::<f64>() / 8.0;
        assert!(
            (below - above).abs() < 0.4,
            "step at {:.0} Hz: {below:+.2} → {above:+.2}",
            b.anchor_hz
        );
    }
}

#[test]
fn each_band_stamps_its_own_frame_end_exactly() {
    let fs = 48_000.0;
    let mut a = analyzer(fs, 48, Window::Hann, Averaging::Off);
    let bands = a.ladder().bands.clone();
    let mut total = 0u64;
    let mut rng = Noise(5);
    for size in [
        1usize, 313, 4096, 777, 10_000, 1, 2048, 30_001, 511, 65_536, 999,
    ] {
        let block: Vec<f64> = (0..size).map(|_| rng.gauss()).collect();
        let mask = a.push(&block);
        let before = total;
        total += size as u64;
        for (b, band) in bands.iter().enumerate() {
            let (n, hop) = (band.n as u64, band.hop as u64);
            let newest = |t: u64| (t >= n).then(|| n + (t - n) / hop * hop);
            assert_eq!(
                a.samples_since_frame(b),
                newest(total).map(|e| total - e),
                "band {b} after {total}"
            );
            assert_eq!(
                mask & (1 << b) != 0,
                newest(total) != newest(before),
                "band {b} mask after {total}"
            );
        }
        let newest = bands
            .iter()
            .filter(|b| total >= b.n as u64)
            .map(|b| total - (b.n as u64 + (total - b.n as u64) / b.hop as u64 * b.hop as u64))
            .min();
        assert_eq!(a.samples_since_newest(), newest);
    }
}

#[test]
fn a_column_has_no_value_until_every_band_behind_it_has_a_frame() {
    let fs = 48_000.0;
    let mut a = analyzer(fs, 48, Window::Hann, Averaging::Off);
    let mut out = vec![0.0; a.columns()];
    assert!(!a.column_power(&mut out));
    // Half the longest window: the short bands have frames, the longest has none.
    a.push(&vec![0.1; 16_384]);
    assert!(a.column_power(&mut out));
    let grid = a.config().grid;
    let a1 = a.ladder().bands[1].anchor_hz;
    for (c, p) in out.iter().enumerate() {
        let f = grid.frequency(c);
        if f < a1 {
            assert!(p.is_nan(), "{f} Hz reads before its band has a frame");
        } else {
            assert!(!p.is_nan(), "{f} Hz");
        }
    }
}

/// Matches the independent numpy implementation of the same estimator
/// (`tools/refgen/sets/multires.py`): bands, hops, anchors, blend and per-band FIFO.
#[test]
fn matches_golden() {
    let g = ac2_testkit::golden::GoldenSet::load("multires_spectrum").expect("golden");
    let x = g.f64("x").expect("x");
    let fs = g.scalar("fs_hz").expect("fs");
    let ppo = g.scalar("ppo").expect("ppo") as u32;
    let frames = g.scalar("fifo_frames").expect("frames") as usize;
    let hops = g.f64("hops").expect("hops");
    let ladder = MultiResLadder::new(fs, |n| {
        let b = (MultiResLadder::longest_window(fs) / n).trailing_zeros() as usize;
        hops[b] as usize
    })
    .expect("ladder");
    let anchors: Vec<f64> = ladder.bands.iter().map(|b| b.anchor_hz).collect();
    g.assert_f64("anchor_hz", &anchors);
    let mut a = MultiResAnalyzer::new(
        ladder,
        MultiResConfig {
            window: Window::Hann,
            averaging: Averaging::Fifo { frames },
            grid: MultiResLadder::grid(fs, ppo),
        },
    )
    .expect("analyzer");
    for chunk in x.chunks(1000) {
        a.push(chunk);
    }
    let mut out = vec![0.0; a.columns()];
    assert!(a.column_power(&mut out));
    g.assert_f64("freq_hz", &a.config().grid.frequencies());
    let level: Vec<f64> = out.iter().map(|p| db(*p)).collect();
    g.assert_f64("level_db", &level);
}
