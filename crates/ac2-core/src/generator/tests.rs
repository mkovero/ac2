use super::*;
use crate::window::Window;

const FS: f64 = 48_000.0;

fn cfg(signal: Signal, level_dbfs: f64) -> GeneratorConfig {
    GeneratorConfig {
        signal,
        sample_rate: FS,
        seed: 0x00AC_2001,
        band: BandLimit::NONE,
        level_dbfs,
        ceiling_dbfs: f64::INFINITY,
    }
}

fn render(g: &mut Generator, n: usize) -> Vec<f32> {
    let mut out = vec![0.0; n];
    // Odd block size so block boundaries land everywhere.
    for chunk in out.chunks_mut(509) {
        g.fill(chunk);
    }
    out
}

/// Renders past the initial fade-in ramp, then `n` samples.
fn steady(g: &mut Generator, n: usize) -> Vec<f32> {
    let skip = g.ramp_samples() as usize + 1;
    render(g, skip);
    render(g, n)
}

fn rms_dbfs(x: &[f32]) -> f64 {
    let p = x.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / x.len() as f64;
    rms_to_dbfs(p.sqrt())
}

/// One-sided Welch PSD (Hann, 50 % overlap) in FS²/Hz per design Q4.
fn welch_psd(x: &[f32], n: usize) -> Vec<f64> {
    let w = Window::Hann.coefficients(n);
    let s2: f64 = w.iter().map(|v| v * v).sum();
    let mut planner = RealFftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(n);
    let mut acc = vec![0.0; n / 2 + 1];
    let mut buf = vec![0.0; n];
    let mut spec = fft.make_output_vec();
    let mut count = 0;
    let mut start = 0;
    while start + n <= x.len() {
        for (i, b) in buf.iter_mut().enumerate() {
            *b = f64::from(x[start + i]) * w[i];
        }
        fft.process(&mut buf, &mut spec).expect("fft");
        for (a, s) in acc.iter_mut().zip(&spec) {
            *a += s.norm_sqr();
        }
        count += 1;
        start += n / 2;
    }
    acc.iter()
        .enumerate()
        .map(|(k, a)| {
            let c = if k == 0 || k == n / 2 { 1.0 } else { 2.0 };
            c * a / (count as f64 * FS * s2)
        })
        .collect()
}

/// Mean PSD in dB over octave bands centred at 31.25·2^k Hz up to 16 kHz.
fn octave_means_db(psd: &[f64], n: usize) -> Vec<(f64, f64)> {
    let bin_hz = FS / n as f64;
    (0..10)
        .map(|k| {
            let fc = 31.25 * 2f64.powi(k);
            let lo = (fc / SQRT_2 / bin_hz).ceil() as usize;
            let hi = (fc * SQRT_2 / bin_hz).floor() as usize;
            let mean = psd[lo..=hi].iter().sum::<f64>() / (hi - lo + 1) as f64;
            (fc, 10.0 * mean.log10())
        })
        .collect()
}

#[test]
fn rms_accuracy_per_signal_type() {
    let band = BandLimit {
        highpass_hz: Some(40.0),
        lowpass_hz: Some(10_000.0),
        order: FilterOrder::Fourth,
    };
    let cases: [(&str, Signal, BandLimit, usize); 7] = [
        ("white", Signal::White, BandLimit::NONE, 1 << 20),
        ("white band-limited", Signal::White, band, 1 << 20),
        ("pink", Signal::Pink, BandLimit::NONE, 1 << 22),
        ("pink band-limited", Signal::Pink, band, 1 << 22),
        (
            "periodic pink",
            Signal::PeriodicPink { period: 1 << 16 },
            BandLimit::NONE,
            1 << 16,
        ),
        (
            "periodic pink band-limited",
            Signal::PeriodicPink { period: 1 << 16 },
            band,
            1 << 16,
        ),
        (
            "sine 1 kHz",
            Signal::Sine { freq_hz: 1000.0 },
            BandLimit::NONE,
            48_000,
        ),
    ];
    for (name, signal, band, n) in cases {
        for level in [-20.0, -40.0] {
            let mut c = cfg(signal, level);
            c.band = band;
            let mut g = Generator::new(&c).expect(name);
            let x = steady(&mut g, n);
            let err = rms_dbfs(&x) - level;
            println!(
                "{name} @ {level} dBFS: error {err:+.4} dB, crest {:.3}",
                g.crest_factor()
            );
            assert!(err.abs() <= 0.05, "{name}: RMS error {err:+.4} dB");
        }
    }
}

#[test]
fn ess_rms_matches_level_on_constant_envelope() {
    let ess = EssConfig {
        start_hz: 20.0,
        end_hz: 20_000.0,
        duration_s: 2.0,
        fade_in_s: 0.05,
        fade_out_s: 0.01,
    };
    let mut g = Generator::new(&cfg(Signal::Ess(ess), -10.0)).expect("ess");
    let plan = EssPlan::new(&ess, FS).expect("plan");
    let x = render(&mut g, plan.len);
    let body = &x[(0.1 * FS) as usize..plan.len - (0.05 * FS) as usize];
    let err = rms_dbfs(body) - (-10.0);
    println!("ESS constant-envelope RMS error {err:+.4} dB");
    assert!(err.abs() <= 0.05, "ESS RMS error {err:+.4} dB");
    assert!(g.is_finished());
    assert!(render(&mut g, 100).iter().all(|&v| v == 0.0));
}

fn pink_deviation_db(fs: f64) -> f64 {
    let filt = PinkFilter::design(fs);
    let (lo, hi) = PinkFilter::fit_band(fs);
    let dev: Vec<f64> = (0..400)
        .map(|i| {
            let f = lo * (hi / lo).powf(i as f64 / 399.0);
            20.0 * filt.response(f, fs).norm().log10() + 10.0 * f.log10()
        })
        .collect();
    let max = dev.iter().copied().fold(f64::MIN, f64::max);
    let min = dev.iter().copied().fold(f64::MAX, f64::min);
    (max - min) / 2.0
}

#[test]
fn pink_filter_response_within_spec_at_all_rates() {
    for fs in [44_100.0, 48_000.0, 88_200.0, 96_000.0, 176_400.0, 192_000.0] {
        let dev = pink_deviation_db(fs);
        println!("pink filter @ {fs} Hz: ±{dev:.3} dB from −3 dB/oct");
        assert!(dev <= PINK_FLATNESS_DB, "fs {fs}: ±{dev:.3} dB");
    }
}

#[test]
fn pink_spectral_slope_from_averaged_fft() {
    let mut g = Generator::new(&cfg(Signal::Pink, -20.0)).expect("pink");
    let x = steady(&mut g, 1 << 21);
    let n = 1 << 14;
    let bands = octave_means_db(&welch_psd(&x, n), n);
    for pair in bands.windows(2) {
        let slope = pair[1].1 - pair[0].1;
        println!(
            "pink {:>6.0} → {:>6.0} Hz: {slope:+.3} dB/oct",
            pair[0].0, pair[1].0
        );
        assert!(
            (slope + 3.01).abs() <= 0.5,
            "slope {slope:+.3} dB/oct at {} Hz",
            pair[0].0
        );
    }
}

#[test]
fn white_is_flat_from_averaged_fft() {
    let mut g = Generator::new(&cfg(Signal::White, -20.0)).expect("white");
    let x = steady(&mut g, 1 << 20);
    let n = 1 << 14;
    let bands = octave_means_db(&welch_psd(&x, n), n);
    let mean = bands.iter().map(|b| b.1).sum::<f64>() / bands.len() as f64;
    // Expected density for the requested level: σ² / (fs/2).
    let expected = 10.0 * (dbfs_to_rms(-20.0).powi(2) / (FS / 2.0)).log10();
    println!("white mean PSD {mean:.3} dB, expected {expected:.3} dB");
    assert!((mean - expected).abs() < 0.1);
    for (fc, db) in bands {
        println!("white {fc:>6.0} Hz: {:+.3} dB", db - mean);
        assert!((db - mean).abs() <= 0.3, "{fc} Hz: {:+.3} dB", db - mean);
    }
}

#[test]
fn periodic_pink_repeats_exactly() {
    let p = 1 << 14;
    let mut g = Generator::new(&cfg(Signal::PeriodicPink { period: p }, -20.0)).expect("pp");
    let x = steady(&mut g, 3 * p);
    for n in 0..2 * p {
        assert_eq!(x[n].to_bits(), x[n + p].to_bits(), "sample {n}");
    }
}

#[test]
fn periodic_pink_spectrum_is_exactly_pink() {
    let p = 1 << 16;
    let (table, _) = periodic_pink_table(p, FS, 7, &BandLimit::NONE).expect("table");
    let mut planner = RealFftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(p);
    let mut buf: Vec<f64> = table.iter().map(|&v| f64::from(v)).collect();
    let mut spec = fft.make_output_vec();
    fft.process(&mut buf, &mut spec).expect("fft");
    let bin_hz = FS / p as f64;
    let k_ref = (1000.0 / bin_hz) as usize;
    let ref_level = spec[k_ref].norm_sqr() * k_ref as f64;
    for k in ((PINK_CORNER_HZ / bin_hz).ceil() as usize..p / 2).step_by(97) {
        let rel = spec[k].norm_sqr() * k as f64 / ref_level;
        // f32 storage limits agreement to roughly −100 dB relative error per bin.
        assert!((10.0 * rel.log10()).abs() < 0.01, "bin {k}: {rel}");
    }
    assert!(spec[0].norm() < 1e-3);
}

#[test]
fn period_rule() {
    // ±1 s search (W = 2 s) and 1 s tail at 48 kHz → 2^18 (PLAN §5.4 example).
    assert_eq!(periodic_period(2.0, 1.0, 48_000.0, 0), Ok(1 << 18));
    for (w, t, fs, min) in [
        (2.0, 1.0, 44_100.0, 0),
        (0.5, 0.25, 96_000.0, 0),
        (0.2, 0.0, 48_000.0, 1 << 16),
        (2.0, 1.0, 192_000.0, 0),
        // Exactly a power of two: P must be strictly larger.
        (1.0, 0.0, 65_536.0 / 1.0, 0),
    ] {
        let p = periodic_period(w, t, fs, min).expect("period");
        let span = ((w + t) * fs).ceil() as usize;
        assert!(p.is_power_of_two());
        assert!(p > span, "P {p} must exceed W+T {span}");
        assert!(p >= min);
        assert!(
            p / 2 <= span.max(min) || p == MIN_PERIOD,
            "P {p} not minimal"
        );
    }
    assert_eq!(periodic_period(1.0, 0.0, 65_536.0, 0), Ok(1 << 17));
    assert!(periodic_period(-1.0, 0.0, 48_000.0, 0).is_err());
    assert!(periodic_period(400.0, 0.0, 48_000.0, 0).is_err());
    assert_eq!(
        Generator::new(&cfg(Signal::PeriodicPink { period: 3000 }, -20.0)).err(),
        Some(GeneratorError::InvalidPeriod)
    );
}

#[test]
fn ess_with_inverse_yields_band_limited_impulse() {
    let ess = EssConfig {
        start_hz: 40.0,
        end_hz: 16_000.0,
        duration_s: 1.0,
        fade_in_s: 0.02,
        fade_out_s: 0.002,
    };
    let level = -12.0;
    let mut g = Generator::new(&cfg(Signal::Ess(ess), level)).expect("ess");
    let plan = EssPlan::new(&ess, FS).expect("plan");
    let sweep: Vec<f64> = render(&mut g, plan.len)
        .iter()
        .map(|&v| f64::from(v))
        .collect();
    let inv = EssInverse::new(&ess, FS, level).expect("inverse");
    let y = inv.deconvolve(&sweep);
    let (peak_idx, peak) =
        y.iter().enumerate().fold(
            (0, 0.0f64),
            |(i, m), (j, &v)| if v.abs() > m { (j, v.abs()) } else { (i, m) },
        );
    assert_eq!(peak_idx, inv.latency, "impulse at the sweep latency");
    // An ideal band-limited impulse over [f1, f2] has peak 2(f2 − f1)/fs.
    let ideal = 2.0 * (ess.end_hz - ess.start_hz) / FS;
    println!("ESS impulse peak {peak:.4} (ideal band-limited {ideal:.4})");
    assert!((peak / ideal - 1.0).abs() < 0.05);
    // Energy is concentrated around the peak.
    let total: f64 = y.iter().map(|v| v * v).sum();
    let near: f64 = y[peak_idx - 48..=peak_idx + 48].iter().map(|v| v * v).sum();
    println!("ESS energy within ±1 ms: {:.4}", near / total);
    assert!(near / total > 0.99);
    // Flat magnitude in band: FFT of a window around the impulse.
    let n = 1 << 13;
    let mut buf: Vec<f64> = y[peak_idx - n / 2..peak_idx + n / 2].to_vec();
    let mut planner = RealFftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(n);
    let mut spec = fft.make_output_vec();
    fft.process(&mut buf, &mut spec).expect("fft");
    let bin_hz = FS / n as f64;
    let mut worst = 0.0f64;
    for (k, s) in spec.iter().enumerate() {
        let f = k as f64 * bin_hz;
        if f >= 2.0 * ess.start_hz && f <= ess.end_hz / 1.25 {
            worst = worst.max((20.0 * s.norm().log10()).abs());
        }
        if f >= 1.1 * ess.end_hz {
            assert!(20.0 * s.norm().log10() < -20.0, "{f} Hz not attenuated");
        }
    }
    println!("ESS in-band magnitude deviation ±{worst:.3} dB");
    assert!(worst < 0.3, "in-band ripple {worst:.3} dB");
}

#[test]
fn levels_that_would_clip_are_refused() {
    match Generator::new(&cfg(Signal::Sine { freq_hz: 1000.0 }, 0.5)) {
        Err(GeneratorError::WouldClip { max_dbfs, .. }) => assert!(max_dbfs.abs() < 1e-9),
        other => panic!("expected refusal, got {other:?}"),
    }
    assert!(Generator::new(&cfg(Signal::Sine { freq_hz: 1000.0 }, 0.0)).is_ok());

    // Uniform white: crest √3 → max 20·log10(√2/√3) = −1.76 dBFS.
    match Generator::new(&cfg(Signal::White, -1.0)) {
        Err(GeneratorError::WouldClip { max_dbfs, .. }) => {
            assert!((max_dbfs - (-1.761)).abs() < 0.001, "{max_dbfs}")
        }
        other => panic!("expected refusal, got {other:?}"),
    }
    match Generator::new(&cfg(Signal::Pink, -10.0)) {
        Err(GeneratorError::WouldClip { max_dbfs, .. }) => {
            assert!((max_dbfs - max_level_for_crest(FILTERED_NOISE_CREST)).abs() < 1e-9)
        }
        other => panic!("expected refusal, got {other:?}"),
    }
    // Periodic pink reports its exact crest.
    let p = Generator::new(&cfg(Signal::PeriodicPink { period: 1 << 16 }, -30.0)).expect("pp");
    let max = p.max_level_dbfs();
    assert!(Generator::new(&cfg(Signal::PeriodicPink { period: 1 << 16 }, max)).is_ok());
    assert!(matches!(
        Generator::new(&cfg(Signal::PeriodicPink { period: 1 << 16 }, max + 0.01)),
        Err(GeneratorError::WouldClip { .. })
    ));

    // Run-time requests are refused too and leave the level unchanged.
    let g = Generator::new(&cfg(Signal::White, -20.0)).expect("white");
    let ctl = g.level_control();
    assert!(matches!(
        ctl.set_level_dbfs(0.0),
        Err(GeneratorError::WouldClip { .. })
    ));
    assert!((ctl.level_dbfs() - (-20.0)).abs() < 1e-9);
    assert_eq!(
        ctl.set_level_dbfs(f64::NAN),
        Err(GeneratorError::NonFiniteLevel)
    );

    // Global ceiling.
    let mut c = cfg(Signal::Sine { freq_hz: 1000.0 }, -10.0);
    c.ceiling_dbfs = -20.0;
    assert_eq!(
        Generator::new(&c).err(),
        Some(GeneratorError::AboveCeiling {
            requested_dbfs: -10.0,
            ceiling_dbfs: -20.0
        })
    );
}

#[test]
fn noise_at_max_level_does_not_clip() {
    let mut g = Generator::new(&cfg(Signal::Pink, -40.0)).expect("pink");
    g.level_control()
        .set_level_dbfs(g.max_level_dbfs())
        .expect("max level");
    let x = steady(&mut g, 1 << 22);
    let rms = 10f64.powf(rms_dbfs(&x) / 20.0) * FRAC_1_SQRT_2;
    let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    println!(
        "pink at max level {:.2} dBFS: observed crest {:.2}",
        g.max_level_dbfs(),
        f64::from(peak) / rms
    );
    assert_eq!(g.clipped_samples(), 0);
}

#[test]
fn level_changes_ramp_without_steps() {
    // 100 Hz sine: the largest natural sample-to-sample change at gain g is g·√2·2π·f/fs.
    // A linear ramp over R samples adds at most |Δg|·√2/R per sample (documented bound).
    let f = 100.0;
    let mut g = Generator::new(&cfg(Signal::Sine { freq_hz: f }, -40.0)).expect("sine");
    let ctl = g.level_control();
    let r = f64::from(g.ramp_samples());
    assert_eq!(g.ramp_samples(), 960);
    let mut prev = 0.0f32;
    let mut check = |x: &[f32], g_max: f64, dg: f64| {
        let bound = g_max * SQRT_2 * TAU * f / FS + dg * SQRT_2 / r + 1e-6;
        for &v in x {
            assert!(
                f64::from((v - prev).abs()) <= bound,
                "step {} > {bound}",
                (v - prev).abs()
            );
            prev = v;
        }
    };
    let low = dbfs_to_rms(-40.0);
    let high = dbfs_to_rms(0.0);
    check(&render(&mut g, 2000), low, low);
    ctl.set_level_dbfs(0.0).expect("0 dBFS");
    check(&render(&mut g, 2000), high, high - low);
    ctl.set_muted(true);
    check(&render(&mut g, 2000), high, high);
    assert!(g.is_silent());
    ctl.set_muted(false);
    check(&render(&mut g, 2000), high, high);
    // Per-sample gain change during a ramp is exactly |Δg| / R.
    ctl.set_level_dbfs(-6.0).expect("level");
    let g0 = g.current_gain();
    render(&mut g, 1);
    let g1 = g.current_gain();
    assert!(((g0 - g1) - (high - dbfs_to_rms(-6.0)) / r).abs() < 1e-12);
}

#[test]
fn fade_out_reaches_silence_in_20_ms() {
    let mut g = Generator::new(&cfg(Signal::Pink, -20.0)).expect("pink");
    steady(&mut g, 4800);
    g.fade_out();
    let x = render(&mut g, 960);
    assert!(x[0] != 0.0);
    assert!(g.is_silent());
    assert!(render(&mut g, 4800).iter().all(|&v| v == 0.0));
}

#[test]
fn deterministic_and_block_size_independent() {
    for signal in [
        Signal::White,
        Signal::Pink,
        Signal::PeriodicPink { period: 1 << 12 },
    ] {
        let mut a = Generator::new(&cfg(signal, -20.0)).expect("a");
        let mut b = Generator::new(&cfg(signal, -20.0)).expect("b");
        let mut whole = vec![0.0; 10_000];
        a.fill(&mut whole);
        let mut parts = vec![0.0; 10_000];
        for chunk in parts[..100].chunks_mut(1) {
            b.fill(chunk);
        }
        for chunk in parts[100..].chunks_mut(733) {
            b.fill(chunk);
        }
        assert_eq!(whole, parts, "{signal:?}");
        let mut other = cfg(signal, -20.0);
        other.seed ^= 1;
        let mut c = Generator::new(&other).expect("c");
        let mut diff = vec![0.0; 10_000];
        c.fill(&mut diff);
        assert_ne!(whole, diff, "{signal:?} must depend on the seed");
    }
}

#[test]
fn invalid_configurations_are_refused() {
    let mut c = cfg(Signal::Sine { freq_hz: 1000.0 }, -20.0);
    c.band.highpass_hz = Some(100.0);
    assert_eq!(
        Generator::new(&c).err(),
        Some(GeneratorError::BandLimitNotApplicable)
    );
    let mut c = cfg(Signal::White, -20.0);
    c.band = BandLimit {
        highpass_hz: Some(1000.0),
        lowpass_hz: Some(500.0),
        order: FilterOrder::Second,
    };
    assert_eq!(Generator::new(&c).err(), Some(GeneratorError::InvalidBand));
    assert_eq!(
        Generator::new(&cfg(Signal::Sine { freq_hz: 30_000.0 }, -20.0)).err(),
        Some(GeneratorError::InvalidFrequency)
    );
    let mut c = cfg(Signal::White, -20.0);
    c.sample_rate = 1000.0;
    assert_eq!(
        Generator::new(&c).err(),
        Some(GeneratorError::InvalidSampleRate)
    );
}
