//! The linear response: gating, refusals, timing, room parameters, arrival and phase,
//! and the reference level.

use super::*;

#[test]
fn a_gate_shortens_the_linear_window() {
    let s = SweepSpec {
        gate_s: Some(0.005),
        ..spec(ess(100.0, 10_000.0, 1.0))
    };
    let (reference, mic) = record(&s, 1, |x| x.to_vec(), 1e-6, 3);
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    assert!((r.linear_window_s.1 - 0.005).abs() < 1.5 / FS);
    assert!(r.ir_t0_s < r.harmonic_window_s.0.mul_add(-1.0, -r.rate_s * 5f64.ln()) + 1e-3);
    let at_1k = r
        .frequencies
        .iter()
        .position(|&f| f >= 1000.0)
        .expect("column");
    assert!((r.magnitude_db[at_1k] - 20.0 * (MIC_GAIN / REF_GAIN).log10()).abs() < 0.1);
    // The short window's cell `1/W` is wider than a 1/48-octave column below κ(48)/W
    // (≈ 3.5 kHz for 20 ms): one range from the sweep's start up to there.
    let w = r.linear_window_s.0 + r.linear_window_s.1;
    let edge = crate::resolution::resolved_from_hz(1.0 / w, 48);
    assert_eq!(
        r.unresolved_response.len(),
        1,
        "{:?}",
        r.unresolved_response
    );
    let (lo, hi) = r.unresolved_response[0];
    let col = 2f64.powf(1.0 / 48.0);
    assert!(lo >= 100.0 / col && lo <= 100.0 * col, "{lo}");
    assert!(hi / edge < col && edge / hi < col, "{hi} vs {edge}");
    // Ungated, the window runs to the noise window: under 100 Hz, at the sweep's start.
    let ungated =
        analyse_recording(&spec(ess(100.0, 10_000.0, 1.0)), &reference, &mic, 1).expect("analysis");
    assert!(
        ungated
            .unresolved_response
            .iter()
            .all(|r| r.1 < 100.0 * col),
        "{:?}",
        ungated.unresolved_response
    );
}

#[test]
fn refusals() {
    let s = spec(ess(50.0, 6000.0, 1.0));
    let (reference, mic) = record(&s, 1, |x| x.to_vec(), 0.0, 4);
    // No sweep on the reference.
    let silent = vec![0.0; reference.len()];
    assert!(matches!(
        analyse_recording(&s, &silent, &mic, 1),
        Err(SweepError::NoReference { .. })
    ));
    // Asked for two repeats, recorded one.
    assert!(matches!(
        analyse_recording(&s, &reference, &mic, 2),
        Err(SweepError::Incomplete {
            found: 1,
            expected: 2
        })
    ));
    // The recording stops inside the post-roll.
    let cut = reference.len() - SweepTiming::new(&s).expect("t").post_roll_samples(FS) / 2;
    assert!(matches!(
        analyse_recording(&s, &reference[..cut], &mic[..cut], 1),
        Err(SweepError::Incomplete { found: 0, .. })
    ));
    // A silent measurement input.
    assert_eq!(
        analyse_recording(&s, &reference, &silent, 1),
        Err(SweepError::NoSignal)
    );
    assert_eq!(
        SweepTiming::new(&SweepSpec { max_order: 1, ..s }),
        Err(SweepError::BadOrder)
    );
    assert_eq!(
        SweepTiming::new(&SweepSpec {
            gate_s: Some(0.0),
            ..s
        }),
        Err(SweepError::BadGate)
    );
}

#[test]
fn timing_follows_the_sweep() {
    let s = spec(ess(20.0, 20_000.0, 3.0));
    let t = SweepTiming::new(&s).expect("timing");
    // L rounded so that f1·L is an integer.
    let fl = 20.0 * t.plan.rate_s;
    assert!((fl - fl.round()).abs() < 1e-9);
    assert!((t.harmonic_time_s(2) + t.plan.rate_s * LN_2).abs() < 1e-12);
    // H5's window: 10 % of the H5–H6 gap before, 90 % of the H4–H5 gap after.
    let l = t.plan.rate_s;
    assert!((t.pre_s - 0.1 * l * 1.2f64.ln()).abs() < 1e-12);
    assert!((t.post_s - 0.9 * l * 1.25f64.ln()).abs() < 1e-12);
    assert!(
        (t.window_s() / l - 0.219).abs() < 1e-3,
        "{}",
        t.window_s() / l
    );
    assert_eq!(t.post_roll_s, MIN_POST_ROLL_S);
    // A long sweep needs a longer silence after it.
    let long = SweepTiming::new(&spec(ess(20.0, 20_000.0, 60.0))).expect("timing");
    assert!(long.post_roll_s >= 4.0 * long.window_s() - 1e-9);
    // A long sweep's window is capped (the floor then falls with L) and rises over a third
    // of it before t_k.
    assert!((long.window_s() - MAX_WINDOW_S).abs() < 1e-12);
    assert!((long.pre_s - MAX_WINDOW_S / 3.0).abs() < 1e-12);
    assert!((db_to_percent(-40.0) - 1.0).abs() < 1e-12);
    assert!(is_valid(-40.0, -46.0) && !is_valid(-40.0, -45.0));
}

/// Linear convolution by FFT.
fn convolve(x: &[f64], h: &[f64]) -> Vec<f64> {
    let n = (x.len() + h.len()).next_power_of_two();
    let (a, b) = (fft_forward(x, n), fft_forward(h, n));
    let y = fft_inverse(a.iter().zip(&b).map(|(p, q)| p * q).collect(), n);
    y[..x.len()].to_vec()
}

/// A reverberant room through the whole chain (sweep, loopback, deconvolution, averaging):
/// the room parameters of the measured IR are those of the room's own IR, analysed
/// directly, within 0.5 % (decay times per band; 3 % broadband, whose band the sweep
/// limits) and 0.3 dB (C80: the onset moves by a sample with the band); the tail option
/// lengthens the silence the IR is taken from.
#[test]
fn a_room_keeps_its_parameters_through_the_sweep() {
    let t60 = 0.5;
    let mut noise = Noise(11);
    let room: Vec<f64> = (0..(0.9 * FS) as usize)
        .map(|i| {
            let t = i as f64 / FS;
            let direct = if i == 0 { 3.0 } else { 0.0 };
            direct + 0.05 * 10f64.powf(-3.0 * t / t60) * noise.gauss()
        })
        .collect();
    // The truth needs time before the arrival as the measurement has: the band filters run
    // backwards put the direct sound's band energy there.
    let padded: Vec<f64> = std::iter::repeat_n(0.0, (0.1 * FS) as usize)
        .chain(room.iter().copied())
        .collect();
    let truth = crate::room::analyse(&padded, FS, -0.1, (100.0, 8000.0));
    let s = SweepSpec {
        tail_s: Some(1.2),
        ..spec(ess(50.0, 12_000.0, 2.0))
    };
    let t = SweepTiming::new(&s).expect("timing");
    assert!((t.post_roll_s - 1.2).abs() < 1e-12);
    let (reference, mic) = record(&s, 1, |x| convolve(x, &room), 1e-7, 5);
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    assert!(r.room_end_s > 1.0 && r.room_end_s < 1.2, "{}", r.room_end_s);
    let pairs = std::iter::once((&r.room.broadband, &truth.broadband))
        .chain(r.room.octave.iter().filter_map(|m| {
            truth
                .octave
                .iter()
                .find(|x| x.centre_hz == m.centre_hz)
                .map(|x| (m, x))
        }))
        .collect::<Vec<_>>();
    assert!(pairs.len() >= 6, "{} bands", pairs.len());
    for (got, want) in pairs {
        let name = got.centre_hz.unwrap_or(0.0);
        for (g, w) in [
            (got.edt_s, want.edt_s),
            (got.t20_s, want.t20_s),
            (got.t30_s, want.t30_s),
        ] {
            let (g, w) = (g.expect("measured"), w.expect("truth"));
            // Broadband differs a little by construction: the sweep excites 50 Hz–12 kHz.
            let tol = if got.centre_hz.is_some() { 0.005 } else { 0.03 };
            assert!((g / w - 1.0).abs() < tol, "{name} Hz: {g} vs {w}");
        }
        let (g, w) = (got.c80_db.expect("C80"), want.c80_db.expect("C80"));
        assert!((g - w).abs() < 0.3, "{name} Hz C80: {g} vs {w}");
        // The onset is the arrival (the direct sound), whatever the chain's latencies.
        assert!(got.onset_s.abs() < 2e-3, "{name} Hz onset {}", got.onset_s);
    }
    assert!(matches!(
        SweepTiming::new(&SweepSpec {
            tail_s: Some(f64::NAN),
            ..s
        }),
        Err(SweepError::BadTail)
    ));
}

/// `x` delayed by `delay` samples, a fraction included, exactly: a linear phase on the DFT
/// of the zero-padded record (the padding keeps the circular shift from wrapping).
fn fractionally_delayed(x: &[f64], delay: f64, gain: f64) -> Vec<f64> {
    let n = 2 * x.len().next_power_of_two();
    let mut spec = fft_forward(x, n);
    let last = spec.len() - 1;
    for (k, z) in spec.iter_mut().enumerate() {
        // A fractional delay at Nyquist is not real; the sweep has nothing there.
        *z *= if k == last {
            Complex64::new(0.0, 0.0)
        } else {
            gain * Complex64::from_polar(1.0, -TAU * k as f64 * delay / n as f64)
        };
    }
    let mut y = fft_inverse(spec, n);
    y.truncate(x.len());
    y
}

/// Arrival error, samples, of a sweep through an exact `REF_DELAY`-relative fractional delay.
fn fractional_arrival_error(fs: f64, f2: f64, phi: f64, snr_db: Option<f64>) -> (f64, f64) {
    let s = SweepSpec {
        sample_rate: fs,
        ..spec(ess(50.0, f2, 1.0))
    };
    let t = SweepTiming::new(&s).expect("timing");
    let amp = dbfs_to_rms(s.level_dbfs) * std::f64::consts::SQRT_2;
    let mut x = vec![0.0; (0.37 * fs) as usize];
    x.extend((0..t.sweep_samples()).map(|n| amp * t.plan.sample(n)));
    x.extend(std::iter::repeat_n(
        0.0,
        t.post_roll_samples(fs) + (0.05 * fs) as usize,
    ));
    let reference = delayed(&x, REF_DELAY, REF_GAIN);
    let mut mic = fractionally_delayed(&x, MIC_DELAY as f64 + phi, MIC_GAIN);
    if let Some(snr) = snr_db {
        let rms = dbfs_to_rms(s.level_dbfs) * MIC_GAIN * 10f64.powf(-snr / 20.0);
        Noise(phi.to_bits() ^ fs.to_bits()).add(&mut mic, rms);
    }
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    let expected = (MIC_DELAY - REF_DELAY) as f64 + phi;
    let err = r.arrival_s * fs - expected;
    // A pure delay referred to its own arrival has no phase left.
    let at = |f: f64| {
        let i = r.frequencies.iter().position(|&c| c >= f).expect("column");
        r.phase_deg[i]
    };
    let phase = at(1000.0).abs().max(at(10_000.0).abs());
    (err, phase)
}

#[test]
fn slow_a_fractional_delay_reads_as_a_fractional_arrival() {
    for (fs, f2) in [
        (48_000.0, 20_000.0),
        (96_000.0, 20_000.0),
        (96_000.0, 40_000.0),
    ] {
        for phi in [0.0, 0.1, 0.25, 0.5, 0.73] {
            let (clean, phase) = fractional_arrival_error(fs, f2, phi, None);
            let (noisy, _) = fractional_arrival_error(fs, f2, phi, Some(40.0));
            eprintln!(
                "fs {fs} f2 {f2} phi {phi}: clean {clean:+.5}, 40 dB {noisy:+.5} sample, \
                 phase {phase:.4}°"
            );
            assert!(clean.abs() < 0.005, "fs {fs} f2 {f2} phi {phi}: {clean}");
            assert!(noisy.abs() < 0.05, "fs {fs} f2 {f2} phi {phi}: {noisy}");
            assert!(phase < 0.5, "fs {fs} f2 {f2} phi {phi}: phase {phase}°");
        }
    }
}

/// RBJ second-order high-pass, Q = 1/√2: the low-frequency corner of a converter path.
struct HighPass {
    b: [f64; 3],
    a: [f64; 2],
}

impl HighPass {
    fn new(fc: f64) -> Self {
        let w = TAU * fc / FS;
        let alpha = w.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let c = w.cos();
        let a0 = 1.0 + alpha;
        Self {
            b: [(1.0 + c) / 2.0 / a0, -(1.0 + c) / a0, (1.0 + c) / 2.0 / a0],
            a: [-2.0 * c / a0, (1.0 - alpha) / a0],
        }
    }

    fn run(&self, x: &[f64]) -> Vec<f64> {
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        x.iter()
            .map(|&v| {
                let y = self.b[0] * v + self.b[1] * x1 + self.b[2] * x2
                    - self.a[0] * y1
                    - self.a[1] * y2;
                (x2, x1, y2, y1) = (x1, v, y1, y);
                y
            })
            .collect()
    }

    /// Phase, radians.
    fn phase(&self, f: f64) -> f64 {
        let z = Complex64::from_polar(1.0, -TAU * f / FS);
        let num = self.b[0] + self.b[1] * z + self.b[2] * z * z;
        let den = 1.0 + self.a[0] * z + self.a[1] * z * z;
        (num / den).arg()
    }

    /// Group delay, s.
    fn group_delay(&self, f: f64) -> f64 {
        let df = 1e-3;
        -(self.phase(f + df) - self.phase(f - df)) / (TAU * 2.0 * df)
    }
}

#[test]
fn low_frequency_phase_is_free_of_bin_ripple() {
    // A 3 Hz high-pass leads by ≈ 8° at 20 Hz and delays by ≈ 1.7 ms; the default gate
    // (≈ 0.9 s) makes the linear spectrum's bins wider than the 48-per-octave columns there,
    // so the phase must be read at each column's centre for the group delay to hold.
    let s = SweepSpec {
        grid: LogGrid {
            ppo: 48,
            k_min: -288,
            k_max: 239,
        },
        ..spec(ess(10.0, 20_000.0, 5.5))
    };
    let hp = HighPass::new(3.0);
    let x = emitted(&s, 1, 0.37);
    let reference = delayed(&x, REF_DELAY, REF_GAIN);
    let mut mic = fractionally_delayed(&hp.run(&x), MIC_DELAY as f64 + 3.7e-6 * FS, MIC_GAIN);
    Noise(11).add(&mut mic, 1e-5);
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    let cols: Vec<usize> = (0..r.frequencies.len())
        .filter(|&i| (16.0..=40.0).contains(&r.frequencies[i]))
        .collect();
    assert!(cols.len() > 60);
    // Residual against the analytic phase, less the line a misplaced arrival leaves.
    let err: Vec<(f64, f64)> = cols
        .iter()
        .map(|&i| {
            let f = r.frequencies[i];
            let e = wrap_deg(r.phase_deg[i] - hp.phase(f).to_degrees());
            (f, e)
        })
        .collect();
    let slope =
        err.iter().map(|(f, e)| f * e).sum::<f64>() / err.iter().map(|(f, _)| f * f).sum::<f64>();
    let ripple = err
        .iter()
        .fold(0.0f64, |a, (f, e)| a.max((e - slope * f).abs()));
    // Neighbour (central) difference of the unwrapped phase against the analytic delay.
    let mut worst_gd = 0.0f64;
    for w in cols.windows(3) {
        let (a, c, b) = (w[0], w[1], w[2]);
        let dphi = wrap_deg(r.phase_deg[b] - r.phase_deg[a]).to_radians();
        let gd = -dphi / (TAU * (r.frequencies[b] - r.frequencies[a]));
        let want = hp.group_delay(r.frequencies[c]);
        worst_gd = worst_gd.max((gd / want - 1.0).abs());
    }
    eprintln!(
        "phase ripple {ripple:.5}°, arrival slope {slope:.2e}°/Hz, group delay within {:.1} %",
        100.0 * worst_gd
    );
    assert!(ripple < 0.005, "phase ripple {ripple}°");
    assert!(
        worst_gd < 0.03,
        "group delay off by {:.1} %",
        100.0 * worst_gd
    );
}

#[test]
fn reference_level_is_the_mid_band_gain_under_hf_roll_off() {
    // A loopback whose gain falls towards the top of the band (a two-tap mean: cos(π f / fs),
    // −3 dB at fs/4): the level it passes on is its mid-band gain, not the matched-filter
    // peak, which weights the rolled-off top of the band most.
    let s = spec(ess(50.0, 20_000.0, 3.0));
    let x = emitted(&s, 1, 0.37);
    let d = delayed(&x, REF_DELAY, REF_GAIN);
    let reference: Vec<f64> = (0..d.len())
        .map(|i| 0.5 * (d[i] + if i > 0 { d[i - 1] } else { 0.0 }))
        .collect();
    let mic = delayed(&x, MIC_DELAY, MIC_GAIN);
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    // the two-tap power gain averaged per octave over the log-middle half of the full-level band
    let t = SweepTiming::new(&s).expect("timing");
    let f_full = t.full_level_hz();
    let f_top = (20_000.0 * (-s.ess.fade_out_s / t.plan.rate_s).exp()).min(0.45 * FS);
    let (lo, hi) = (
        f_full * (f_top / f_full).powf(0.25),
        f_full * (f_top / f_full).powf(0.75),
    );
    let n = 10_000;
    let mean = (0..n)
        .map(|i| {
            let f = lo * (hi / lo).powf((i as f64 + 0.5) / n as f64);
            (std::f64::consts::PI * f / FS).cos().powi(2)
        })
        .sum::<f64>()
        / n as f64;
    let want = 20.0 * REF_GAIN.log10() + 10.0 * mean.log10();
    assert!(
        want < 20.0 * REF_GAIN.log10() - 0.05,
        "the roll-off must reach the band: {want}"
    );
    assert!(
        (r.reference_db - want).abs() < 0.05,
        "reference {} dB, mid-band gain {want} dB",
        r.reference_db
    );
}
