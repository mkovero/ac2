use super::*;
use ac2_testkit::golden::GoldenSet;
use std::f64::consts::TAU;

fn analyzer(fs: f64, n: usize, window: Window, averaging: Averaging) -> SpectrumAnalyzer {
    SpectrumAnalyzer::new(SpectrumConfig {
        fs,
        n,
        hop: n,
        window,
        averaging,
        peak_hold: None,
    })
    .expect("config")
}

/// Deterministic Gaussian noise (xorshift64* + Box–Muller); statistics only, no golden.
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

#[test]
fn column_max_keeps_a_narrow_peak() {
    // A one-bin tone 40 dB over a flat floor, in a column of 50 bins: the column reads
    // the tone, where a mean would read ~23 dB under it.
    let mut p = vec![1e-6; 200];
    p[123] = 1e-2;
    p[7] = f64::NAN;
    let first = [0, 1, 100, 150, 200];
    let cols: Vec<f64> = column_max(&p, &first).collect();
    assert_eq!(cols, [1e-6, 1e-6, 1e-2, 1e-6]);
    // A column of gaps is a gap.
    let cols: Vec<f64> = column_max(&[f64::NAN, 1.0], &[0, 1, 2]).collect();
    assert!(cols[0].is_nan());
    assert_eq!(cols[1], 1.0);
}

#[test]
fn tone_power_factor_matches_amplitude_dbfs() {
    let fs = 48_000.0;
    let n = 4096;
    let mut a = analyzer(fs, n, Window::Hann, Averaging::Off);
    let x: Vec<f64> = (0..n)
        .map(|i| 0.5 * (TAU * 1000.0 * i as f64 / fs).sin())
        .collect();
    a.push(&x);
    let ps = a.average().expect("one frame");
    for k in [10, 85, 86, 400] {
        let via = 10.0 * (ps.tone_power_factor() * ps.folded()[k]).log10();
        assert!((via - ps.amplitude_dbfs(k)).abs() < 1e-9, "{k}");
    }
}

#[test]
fn tone_noise_matches_golden() {
    let g = GoldenSet::load("spectrum_hann_tone_noise").expect("golden");
    let x = g.f64("x").expect("x");
    let n = x.len();
    let mut a = analyzer(48_000.0, n, Window::Hann, Averaging::Off);
    assert_eq!(a.push(&x), 1);
    let s = a.average().expect("frame");
    let f: Vec<f64> = (0..s.bins()).map(|k| s.freq(k)).collect();
    let amp: Vec<f64> = (0..s.bins()).map(|k| s.amplitude_rms(k)).collect();
    let amp_db: Vec<f64> = (0..s.bins()).map(|k| s.amplitude_dbfs(k)).collect();
    let psd: Vec<f64> = (0..s.bins()).map(|k| s.psd(k)).collect();
    let psd_db: Vec<f64> = (0..s.bins()).map(|k| s.psd_db(k)).collect();
    g.assert_f64("freq_hz", &f);
    g.assert_f64("amplitude_rms", &amp);
    g.assert_f64("amplitude_dbfs", &amp_db);
    g.assert_f64("psd", &psd);
    g.assert_f64("psd_db", &psd_db);
}

/// Off-bin tone at a half-bin offset: the spectrum matches the scipy/analytic golden and
/// the peak reads exactly the window's scalloping loss below the tone level.
#[test]
fn half_bin_tone_reads_scalloping_loss() {
    let g = GoldenSet::load("spectrum_offbin_windows").expect("golden");
    let x = g.f64("x").expect("x");
    let fs = g.scalar("fs_hz").expect("fs");
    for (win, key) in [
        (Window::Hann, "hann"),
        (Window::BlackmanHarris4, "blackmanharris4"),
        (Window::FlatTop, "flattop_hft95"),
        (Window::Rectangular, "rectangular"),
    ] {
        let mut a = analyzer(fs, x.len(), win, Averaging::Off);
        a.push(&x);
        let s = a.average().expect("frame");
        let db: Vec<f64> = (0..s.bins()).map(|k| s.amplitude_dbfs(k)).collect();
        g.assert_f64(&format!("{key}_amplitude_dbfs"), &db);
        let peak = db.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let loss = -peak;
        let expected = g.scalar(&format!("{key}_scalloping_db")).expect("loss");
        assert!(
            (loss - expected).abs() < 1e-9,
            "{key}: {loss} vs {expected}"
        );
        assert!(
            (loss - win.max_scalloping_loss_db()).abs() < 0.01,
            "{key}: stated {} vs measured {loss}",
            win.max_scalloping_loss_db()
        );
    }
}

#[test]
fn fft_banding_matches_golden_and_parseval() {
    let g = GoldenSet::load("fft_banding_noise").expect("golden");
    let x = g.f64("x").expect("x");
    let fs = g.scalar("fs_hz").expect("fs");
    let n = x.len();
    let mut a = analyzer(fs, n, Window::Hann, Averaging::Off);
    a.push(&x);
    let s = a.average().expect("frame");
    for (fr, key) in [
        (BandFraction::Octave, "oct"),
        (BandFraction::Third, "third"),
    ] {
        let banding = FftBanding::iec(fr, 15.0, 21_000.0, n, fs);
        let centres: Vec<f64> = banding.bands().iter().map(|b| b.centre_hz).collect();
        g.assert_f64(&format!("{key}_centre_hz"), &centres);
        let status: Vec<f64> = banding
            .bands()
            .iter()
            .map(|b| match b.status {
                BandStatus::Valid => 0.0,
                BandStatus::InsufficientResolution => 1.0,
                BandStatus::AboveNyquist => 2.0,
            })
            .collect();
        g.assert_f64(&format!("{key}_status"), &status);
        let mut p = vec![0.0; banding.len()];
        s.band_powers(&banding, &mut p);
        let p0: Vec<f64> = p
            .iter()
            .map(|v| if v.is_nan() { 0.0 } else { *v })
            .collect();
        g.assert_f64(&format!("{key}_band_power"), &p0);
    }
    // Parseval: bands tiling [0, fs/2] sum to the windowed mean square.
    let mut edges = vec![0.0];
    let third = FftBanding::iec(BandFraction::Third, 100.0, 20_000.0, n, fs);
    edges.extend(third.bands().iter().map(|b| b.lower_hz));
    edges.push(third.bands().last().expect("band").upper_hz);
    edges.push(fs / 2.0);
    let tiling = FftBanding::from_edges(&edges, n, fs);
    assert!(tiling.bands().iter().all(|b| b.status == BandStatus::Valid));
    let mut p = vec![0.0; tiling.len()];
    s.band_powers(&tiling, &mut p);
    let total: f64 = p.iter().sum();
    let expected = g.scalar("windowed_mean_square").expect("ms");
    assert!(
        (total / expected - 1.0).abs() < 1e-9,
        "{total} vs {expected}"
    );
    assert!((s.total_power() / expected - 1.0).abs() < 1e-12);
}

#[test]
fn narrow_and_out_of_range_bands_are_nan_with_reason() {
    let banding = FftBanding::iec(BandFraction::Third, 19.0, 21_000.0, 4096, 44_100.0);
    let b = banding.bands();
    assert_eq!(b[0].status, BandStatus::InsufficientResolution);
    assert_eq!(b.last().expect("band").status, BandStatus::AboveNyquist);
    let x = vec![0.1; 4096];
    let mut a = analyzer(44_100.0, 4096, Window::Hann, Averaging::Off);
    a.push(&x);
    let mut p = vec![0.0; banding.len()];
    a.average().expect("frame").band_powers(&banding, &mut p);
    assert!(p[0].is_nan() && p[p.len() - 1].is_nan());
    assert!(p[10].is_finite());
}

/// White noise: band power is independent of N (statistical tolerance), and equals
/// σ²·B/(fs/2).
#[test]
fn white_noise_band_power_independent_of_n() {
    let fs = 48_000.0;
    let sigma = 0.1;
    let total = 1 << 22;
    let mut rng = Noise(0x0AC2_5EED);
    let x: Vec<f64> = (0..total).map(|_| sigma * rng.gauss()).collect();
    for n in [1024, 4096, 16_384, 65_536] {
        let mut a = analyzer(fs, n, Window::Hann, Averaging::Fifo { frames: total / n });
        a.push(&x);
        let banding = FftBanding::iec(BandFraction::Octave, 250.0, 8000.0, n, fs);
        let mut p = vec![0.0; banding.len()];
        a.average().expect("avg").band_powers(&banding, &mut p);
        for (b, p) in banding.bands().iter().zip(&p) {
            let bw = b.upper_hz - b.lower_hz;
            let expect = sigma * sigma * bw / (fs / 2.0);
            let err_db = 10.0 * (p / expect).log10();
            // Relative standard deviation of a band-power estimate ≈ 1/√(B·T_eff); with
            // non-overlapped Hann frames T_eff is about half the record. Allow 4σ.
            let t_eff = 0.5 * total as f64 / fs;
            let tol_db = 10.0 * (1.0 + 4.0 / (bw * t_eff).sqrt()).log10();
            assert!(
                err_db.abs() < tol_db,
                "N={n} band {:.0}: {err_db:.3} dB (tol {tol_db:.3})",
                b.centre_hz
            );
        }
    }
}

/// Periodic pink noise (1/f power, random phases, period 2²⁰): band power is
/// independent of N and matches the components' power in the band.
#[test]
fn pink_noise_band_power_independent_of_n() {
    let fs = 48_000.0;
    let len = 1usize << 20;
    let mut rng = Noise(0x0AC2_D1CE);
    let mut planner = RealFftPlanner::<f64>::new();
    let inv = planner.plan_fft_inverse(len);
    let mut spec = inv.make_input_vec();
    let df = fs / len as f64;
    // Component k has amplitude a_k ∝ 1/√f_k, so its power a_k²/2 ∝ 1/f_k.
    let comp_power = |k: usize| 1e-3 / (k as f64 * df);
    for (k, c) in spec.iter_mut().enumerate().take(len / 2).skip(1) {
        let amp = (2.0 * comp_power(k)).sqrt();
        *c = Complex64::from_polar(amp * len as f64 / 2.0, TAU * rng.uniform());
    }
    let mut x = inv.make_output_vec();
    inv.process(&mut spec, &mut x).expect("ifft");
    // inverse realfft is unnormalised: x[n] = Σ_k a_k cos(...) with the scaling above.
    for v in &mut x {
        *v /= len as f64;
    }
    for n in [1024, 4096, 16_384, 65_536] {
        let mut a = analyzer(fs, n, Window::Hann, Averaging::Fifo { frames: len / n });
        a.push(&x);
        let banding = FftBanding::iec(BandFraction::Third, 500.0, 8000.0, n, fs);
        let mut p = vec![0.0; banding.len()];
        a.average().expect("avg").band_powers(&banding, &mut p);
        for (b, p) in banding.bands().iter().zip(&p) {
            let k0 = (b.lower_hz / df).ceil() as usize;
            let k1 = (b.upper_hz / df).floor() as usize;
            let expect: f64 = (k0..=k1).map(comp_power).sum();
            let err_db = 10.0 * (p / expect).log10();
            assert!(
                err_db.abs() < 0.2,
                "N={n} band {:.0}: {err_db:.3} dB",
                b.centre_hz
            );
        }
    }
}

#[test]
fn streaming_hop_and_block_independence() {
    let fs = 48_000.0;
    let n = 1024;
    let hop = 256;
    let mut rng = Noise(7);
    let x: Vec<f64> = (0..10_000).map(|_| rng.gauss()).collect();
    let cfg = SpectrumConfig {
        fs,
        n,
        hop,
        window: Window::Hann,
        averaging: Averaging::Fifo { frames: 8 },
        peak_hold: None,
    };
    let mut one = SpectrumAnalyzer::new(cfg).expect("cfg");
    let mut many = SpectrumAnalyzer::new(cfg).expect("cfg");
    let frames = one.push(&x);
    assert_eq!(frames, (x.len() - n) / hop + 1);
    let mut count = 0;
    for chunk in x.chunks(97) {
        count += many.push(chunk);
    }
    assert_eq!(count, frames);
    // The newest frame ended at the same input sample however the input was split.
    let since = (x.len() - n) % hop;
    assert_eq!(one.samples_since_frame(), Some(since as u64));
    assert_eq!(many.samples_since_frame(), Some(since as u64));
    let (a, b) = (one.average().expect("a"), many.average().expect("b"));
    assert_eq!(a.folded(), b.folded());
    // FIFO of 8 equals the mean of the last 8 single frames computed directly.
    let mut direct = vec![0.0; n / 2 + 1];
    for f in 0..8 {
        let end = n + (frames - 1 - f) * hop;
        let mut single = analyzer(fs, n, Window::Hann, Averaging::Off);
        single.push(&x[end - n..end]);
        for (d, v) in direct.iter_mut().zip(single.average().expect("s").folded()) {
            *d += v / 8.0;
        }
    }
    for (d, v) in direct.iter().zip(a.folded()) {
        assert!((d - v).abs() <= 1e-9 * d.abs().max(1e-12), "{d} vs {v}");
    }
}

#[test]
fn exponential_average_converges_to_mean() {
    let fs = 48_000.0;
    let n = 512;
    let mut rng = Noise(11);
    let x: Vec<f64> = (0..n * 2000).map(|_| 0.5 * rng.gauss()).collect();
    let mut a = analyzer(
        fs,
        n,
        Window::Hann,
        Averaging::Exponential {
            time_constant_s: 2.0,
        },
    );
    a.push(&x);
    let s = a.average().expect("avg");
    // White noise PSD σ²/(fs/2); 2 s time constant ≈ 375 frames → ~5 % per bin, so
    // compare the in-band mean.
    let mean: f64 = (10..200).map(|k| s.psd(k)).sum::<f64>() / 190.0;
    let expect = 0.25 / (fs / 2.0);
    assert!((mean / expect - 1.0).abs() < 0.02, "{mean} vs {expect}");
    // First frame is not ramped from zero.
    let mut b = analyzer(
        fs,
        n,
        Window::Hann,
        Averaging::Exponential {
            time_constant_s: 10.0,
        },
    );
    b.push(&x[..n]);
    let mut c = analyzer(fs, n, Window::Hann, Averaging::Off);
    c.push(&x[..n]);
    assert_eq!(
        b.average().expect("b").folded(),
        c.average().expect("c").folded()
    );
}

#[test]
fn peak_hold_holds_then_decays() {
    let fs = 48_000.0;
    let n = 1024;
    let hold_s = 0.5;
    let decay = 20.0;
    let mut a = SpectrumAnalyzer::new(SpectrumConfig {
        fs,
        n,
        hop: n,
        window: Window::Hann,
        averaging: Averaging::Off,
        peak_hold: Some(PeakHold {
            hold_s,
            decay_db_per_s: decay,
        }),
    })
    .expect("cfg");
    let k = 64;
    let f = k as f64 * fs / n as f64;
    let loud: Vec<f64> = (0..n)
        .map(|i| 0.5 * (TAU * f * i as f64 / fs).sin())
        .collect();
    let quiet: Vec<f64> = (0..n)
        .map(|i| 0.005 * (TAU * f * i as f64 / fs).sin())
        .collect();
    a.push(&loud);
    let peak0 = a.peak().expect("peak").amplitude_dbfs(k);
    assert!((peak0 - rms_dbfs(0.5 / SQRT_2)).abs() < 1e-9);
    let dt = n as f64 / fs;
    let frames_held = (hold_s / dt).floor() as usize;
    for _ in 0..frames_held {
        a.push(&quiet);
    }
    assert_eq!(a.peak().expect("p").amplitude_dbfs(k), peak0);
    let extra = 10;
    for _ in 0..extra {
        a.push(&quiet);
    }
    let dropped = peak0 - a.peak().expect("p").amplitude_dbfs(k);
    assert!(
        (dropped - decay * dt * extra as f64).abs() < 1e-9,
        "{dropped}"
    );
    // Decay never goes below the live average.
    for _ in 0..1000 {
        a.push(&quiet);
    }
    let p = a.peak().expect("p").amplitude_dbfs(k);
    let live = a.average().expect("a").amplitude_dbfs(k);
    assert!((p - live).abs() < 1e-9);
}

#[test]
fn dc_and_nyquist_not_doubled() {
    let n = 64;
    let x: Vec<f64> = (0..n)
        .map(|i| 0.25 + if i % 2 == 0 { 0.5 } else { -0.5 })
        .collect();
    let mut a = analyzer(1000.0, n, Window::Rectangular, Averaging::Off);
    a.push(&x);
    let s = a.average().expect("s");
    assert!((s.amplitude_rms(0) - 0.25).abs() < 1e-12);
    assert!((s.amplitude_rms(n / 2) - 0.5).abs() < 1e-12);
}

#[test]
fn invalid_configs_rejected() {
    let base = SpectrumConfig {
        fs: 48_000.0,
        n: 1024,
        hop: 512,
        window: Window::Hann,
        averaging: Averaging::Off,
        peak_hold: None,
    };
    assert!(matches!(
        SpectrumAnalyzer::new(SpectrumConfig { n: 1, ..base }),
        Err(SpectrumError::FftLength(1))
    ));
    assert!(matches!(
        SpectrumAnalyzer::new(SpectrumConfig { hop: 0, ..base }),
        Err(SpectrumError::Hop(0))
    ));
    assert!(matches!(
        SpectrumAnalyzer::new(SpectrumConfig {
            averaging: Averaging::Fifo { frames: 0 },
            ..base
        }),
        Err(SpectrumError::Averaging)
    ));
    assert!(matches!(
        SpectrumAnalyzer::new(SpectrumConfig {
            peak_hold: Some(PeakHold {
                hold_s: -1.0,
                decay_db_per_s: 1.0
            }),
            ..base
        }),
        Err(SpectrumError::PeakHold)
    ));
}
