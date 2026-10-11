//! Power spectral density of the fixed and multi-resolution spectra: matches the numpy
//! goldens (`tools/refgen/sets/density.py`), reads white noise flat, pink noise falling
//! 3 dB per octave and a sine at its level less `10·lg(noise bandwidth)`, and — the reason
//! density exists — reads the same noise the same on a 65 536-point spectrum and on the
//! multi-resolution columns.

use std::f64::consts::TAU;

use ac2_core::grid::LogGrid;
use ac2_core::multires::{MultiResAnalyzer, MultiResConfig, MultiResLadder};
use ac2_core::spectrum::{Averaging, SpectrumAnalyzer, SpectrumConfig, SpectrumLevel};
use ac2_core::window::Window;
use ac2_testkit::golden::GoldenSet;

fn db(p: f64) -> f64 {
    10.0 * p.log10()
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

/// The daemon's hop rule (largest power of two ≤ fs/30, clamped to n/8 … n/2).
fn display_hop(fs: f64) -> impl Fn(usize) -> usize {
    move |n| {
        let target = ((fs / 30.0) as usize).max(64);
        let pow2 = 1usize << (usize::BITS - 1 - target.leading_zeros());
        pow2.clamp((n / 8).max(1), (n / 2).max(1))
    }
}

fn fixed(fs: f64, n: usize, hop: usize, averaging: Averaging) -> SpectrumAnalyzer {
    SpectrumAnalyzer::new(SpectrumConfig {
        fs,
        n,
        hop,
        window: Window::Hann,
        averaging,
        peak_hold: None,
    })
    .expect("fixed")
}

fn fixed_density(a: &SpectrumAnalyzer) -> Vec<f64> {
    let k = a.power_factor(SpectrumLevel::Density);
    a.average()
        .expect("a spectrum")
        .folded()
        .iter()
        .map(|f| f * k)
        .collect()
}

fn multi(ladder: MultiResLadder, grid: LogGrid, averaging: Averaging) -> MultiResAnalyzer {
    MultiResAnalyzer::new(
        ladder,
        MultiResConfig {
            window: Window::Hann,
            averaging,
            grid,
            level: SpectrumLevel::Density,
        },
    )
    .expect("multi")
}

fn columns(a: &MultiResAnalyzer) -> Vec<f64> {
    let mut out = vec![0.0; a.columns()];
    assert!(a.column_power(&mut out));
    out
}

/// Power ratios `ratio` (linear) averaged over third-octave bands (centres 1000·2^(k/3))
/// between `lo` and `hi`, in dB: `(centre, mean)`. A band's mean of power, not of dB,
/// so a noise's scatter does not bias it low.
fn third_octaves(f: &[f64], ratio: &[f64], lo: f64, hi: f64) -> Vec<(f64, f64)> {
    (-30..=14)
        .map(|k| 1000.0 * 2f64.powf(f64::from(k) / 3.0))
        .filter(|c| *c >= lo && *c <= hi)
        .filter_map(|c| {
            let h = 2f64.powf(1.0 / 6.0);
            let v: Vec<f64> = f
                .iter()
                .zip(ratio)
                .filter(|(f, _)| **f >= c / h && **f < c * h)
                .map(|(_, r)| *r)
                .collect();
            (!v.is_empty()).then(|| (c, db(v.iter().sum::<f64>() / v.len() as f64)))
        })
        .collect()
}

#[test]
fn fixed_density_matches_golden() {
    let g = GoldenSet::load("spectrum_density").expect("golden");
    let fs = g.scalar("fs_hz").expect("fs");
    let n = g.scalar("n").expect("n") as usize;
    let hop = g.scalar("hop").expect("hop") as usize;
    let frames = g.scalar("fifo_frames").expect("frames") as usize;
    let run = |x: &[f64]| {
        let mut a = fixed(fs, n, hop, Averaging::Fifo { frames });
        for c in x.chunks(700) {
            a.push(c);
        }
        assert!(
            (a.noise_bandwidth_hz() - g.scalar("enbw_hz").expect("enbw")).abs() < 1e-9,
            "ENBW"
        );
        fixed_density(&a)
    };
    let d_a = run(&g.f64("x_a").expect("x_a"));
    let d_pink = run(&g.f64("x_pink").expect("x_pink"));
    g.assert_f64(
        "density_a_db",
        &d_a.iter().map(|p| db(*p)).collect::<Vec<_>>(),
    );
    g.assert_f64(
        "density_pink_db",
        &d_pink.iter().map(|p| db(*p)).collect::<Vec<_>>(),
    );

    // A bin-centred sine reads its level less 10·lg(ENBW · fs / N) at its bin.
    let k = g.scalar("sine_bin").expect("bin") as usize;
    let expect = g.scalar("sine_level_db").expect("level") - db(g.scalar("enbw_hz").expect("B"));
    assert!(
        (db(d_a[k]) - expect).abs() < 0.01,
        "sine: {:.3} vs {expect:.3} dBFS/Hz",
        db(d_a[k])
    );

    // White noise is flat at 4σ²/fs and pink falls 3 dB per octave from its 1 kHz value,
    // in third-octave means from 1 kHz (≥ 4 bins) up, the sine's bins left out: about 3σ of
    // a band's scatter there (32 frames at 50 % overlap). The long-run tests below hold the
    // slopes tighter and lower down.
    let f = g.f64("freq_hz").expect("f");
    let white = 10f64.powf(g.scalar("white_density_db").expect("white") / 10.0);
    let f_sine = k as f64 * fs / n as f64;
    let (fw, rw): (Vec<f64>, Vec<f64>) = f
        .iter()
        .zip(&d_a)
        .filter(|(f, _)| (**f - f_sine).abs() > 8.0 * fs / n as f64)
        .map(|(f, p)| (*f, p / white))
        .unzip();
    for (c, m) in third_octaves(&fw, &rw, 1000.0, 20_000.0) {
        assert!(m.abs() < 1.0, "white at {c:.0} Hz: {m:+.2} dB");
    }
    let pink_1k = 10f64.powf(g.scalar("pink_density_1khz_db").expect("pink") / 10.0);
    let rp: Vec<f64> = d_pink
        .iter()
        .zip(&f)
        .map(|(p, f)| p / (pink_1k * 1000.0 / f.max(1e-9)))
        .collect();
    for (c, m) in third_octaves(&f, &rp, 1000.0, 20_000.0) {
        assert!(m.abs() < 1.0, "pink at {c:.0} Hz: {m:+.2} dB");
    }
}

#[test]
fn multires_density_matches_golden() {
    let g = GoldenSet::load("multires_density").expect("golden");
    let fs = g.scalar("fs_hz").expect("fs");
    let ppo = g.scalar("ppo").expect("ppo") as u32;
    let frames = g.scalar("fifo_frames").expect("frames") as usize;
    let hops = g.f64("hops").expect("hops");
    let ladder = || {
        MultiResLadder::new(fs, |n| {
            let b = (MultiResLadder::longest_window(fs) / n).trailing_zeros() as usize;
            hops[b] as usize
        })
        .expect("ladder")
    };
    let grid = MultiResLadder::grid(fs, ppo);
    let run = |x: &[f64]| {
        let mut a = multi(ladder(), grid, Averaging::Fifo { frames });
        for c in x.chunks(1000) {
            a.push(c);
        }
        columns(&a)
    };
    let f = grid.frequencies();
    g.assert_f64("freq_hz", &f);
    let l = ladder();
    let bw: Vec<f64> = f.iter().map(|f| l.noise_bandwidth_hz(*f, 1.5)).collect();
    g.assert_f64("noise_bandwidth_hz", &bw);
    let d_a = run(&g.f64("x_a").expect("x_a"));
    let d_pink = run(&g.f64("x_pink").expect("x_pink"));
    g.assert_f64(
        "density_a_db",
        &d_a.iter().map(|p| db(*p)).collect::<Vec<_>>(),
    );
    g.assert_f64(
        "density_pink_db",
        &d_pink.iter().map(|p| db(*p)).collect::<Vec<_>>(),
    );

    // The sine sits below the first anchor, where a column reads one bin of band 0: its
    // level less 10·lg(ENBW · Δf₀).
    let f_sine = g.scalar("sine_hz").expect("sine");
    let c = (0..f.len())
        .min_by(|&i, &j| (f[i] - f_sine).abs().total_cmp(&(f[j] - f_sine).abs()))
        .expect("column");
    let expect = g.scalar("sine_level_db").expect("level") - db(bw[c]);
    assert!(
        (db(d_a[c]) - expect).abs() < 0.01,
        "sine: {:.3} vs {expect:.3} dBFS/Hz",
        db(d_a[c])
    );

    // White flat, pink falling 3 dB per octave, in third-octave means (the sine's octave
    // left out of the white), from 250 Hz up: 16 frames of the longest band span two of
    // its windows, so lower down a band's scatter is several dB. About 3σ above.
    let white = 10f64.powf(g.scalar("white_density_db").expect("white") / 10.0);
    let (fw, rw): (Vec<f64>, Vec<f64>) = f
        .iter()
        .zip(&d_a)
        .filter(|(f, _)| (**f / f_sine).log2().abs() > 0.5)
        .map(|(f, p)| (*f, p / white))
        .unzip();
    for (c, m) in third_octaves(&fw, &rw, 250.0, 5000.0) {
        assert!(m.abs() < 1.5, "white at {c:.0} Hz: {m:+.2} dB");
    }
    let expected = g.f64("pink_expected_db").expect("pink");
    let rp: Vec<f64> = d_pink
        .iter()
        .zip(&expected)
        .map(|(p, e)| p / 10f64.powf(e / 10.0))
        .collect();
    for (c, m) in third_octaves(&f, &rp, 250.0, 5000.0) {
        assert!(m.abs() < 1.5, "pink at {c:.0} Hz: {m:+.2} dB");
    }
}

/// White noise reads its density flat on the multi-resolution columns, with no step at
/// any anchor (where tone level rises 3 dB per octave, `multires_tests.rs`).
#[test]
fn multires_white_noise_density_is_flat_through_every_crossover() {
    let fs = 24_000.0;
    let sigma = 0.1;
    let ladder = MultiResLadder::new(fs, display_hop(fs)).expect("ladder");
    let n0 = ladder.bands[0];
    let anchors: Vec<f64> = ladder.bands.iter().map(|b| b.anchor_hz).collect();
    let grid = MultiResLadder::grid(fs, 96);
    let mut a = multi(ladder, grid, Averaging::Fifo { frames: 640 });
    let mut rng = Noise(0x0AC2_D353);
    let x: Vec<f64> = (0..n0.n + 640 * n0.hop)
        .map(|_| sigma * rng.gauss())
        .collect();
    for c in x.chunks(4800) {
        a.push(c);
    }
    let d = columns(&a);
    let f = grid.frequencies();
    let white = 4.0 * sigma * sigma / fs;
    let ratio: Vec<f64> = d.iter().map(|p| p / white).collect();
    // The tone-level test's bounds (`multires_tests.rs`): the same estimator, rescaled.
    for (c, m) in third_octaves(&f, &ratio, 30.0, 10_000.0) {
        assert!(m.abs() < 0.45, "{c:.0} Hz: {m:+.2} dB");
    }
    for anchor in anchors.iter().filter(|a| **a < 10_000.0) {
        let c = f.iter().position(|f| f >= anchor).expect("column");
        let below = db(ratio[c - 16..c].iter().sum::<f64>() / 16.0);
        let above = db(ratio[c..c + 16].iter().sum::<f64>() / 16.0);
        assert!(
            (below - above).abs() < 0.4,
            "step at {anchor:.0} Hz: {below:+.2} → {above:+.2}"
        );
    }
}

/// `len` samples of noise whose density is `4σ²/fs · (1000 / f)^slope` (0 white, 1 pink),
/// shaped in the frequency domain over the whole (circular) signal.
fn coloured(len: usize, sigma: f64, slope: f64, fs: f64, seed: u64) -> Vec<f64> {
    let mut rng = Noise(seed);
    let mut x: Vec<f64> = (0..len).map(|_| sigma * rng.gauss()).collect();
    if slope == 0.0 {
        return x;
    }
    let mut planner = realfft::RealFftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(len);
    let inv = planner.plan_fft_inverse(len);
    let mut spec = fwd.make_output_vec();
    fwd.process(&mut x, &mut spec).expect("fft");
    for (k, z) in spec.iter_mut().enumerate() {
        let f = k as f64 * fs / len as f64;
        *z *= if k == 0 {
            0.0
        } else {
            (1000.0 / f).powf(slope / 2.0) / len as f64
        };
    }
    if let Some(z) = spec.last_mut() {
        z.im = 0.0;
    }
    inv.process(&mut spec, &mut x).expect("ifft");
    x
}

/// The operator's comparison: the same noise on a 65 536-point spectrum and on the
/// multi-resolution columns reads the same density at every column (the fixed bins whose
/// centres a column covers averaged; the bin under it where it covers none), white and
/// pink alike, and both read the analytic density.
#[test]
fn fixed_65536_and_multires_density_agree_at_every_column() {
    let fs = 48_000.0;
    let sigma = 0.1;
    let len = (fs * 60.0) as usize;
    let all = Averaging::Exponential {
        time_constant_s: 1e9,
    };
    let n = 65_536;
    let grid = MultiResLadder::grid(fs, 48);
    let df = fs / n as f64;
    for (slope, seed) in [(0.0, 0x0AC2_D354), (1.0, 0x0AC2_D355)] {
        let x = coloured(len, sigma, slope, fs, seed);
        let mut fx = fixed(fs, n, n / 8, all);
        let mut mr = multi(
            MultiResLadder::new(fs, display_hop(fs)).expect("ladder"),
            grid,
            all,
        );
        for c in x.chunks(1024) {
            fx.push(c);
            mr.push(c);
        }
        let fd = fixed_density(&fx);
        let md = columns(&mr);
        let (mut f_used, mut diff, mut fixed_vs_true, mut multi_vs_true) =
            (vec![], vec![], vec![], vec![]);
        for (c, &m) in md.iter().enumerate().take(grid.len()) {
            let (lo, hi) = grid.edges(c);
            let f = grid.frequency(c);
            if f < 20.0 {
                continue;
            }
            let first = (lo / df).ceil() as usize;
            let end = ((hi / df).ceil() as usize).min(fd.len());
            let bins = if end > first {
                &fd[first..end]
            } else {
                let k = (f / df).round() as usize;
                &fd[k..k + 1]
            };
            let fixed_col = bins.iter().sum::<f64>() / bins.len() as f64;
            let truth = 4.0 * sigma * sigma / fs * (1000.0 / f).powf(slope);
            f_used.push(f);
            diff.push(m / fixed_col);
            fixed_vs_true.push(fixed_col / truth);
            multi_vs_true.push(m / truth);
        }
        let worst = |lo: f64, hi: f64| {
            f_used
                .iter()
                .zip(&diff)
                .filter(|(f, _)| **f >= lo && **f < hi)
                .map(|(f, r)| (*f, db(*r)))
                .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                .expect("columns")
        };
        let rms = (diff.iter().map(|r| db(*r).powi(2)).sum::<f64>() / diff.len() as f64).sqrt();
        let (lf, hf) = (worst(20.0, 1000.0), worst(1000.0, 24_000.0));
        let bands = |v: &[f64]| {
            third_octaves(&f_used, v, 25.0, 20_000.0)
                .into_iter()
                .map(|(_, m)| m.abs())
                .fold(0.0, f64::max)
        };
        let (b_diff, b_fixed, b_multi) =
            (bands(&diff), bands(&fixed_vs_true), bands(&multi_vs_true));
        eprintln!(
            "slope {slope}: multi − fixed per column: rms {rms:.3} dB, worst {:+.2} dB at {:.0} Hz \
             (< 1 kHz), {:+.2} dB at {:.0} Hz (≥ 1 kHz); third-octave means: multi − fixed \
             ≤ {b_diff:.3} dB, fixed − true ≤ {b_fixed:.3} dB, multi − true ≤ {b_multi:.3} dB",
            lf.1, lf.0, hf.1, hf.0
        );
        // Per column the two estimators scatter independently: below 1 kHz a column reads
        // one or two bins of a 1.4 s / 0.68 s window averaged over 60 s (σ of the
        // difference ≈ 0.5 dB); above, many bins of the fixed spectrum against a short
        // window's many frames.
        assert!(lf.1.abs() < 1.5, "{:+.2} dB at {:.0} Hz", lf.1, lf.0);
        assert!(hf.1.abs() < 0.5, "{:+.2} dB at {:.0} Hz", hf.1, hf.0);
        assert!(rms < 0.25, "rms {rms:.3} dB");
        // No offset anywhere: third-octave means agree with each other and with the truth.
        assert!(b_diff < 0.25, "multi − fixed band mean {b_diff:.3} dB");
        assert!(b_fixed < 0.3, "fixed − true band mean {b_fixed:.3} dB");
        assert!(b_multi < 0.3, "multi − true band mean {b_multi:.3} dB");
    }
}
