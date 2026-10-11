//! Harmonic levels and THD: polynomial and Hammerstein systems, the lowest columns,
//! low-frequency roll-off, the sweep's top and the per-order own windows.

use super::*;

#[test]
fn memoryless_polynomial_harmonics_and_thd() {
    let (a2, a3) = (0.2, 1.265);
    let s = spec(ess(50.0, 6000.0, 3.0));
    let (reference, mic) = record(&s, 1, |x| poly(x, a2, a3), 1e-5, 1);
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    let (fund, h2, h3) = analytic(amp(), a2, a3);
    let (h2_db, h3_db) = (20.0 * h2.log10(), 20.0 * h3.log10());
    assert!((h2_db + 40.0).abs() < 0.2 && (h3_db + 50.0).abs() < 0.2);

    // Arrival: the measurement path's delay re the loopback's.
    let want = (MIC_DELAY - REF_DELAY) as f64 / FS;
    assert!((r.arrival_s - want).abs() <= 0.5 / FS, "{}", r.arrival_s);
    assert!(
        (r.reference_db - 20.0 * REF_GAIN.log10()).abs() < 0.02,
        "{}",
        r.reference_db
    );
    assert_eq!(r.repeats, 1);
    assert!(!r.clipped);

    let e2 = max_err(&r, &r.harmonics[0], 100.0, 6000.0 / 2.0 / 1.15, |_| h2_db);
    let e3 = max_err(&r, &r.harmonics[1], 100.0, 6000.0 / 3.0 / 1.15, |_| h3_db);
    eprintln!("memoryless: H2 max error {e2:.3} dB, H3 max error {e3:.3} dB");
    assert!(e2 <= 0.5 && e3 <= 0.5);

    // THD: power sum of H2 and H3.
    let thd_db = 10.0 * (h2 * h2 + h3 * h3).log10();
    let mut e_thd = 0.0f64;
    for (i, &f) in r.frequencies.iter().enumerate() {
        if (100.0..=6000.0 / 3.0 / 1.15).contains(&f) {
            assert!(is_valid(r.thd_db[i], r.thd_floor_db[i]));
            e_thd = e_thd.max((r.thd_db[i] - thd_db).abs());
        }
    }
    eprintln!("memoryless: THD max error {e_thd:.3} dB");
    assert!(e_thd <= 0.5);

    // H4 and H5 do not exist: never reported as distortion.
    for c in &r.harmonics[2..] {
        let valid = c
            .level_db
            .iter()
            .zip(&c.floor_db)
            .filter(|(l, f)| is_valid(**l, **f))
            .count();
        let measured = c.level_db.iter().filter(|v| v.is_finite()).count();
        assert!(
            valid * 50 <= measured,
            "H{}: {valid} of {measured} valid",
            c.order
        );
    }

    // Fundamental: the path gain over the loopback gain, with the cubic's compression.
    let want_db = 20.0 * (MIC_GAIN * fund / REF_GAIN).log10();
    let mut e_lin = 0.0f64;
    for (i, &f) in r.frequencies.iter().enumerate() {
        if (100.0..=5000.0).contains(&f) {
            e_lin = e_lin.max((r.magnitude_db[i] - want_db).abs());
            assert!(
                r.phase_deg[i].abs() < 2.0,
                "phase {} at {f}",
                r.phase_deg[i]
            );
        }
        if !(50.0..=6000.0).contains(&f) {
            assert!(r.magnitude_db[i].is_nan());
        }
    }
    eprintln!("memoryless: fundamental max error {e_lin:.4} dB");
    assert!(e_lin <= 0.1);

    // Below the sweep (and inside the fade-in) and where the harmonic's band reaches the
    // fade-out nothing is reported.
    let at = |f: f64| r.frequencies.iter().position(|&x| x >= f).expect("column");
    assert!(r.harmonics[0].level_db[at(40.0)].is_nan());
    assert!(r.harmonics[0].level_db[at(3100.0)].is_nan());
    assert!(r.harmonics[1].level_db[at(2100.0)].is_nan());
    assert!(r.harmonics[0].level_db[at(2900.0)].is_nan());
    assert!(r.harmonics[0].level_db[at(2850.0)].is_finite());

    // The stored IR shows the linear impulse at 0 and H2 at −L·ln 2.
    let t_of = |i: usize| r.ir_t0_s + i as f64 * r.ir_dt_s;
    let (imax, _) =
        r.ir.iter().enumerate().fold(
            (0, 0.0f64),
            |b, (i, v)| if v.abs() > b.1 { (i, v.abs()) } else { b },
        );
    assert!(t_of(imax).abs() <= r.ir_dt_s + 1.0 / FS);
    assert!(r.ir.len() <= IR_POINTS);
    let h2_t = -r.rate_s * LN_2;
    let near = |t: f64| ((t - r.ir_t0_s) / r.ir_dt_s).round() as usize;
    let around = r.ir_etc_db[near(h2_t) - 3..near(h2_t) + 3]
        .iter()
        .fold(f64::NEG_INFINITY, |a, &v| a.max(v));
    let far = r.ir_etc_db[near(h2_t * 0.85)];
    assert!(
        around > far + 15.0,
        "H2 impulse {around} dB vs {far} dB between"
    );
}

#[test]
fn hammerstein_maps_harmonics_to_the_fundamental_frequency() {
    // A polynomial followed by a low-pass: harmonic k at fundamental f is attenuated by
    // |G(k·f)| while the fundamental is attenuated by |G(f)|.
    let (a2, a3) = (0.2, 1.265);
    let g = LowPass::new(2000.0);
    let s = spec(ess(50.0, 6000.0, 3.0));
    let (reference, mic) = record(&s, 1, |x| g.run(&poly(x, a2, a3)), 1e-7, 2);
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    let (_, h2, h3) = analytic(amp(), a2, a3);
    let g = &g;
    let want = |h: f64, k: f64| move |f: f64| 20.0 * (h * g.gain(k * f) / g.gain(f)).log10();
    let e2 = max_err(&r, &r.harmonics[0], 100.0, 2400.0, want(h2, 2.0));
    let e3 = max_err(&r, &r.harmonics[1], 100.0, 1600.0, want(h3, 3.0));
    eprintln!("hammerstein: H2 max error {e2:.3} dB, H3 max error {e3:.3} dB");
    assert!(e2 <= 1.0 && e3 <= 1.0);
    // The fundamental follows the low-pass.
    for (i, &f) in r.frequencies.iter().enumerate() {
        if (100.0..=5000.0).contains(&f) {
            let w = 20.0 * (MIC_GAIN * analytic(amp(), a2, a3).0 * g.gain(f) / REF_GAIN).log10();
            assert!(
                (r.magnitude_db[i] - w).abs() < 0.2,
                "{f}: {}",
                r.magnitude_db[i]
            );
        }
    }
}

#[test]
fn the_lowest_columns_read_the_harmonic_level() {
    // A capped window (L·ln(6/5) > 100 ms), so the octave above the lowest resolved
    // fundamental, 2/W = 20 Hz, lies inside the sweep.
    let (a2, a3) = (0.2, 1.265);
    let s = SweepSpec {
        grid: LogGrid {
            ppo: 48,
            k_min: -288,
            k_max: 239,
        },
        ..spec(ess(10.0, 20_000.0, 5.5))
    };
    let t = SweepTiming::new(&s).expect("timing");
    assert!((t.window_s() - MAX_WINDOW_S).abs() < 1e-12);
    let (reference, mic) = record(&s, 1, |x| dc_blocked(&poly(x, a2, a3), 1.0), 1e-5, 5);
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    let (_, h2, h3) = analytic(amp(), a2, a3);
    let (h2_db, h3_db) = (20.0 * h2.log10(), 20.0 * h3.log10());
    let f_lo = 2.0 / t.window_s();
    let e2 = max_err(&r, &r.harmonics[0], f_lo, 2.0 * f_lo, |_| h2_db);
    let e3 = max_err(&r, &r.harmonics[1], f_lo, 2.0 * f_lo, |_| h3_db);
    eprintln!("lowest octave: H2 max error {e2:.3} dB, H3 max error {e3:.3} dB");
    assert!(e2 <= 0.5 && e3 <= 0.5);
}

#[test]
fn the_sweep_starts_below_the_asked_band_and_harmonics_read_from_its_start() {
    let (a2, a3) = (0.2, 1.265);
    let f1 = 50.0;
    let s = spec(ess(f1, 6000.0, 3.0));
    let t = SweepTiming::new(&s).expect("timing");
    // At least two octaves lower, a whole number of cycles per L, at the asked rate.
    let e = t.emitted;
    assert!(e.start_hz <= f1 / ONSET_EXTENSION && e.start_hz > f1 / (2.0 * ONSET_EXTENSION));
    let cycles = e.start_hz * t.plan.rate_s;
    assert!((cycles - cycles.round()).abs() < 1e-9);
    let asked = EssPlan::new(&s.ess, FS).expect("plan");
    assert!((t.plan.rate_s - asked.rate_s).abs() < 1e-12);
    // Full level from the asked start; the added octaves stay below it.
    assert!((t.full_level_hz() - f1).abs() < 1e-9);
    let below = ((t.plan.rate_s * (f1 / e.start_hz).ln() * FS) as usize).saturating_sub(1);
    let peak = |r: std::ops::Range<usize>| r.map(|n| t.plan.sample(n).abs()).fold(0.0, f64::max);
    assert!(peak(0..below / 2) < 0.5 && peak(0..t.plan.len) <= 1.0);

    let (reference, mic) = record(&s, 1, |x| dc_blocked(&poly(x, a2, a3), 1.0), 1e-5, 6);
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    let (_, h2, h3) = analytic(amp(), a2, a3);
    let e2 = max_err(&r, &r.harmonics[0], f1, 2.0 * f1, |_| 20.0 * h2.log10());
    let e3 = max_err(&r, &r.harmonics[1], f1, 2.0 * f1, |_| 20.0 * h3.log10());
    eprintln!("from the asked start: H2 max error {e2:.3} dB, H3 max error {e3:.3} dB");
    assert!(e2 <= 0.5 && e3 <= 0.5);
    for (i, &f) in r.frequencies.iter().enumerate() {
        if (f1..=6000.0 / 3.0 / 1.15).contains(&f) {
            assert!(r.thd_db[i].is_finite() && r.thd_floor_db[i].is_finite());
        }
        // The linear response covers the asked band only.
        assert_eq!(
            r.magnitude_db[i].is_finite(),
            (f1..=6000.0).contains(&f),
            "{f}"
        );
    }
}

#[test]
fn a_start_at_the_floor_is_emitted_as_asked() {
    let s = spec(ess(MIN_EMITTED_START_HZ, 20_000.0, 5.0));
    let t = SweepTiming::new(&s).expect("timing");
    assert_eq!(t.emitted, s.ess);
}

#[test]
fn a_dc_blocked_loopback_leaves_the_lowest_harmonics_clean() {
    // An interface's DC blocking high-pass (second order at 2 Hz) on the loopback takes the
    // reference's sub-sonic tail down to the regularisation; the path itself is flat to 1 Hz
    // with a small, known H2. The lowest columns must read that H2, not the low-frequency
    // swell the division leaves around the arrival.
    let s = SweepSpec {
        grid: LogGrid::covering(48, 20.0, 20_000.0),
        ..spec(ess(20.0, 20_000.0, 5.5))
    };
    let x = emitted(&s, 1, 0.37);
    let reference = delayed(&dc_blocked(&dc_blocked(&x, 2.0), 2.0), REF_DELAY, REF_GAIN);
    // H2 at −80 dB re the fundamental.
    let a2 = 2e-4 / amp();
    let mut mic = delayed(&dc_blocked(&poly(&x, a2, 0.0), 1.0), MIC_DELAY, MIC_GAIN);
    Noise(13).add(&mut mic, 1e-7);
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    let (_, h2, _) = analytic(amp(), a2, 0.0);
    let e2 = max_err(&r, &r.harmonics[0], 20.0, 40.0, |_| 20.0 * h2.log10());
    eprintln!("dc-blocked loopback, 20–40 Hz: H2 max error {e2:.2} dB");
    assert!(e2 <= 1.0, "{e2}");
}

#[test]
fn a_linear_path_with_its_own_low_frequency_roll_off_reads_no_h2_at_the_lowest_columns() {
    // The path rolls off a few hertz up (coupling capacitors) behind the DC-blocked loopback:
    // its response in the octaves below the analysed band is large and rings, so a filter that
    // takes those octaves out of the harmonic windows must not spread them back before the
    // arrival, where every harmonic window lies. A linear path has no H2 at all.
    let s = SweepSpec {
        grid: LogGrid::covering(48, 20.0, 20_000.0),
        ..spec(ess(20.0, 20_000.0, 5.5))
    };
    let x = emitted(&s, 1, 0.37);
    let reference = delayed(&dc_blocked(&dc_blocked(&x, 2.0), 2.0), REF_DELAY, REF_GAIN);
    let mut mic = delayed(&dc_blocked(&dc_blocked(&x, 4.0), 4.0), MIC_DELAY, MIC_GAIN);
    Noise(17).add(&mut mic, 1e-8);
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    let worst = r
        .frequencies
        .iter()
        .zip(&r.harmonics[0].level_db)
        .filter(|(f, l)| (20.0..=40.0).contains(*f) && l.is_finite())
        .map(|(_, &l)| l)
        .fold(f64::NEG_INFINITY, f64::max);
    eprintln!("linear path, 20–40 Hz: H2 at most {worst:.1} dBr");
    assert!(worst <= -115.0, "{worst}");
}

/// |H| of [`dc_blocked`] at `f`.
fn dc_blocked_gain(fc: f64, f: f64) -> f64 {
    let a = 1.0 / (1.0 + TAU * fc / FS);
    let z = Complex64::from_polar(1.0, -TAU * f / FS);
    (a * (1.0 - z) / (1.0 - a * z)).norm()
}

#[test]
fn harmonics_under_a_low_frequency_roll_off_read_the_fundamentals_own_gain() {
    // A polynomial followed by a 20 Hz high-pass (a coupling capacitor after the stage that
    // distorts): exact for a sweep, so what the analysis adds is all the error. At 25 Hz the
    // fundamental's power changes 6 dB over 10–40 Hz; read over a wider band of the
    // fundamental than its harmonics are, the ratio comes out high.
    // The sweep starts an octave below the checked band: nearer its start the fade-in drives
    // the harmonics below full level.
    let (a2, a3) = (0.2, 1.265);
    let fc = 20.0;
    let s = SweepSpec {
        grid: LogGrid::covering(48, 20.0, 20_000.0),
        ..spec(ess(10.0, 20_000.0, 5.5))
    };
    let (reference, mic) = record(&s, 1, |x| dc_blocked(&poly(x, a2, a3), fc), 1e-7, 21);
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    let (_, h2, h3) = analytic(amp(), a2, a3);
    let want = |h: f64, k: f64| {
        move |f: f64| 20.0 * (h * dc_blocked_gain(fc, k * f) / dc_blocked_gain(fc, f)).log10()
    };
    // Read over a band of the fundamental k times wider than the harmonic's, the ratio came
    // out 0.2–0.4 dB high throughout 20–40 Hz. What remains is ripple, not a bias.
    let bias = |c: &HarmonicCurve, want: &dyn Fn(f64) -> f64| {
        let d: Vec<f64> = r
            .frequencies
            .iter()
            .zip(&c.level_db)
            .filter(|(f, _)| (20.0..=40.0).contains(*f))
            .map(|(&f, &l)| l - want(f))
            .collect();
        d.iter().sum::<f64>() / d.len() as f64
    };
    let (b2, b3) = (
        bias(&r.harmonics[0], &want(h2, 2.0)),
        bias(&r.harmonics[1], &want(h3, 3.0)),
    );
    let e2 = max_err(&r, &r.harmonics[0], 20.0, 80.0, want(h2, 2.0));
    let e3 = max_err(&r, &r.harmonics[1], 20.0, 80.0, want(h3, 3.0));
    eprintln!(
        "20 Hz roll-off, 20–80 Hz: H2 max error {e2:.3} dB, H3 {e3:.3}; 20–40 Hz mean \
         H2 {b2:+.3} dB, H3 {b3:+.3}"
    );
    assert!(
        b2.abs() <= 0.03 && b3.abs() <= 0.03,
        "bias H2 {b2:.3} dB, H3 {b3:.3}"
    );
    assert!(e2 <= 0.2 && e3 <= 0.2, "H2 {e2:.3} dB, H3 {e3:.3} dB");
}

/// `Σ c[k]·x^k` per sample.
fn polyn(x: &[f64], c: &[f64]) -> Vec<f64> {
    x.iter()
        .map(|&v| c.iter().rev().fold(0.0, |acc, &k| acc * v + k))
        .collect()
}

/// |harmonic k| / |fundamental| at the output of `polyn` driven by `a·sin`, from one period's DFT.
fn harm(c: &[f64], a: f64, k: usize) -> f64 {
    let n = 256;
    let y = polyn(
        &(0..n)
            .map(|i| a * (TAU * i as f64 / n as f64).sin())
            .collect::<Vec<_>>(),
        c,
    );
    let bin = |m: usize| {
        y.iter()
            .enumerate()
            .map(|(i, &v)| Complex64::from_polar(v, -TAU * (m * i) as f64 / n as f64))
            .sum::<Complex64>()
            .norm()
    };
    bin(k) / bin(1)
}

/// A fifth-order polynomial, as in a digital test stage.
const POLY5: [f64; 6] = [0.0, 1.0, 0.002, 0.008, 0.18, 0.5];

#[test]
fn harmonics_near_the_sweeps_top_read_no_higher_than_they_are() {
    // A memoryless polynomial: every harmonic is flat. Near the top the reference fades out,
    // so dividing by it lifts a harmonic its fundamental drove at full level. With the fade
    // L·ln 2/24 the fade starts at f2·2^(−1/24): H5's band at 40 kHz/5 of that has its centre
    // just below it and its upper half inside, and read 1 dB high. Above 9 kHz the stage's own
    // aliases land in the harmonic windows (H3 of 12.3 kHz folds to 96 − 37 kHz, L·ln 2 early).
    let fs = 96_000.0;
    let s = SweepSpec {
        level_dbfs: -13.0,
        sample_rate: fs,
        grid: LogGrid::covering(48, 20.0, 40_000.0),
        ..spec(ess(10.0, 40_000.0, 5.5))
    };
    let t = SweepTiming::new(&s).expect("timing");
    let a = dbfs_to_rms(s.level_dbfs) * std::f64::consts::SQRT_2;
    let mut x = vec![0.0; (0.37 * fs) as usize];
    x.extend((0..t.sweep_samples()).map(|n| a * t.plan.sample(n)));
    x.extend(std::iter::repeat_n(0.0, t.post_roll_samples(fs) + 4_800));
    let mut mic = polyn(&x, &POLY5);
    Noise(5).add(&mut mic, 1e-7);
    let r = analyse_recording(&s, &x, &mic, 1).expect("analysis");
    let mut worst = (0.0, 0, 0.0f64);
    for c in &r.harmonics {
        let want = 20.0 * harm(&POLY5, a, usize::from(c.order)).log10();
        for (&f, &l) in r.frequencies.iter().zip(&c.level_db) {
            if (2_000.0..=9_000.0).contains(&f) && l.is_finite() && (l - want).abs() > worst.2 {
                worst = (f, c.order, (l - want).abs());
            }
        }
    }
    let (f, k, e) = worst;
    eprintln!("top of the sweep: max error {e:.3} dB (H{k} at {f:.0} Hz)");
    assert!(e <= 0.05, "H{k} at {f:.0} Hz: {e:.3} dB");
}

#[test]
fn own_windows_resolve_the_lowest_columns_finer() {
    // The high-pass ahead of a strong x⁵ (as in harmonics_behind…): in the shared 0.1 s
    // window a cell is 10 Hz, 40 % of a 25 Hz fundamental, and the steep harmonics read their
    // mean over it. Each order's own longest window resolves them finer, reaches lower, and
    // pays for it with a floor raised by the longer window.
    let fc = 20.0;
    let run = |lf_harmonics| {
        let s = SweepSpec {
            level_dbfs: -13.0,
            lf_harmonics,
            grid: LogGrid::covering(48, 15.0, 20_000.0),
            ..spec(ess(10.0, 20_000.0, 5.5))
        };
        let t = SweepTiming::new(&s).expect("timing");
        let a = dbfs_to_rms(s.level_dbfs) * std::f64::consts::SQRT_2;
        // Noise well above the far skirts of these strong harmonics, so that each window's
        // floor is the noise it holds (see `GAP_NOISE_FACTOR`).
        let (reference, mic) = record(&s, 1, |x| polyn(&dc_blocked(x, fc), &POLY5), 1e-6, 21);
        let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
        let err = |lo: f64, hi: f64| {
            let mut e = 0.0f64;
            for c in &r.harmonics {
                for (i, &f) in r.frequencies.iter().enumerate() {
                    let l = c.level_db[i];
                    if (lo..hi).contains(&f) && is_valid(l, c.floor_db[i]) {
                        let g = dc_blocked_gain(fc, f);
                        let want = 20.0 * harm(&POLY5, a * g, usize::from(c.order)).log10();
                        e = e.max((l - want).abs());
                    }
                }
            }
            e
        };
        let lowest = r
            .frequencies
            .iter()
            .zip(&r.harmonics[0].level_db)
            .find(|(_, l)| l.is_finite())
            .map_or(f64::NAN, |(&f, _)| f);
        let at = r
            .frequencies
            .iter()
            .position(|&f| f >= 25.0)
            .expect("column");
        (
            t.post_roll_s,
            lowest,
            err(20.0, 25.0),
            err(25.0, 30.0),
            r.harmonics[0].floor_db[at],
            (
                r.unresolved_harmonics.last().map_or(f64::NAN, |x| x.1),
                r.harmonic_window_s.0 + r.harmonic_window_s.1,
            ),
        )
    };
    let (roll_s, low_s, e20_s, e25_s, floor_s, (edge_s, w_s)) = run(LfHarmonics::Standard);
    let (roll_f, low_f, e20_f, e25_f, floor_f, (edge_f, _)) = run(LfHarmonics::Fine);
    // The coarsest harmonic cell in the shared window is H2's, 1/(2W) of fundamental: the
    // distortion curves resolve 1/48 octave from κ(48)/(2W) up (≈ 350 Hz for 0.1 s). Fine's
    // own windows serve the low columns with cells several times narrower.
    let col = 2f64.powf(1.0 / 48.0);
    let want = crate::resolution::resolved_from_hz(1.0 / (2.0 * w_s), 48);
    assert!(
        edge_s / want < col && want / edge_s < col,
        "{edge_s} vs {want}"
    );
    assert!(
        edge_f < 0.5 * edge_s,
        "harmonics resolved from {edge_s:.0} → {edge_f:.0} Hz"
    );
    eprintln!(
        "standard: from {low_s:.1} Hz, 20–25 Hz {e20_s:.3} dB, 25–30 Hz {e25_s:.3}, H2 floor \
         {floor_s:.1} dBr at 25 Hz, post-roll {roll_s:.2} s; fine: from {low_f:.1} Hz, \
         {e20_f:.3}, {e25_f:.3}, {floor_f:.1}, {roll_f:.2} s"
    );
    assert!(
        e20_s > 0.5 && e20_f < 0.3,
        "20–25 Hz: {e20_s:.3} → {e20_f:.3} dB"
    );
    assert!(
        e25_f < 0.2 && e25_f < e25_s,
        "25–30 Hz: {e25_s:.3} → {e25_f:.3} dB"
    );
    assert!(
        low_s >= 20.0 && low_f < 17.0,
        "lowest column {low_s:.1} → {low_f:.1} Hz"
    );
    // H2's own window is about 4.7 times the shared one: its floor is that much higher.
    assert!(
        (floor_f - floor_s - 6.7).abs() < 2.0,
        "{floor_s:.1} → {floor_f:.1} dBr"
    );
    assert!(roll_f >= 4.0 * 0.45 && roll_s < roll_f);
}

#[test]
fn a_noise_burst_at_a_harmonics_moment_is_no_distortion() {
    // A linear path whose input hums for a moment (0.4 s of 44 Hz) just as the sweep passes
    // 22 Hz, the post-roll quiet: the H2 window at 22 Hz holds the hum, and a floor read only
    // after the sweep would book it as H2. Read beside the window, the floor holds it too.
    let s = SweepSpec {
        level_dbfs: -30.0,
        ..spec(ess(10.0, 20_000.0, 5.5))
    };
    let t = SweepTiming::new(&s).expect("timing");
    let moment = 0.37 + t.plan.rate_s * (22.0 / t.emitted.start_hz).ln();
    let hum = |mic: &mut Vec<f64>| {
        let (c, half) = (moment * FS + MIC_DELAY as f64, 0.2 * FS);
        for (i, v) in mic.iter_mut().enumerate() {
            let x = (i as f64 - c) / half;
            if x.abs() < 1.0 {
                *v += 3e-5 * (0.5 + 0.5 * (PI * x).cos()) * (TAU * 44.0 * i as f64 / FS).sin();
            }
        }
    };
    let (reference, quiet) = record(&s, 1, <[f64]>::to_vec, 1e-6, 5);
    let mut hummed = quiet.clone();
    hum(&mut hummed);
    let a = analyse_recording(&s, &reference, &quiet, 1).expect("analysis");
    let b = analyse_recording(&s, &reference, &hummed, 1).expect("analysis");
    let col = |f: f64| a.frequencies.iter().position(|&x| x >= f).expect("column");
    let (i, j) = (col(22.0), col(1000.0));
    let (h2a, h2b) = (&a.harmonics[0], &b.harmonics[0]);
    eprintln!(
        "H2 at 22 Hz: quiet {:.1} (floor {:.1}), hum {:.1} (floor {:.1}) dBr; floor at 1 kHz {:.1} → {:.1}",
        h2a.level_db[i],
        h2a.floor_db[i],
        h2b.level_db[i],
        h2b.floor_db[i],
        h2a.floor_db[j],
        h2b.floor_db[j]
    );
    assert!(
        h2b.level_db[i] > h2a.level_db[i] + 20.0,
        "the hum reaches the H2 window"
    );
    assert!(
        !is_valid(h2b.level_db[i], h2b.floor_db[i]),
        "the hum reads as H2"
    );
    assert!(
        (h2a.floor_db[j] - h2b.floor_db[j]).abs() < 0.1,
        "away from the hum the floor stays"
    );
}

#[test]
fn fine_reads_as_standard_above_its_own_windows() {
    // A memoryless x⁵ at 48 kHz aliases: products above fs/2 fold down and land at positive
    // lags, lower in frequency the later the lag. Fine's longer post-roll must not move the
    // shared noise windows (or the linear gate) to other lags, or where both settings read
    // the shared window they would report different floors.
    let s = |lf_harmonics| SweepSpec {
        level_dbfs: -13.0,
        lf_harmonics,
        grid: LogGrid::covering(48, 15.0, 20_000.0),
        ..spec(ess(10.0, 20_000.0, 5.5))
    };
    let run = |lf_harmonics| {
        let s = s(lf_harmonics);
        let (reference, mic) = record(&s, 1, |x| polyn(x, &POLY5), 1e-7, 21);
        analyse_recording(&s, &reference, &mic, 1).expect("analysis")
    };
    let (st, fi) = (run(LfHarmonics::Standard), run(LfHarmonics::Fine));
    // Fine reads an order in its own window where the shared band was widened to its
    // resolution: below this harmonic frequency.
    let half = 2f64.powf(DISTORTION_BAND_OCT / 2.0);
    let window_s = SweepTiming::new(&s(LfHarmonics::Standard))
        .expect("timing")
        .window_s();
    let own_below = DISTORTION_MIN_CELLS / window_s / (half - 1.0 / half);
    assert_eq!(st.frequencies, fi.frequencies);
    let (mut d_mag, mut d_level, mut d_floor) = (0.0f64, 0.0f64, 0.0f64);
    for (i, &f) in st.frequencies.iter().enumerate() {
        if f >= 100.0 {
            d_mag = d_mag.max((st.magnitude_db[i] - fi.magnitude_db[i]).abs());
        }
        for (a, b) in st.harmonics.iter().zip(&fi.harmonics) {
            if f64::from(a.order) * f < 1.05 * own_below || !a.floor_db[i].is_finite() {
                continue;
            }
            d_level = d_level.max((a.level_db[i] - b.level_db[i]).abs());
            d_floor = d_floor.max((a.floor_db[i] - b.floor_db[i]).abs());
        }
    }
    eprintln!(
        "shared-window columns, fine − standard: magnitude {d_mag:.4} dB, harmonics \
         {d_level:.4}, floors {d_floor:.3}"
    );
    assert!(
        d_mag < 0.01 && d_level < 0.01 && d_floor < 0.01,
        "{d_mag:.4} {d_level:.4} {d_floor:.3}"
    );
}
