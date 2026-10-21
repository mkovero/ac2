use super::*;

/// 48 columns per octave, 20 Hz … 20 kHz, as a transfer measurement's grid.
fn grid() -> Vec<f64> {
    (0..)
        .map(|i| 20.0 * 2f64.powf(f64::from(i) / 48.0))
        .take_while(|f| *f <= 20_000.0)
        .collect()
}

fn delayed(f: f64, t: f64) -> Complex64 {
    Complex64::from_polar(1.0, -TAU * f * t)
}

fn s(f: f64, fc: f64) -> Complex64 {
    Complex64::new(0.0, f / fc)
}

/// Butterworth 2nd order low-pass at `fc`.
fn bw2(f: f64, fc: f64) -> Complex64 {
    let s = s(f, fc);
    1.0 / (s * s + 2f64.sqrt() * s + 1.0)
}

/// Linkwitz–Riley 4th order low and high pass at `fc`: in phase at every frequency.
fn lr4(f: f64, fc: f64) -> (Complex64, Complex64) {
    let lp = bw2(f, fc) * bw2(f, fc);
    let s2 = s(f, fc) * s(f, fc);
    (lp, lp * s2 * s2)
}

/// Linkwitz–Riley 2nd order: low and high pass 180° apart at every frequency.
fn lr2(f: f64, fc: f64) -> (Complex64, Complex64) {
    let p = 1.0 / ((s(f, fc) + 1.0) * (s(f, fc) + 1.0));
    (p, p * s(f, fc) * s(f, fc))
}

fn band(lo: f64, hi: f64) -> Params {
    Params {
        lo,
        hi,
        max_delay: 0.05,
    }
}

fn rel(got: f64, want: f64) -> f64 {
    ((got - want) / want).abs()
}

#[test]
fn a_known_delay_is_found_and_says_which_to_delay() {
    let f = grid();
    // A arrives 3.7 ms after B: B is the one to delay.
    let a: Vec<_> = f.iter().map(|&f| delayed(f, 0.0037)).collect();
    let b: Vec<_> = f.iter().map(|_| Complex64::new(0.5, 0.0)).collect();
    let fit = fit(&f, &a, &b, None, &band(60.0, 120.0)).expect("the band holds data");
    assert!(rel(fit.best.delay, 0.0037) < 1e-6, "{fit:?}");
    assert!(!fit.best.inverted);
    assert!(fit.best.residual_deg < 1e-3);
    assert!(fit.best.score > 0.999_999);
    assert!(fit.after.mean_db > -1e-6 && fit.after.worst_db > -1e-6);
    assert!(fit.before.worst_db < -3.0, "{:?}", fit.before);
    let other = fit
        .other_polarity
        .expect("the other polarity is a candidate");
    assert!(other.inverted && other.score < fit.best.score);
}

#[test]
fn an_inverted_system_delayed_the_other_way() {
    let f = grid();
    // B arrives 2 ms after A, inverted: A is the one to delay, and to flip.
    let a: Vec<_> = f.iter().map(|_| Complex64::new(1.0, 0.0)).collect();
    let b: Vec<_> = f.iter().map(|&f| -delayed(f, 0.002)).collect();
    let fit = fit(&f, &a, &b, None, &band(200.0, 800.0)).expect("the band holds data");
    assert!(rel(fit.best.delay, -0.002) < 1e-6, "{fit:?}");
    assert!(fit.best.inverted);
    assert!(fit.after.worst_db > -1e-6);
}

#[test]
fn a_linkwitz_riley_4_pair_needs_only_the_geometric_delay() {
    let f = grid();
    let fc = 90.0;
    // Sub (A) low pass at the mic directly, main (B) high pass 5 ms late: the sub is delayed.
    let a: Vec<_> = f.iter().map(|&f| lr4(f, fc).0).collect();
    let b: Vec<_> = f
        .iter()
        .map(|&f| lr4(f, fc).1 * delayed(f, 0.005))
        .collect();
    let fit = fit(&f, &a, &b, None, &band(60.0, 130.0)).expect("the band holds data");
    assert!(rel(fit.best.delay, -0.005) < 0.01, "{fit:?}");
    assert!(!fit.best.inverted);
    assert!(fit.best.residual_deg < 1.0);
}

#[test]
fn a_linkwitz_riley_2_pair_wants_a_polarity_flip() {
    let f = grid();
    let fc = 1200.0;
    let a: Vec<_> = f.iter().map(|&f| lr2(f, fc).0).collect();
    let b: Vec<_> = f.iter().map(|&f| lr2(f, fc).1).collect();
    let fit = fit(&f, &a, &b, None, &band(800.0, 1800.0)).expect("the band holds data");
    assert!(fit.best.inverted, "{fit:?}");
    assert!(fit.best.delay.abs() < 1e-6, "{fit:?}");
}

/// A small deterministic generator: the tests need noise, not a dependency.
fn noise(seed: &mut u64) -> f64 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    ((*seed >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
}

#[test]
fn noisy_phase_at_half_coherence_stays_within_five_percent() {
    let f = grid();
    let mut seed = 7;
    // ±30° of phase noise per column, γ² 0.5.
    let a: Vec<_> = f
        .iter()
        .map(|&f| delayed(f, 0.0042) * Complex64::from_polar(1.0, 0.52 * noise(&mut seed)))
        .collect();
    let b: Vec<_> = f.iter().map(|_| Complex64::new(1.0, 0.0)).collect();
    let coh = vec![0.5; f.len()];
    let fit = fit(&f, &a, &b, Some(&coh), &band(60.0, 120.0)).expect("the band holds data");
    assert!(rel(fit.best.delay, 0.0042) < 0.05, "{fit:?}");
    assert!(!fit.best.inverted);
    assert!(fit.best.residual_deg > 5.0 && fit.best.residual_deg < 25.0);
}

#[test]
fn incoherent_columns_count_less() {
    let f = grid();
    let mut seed = 11;
    // Above 90 Hz the phase is garbage, flagged by γ² 0.01.
    let a: Vec<_> = f
        .iter()
        .map(|&f| {
            if f > 90.0 {
                Complex64::from_polar(1.0, PI * noise(&mut seed))
            } else {
                delayed(f, 0.0031)
            }
        })
        .collect();
    let b: Vec<_> = f.iter().map(|_| Complex64::new(1.0, 0.0)).collect();
    let coh: Vec<f64> = f
        .iter()
        .map(|&f| if f > 90.0 { 0.01 } else { 0.95 })
        .collect();
    let fit = fit(&f, &a, &b, Some(&coh), &band(50.0, 140.0)).expect("the band holds data");
    assert!(rel(fit.best.delay, 0.0031) < 0.01, "{fit:?}");
}

#[test]
fn a_narrow_band_reports_the_next_wrap() {
    let f = grid();
    // A sixth of an octave at 1 kHz: one period (1 ms) away fits almost as well.
    let a: Vec<_> = f.iter().map(|&f| delayed(f, 0.0004)).collect();
    let b: Vec<_> = f.iter().map(|_| Complex64::new(1.0, 0.0)).collect();
    let fit = fit(&f, &a, &b, None, &band(944.0, 1059.0)).expect("the band holds data");
    assert!(rel(fit.best.delay, 0.0004) < 1e-6, "{fit:?}");
    let r = fit.runner_up.expect("runner-up");
    assert!(!r.inverted);
    assert!(
        ((r.delay - fit.best.delay).abs() - 0.001).abs() < 1e-4,
        "{r:?}"
    );
}

#[test]
fn a_band_without_data_has_no_fit() {
    let f = grid();
    let a: Vec<_> = f.iter().map(|_| Complex64::new(1.0, 0.0)).collect();
    assert_eq!(
        fit(&f, &a, &a, None, &band(25_000.0, 30_000.0)),
        Err(NoFit::FewColumns { found: 0 })
    );
    let zero = vec![0.0; f.len()];
    assert_eq!(
        fit(&f, &a, &a, Some(&zero), &band(60.0, 120.0)),
        Err(NoFit::NoWeight)
    );
}

#[test]
fn the_aligned_sum_and_residual_agree() {
    let f = 100.0;
    let a = delayed(f, 0.001);
    let b = Complex64::new(1.0, 0.0);
    assert!(residual(f, a, b, 0.001, false).abs() < 1e-12);
    assert!((aligned_sum(f, a, b, 0.001, false).norm() - 2.0).abs() < 1e-12);
    // Delaying A instead (negative) by the same arrival the other way.
    let a = Complex64::new(1.0, 0.0);
    let b = -delayed(f, 0.001);
    assert!(residual(f, a, b, -0.001, true).abs() < 1e-12);
    assert!((aligned_sum(f, a, b, -0.001, true).norm() - 2.0).abs() < 1e-12);
    assert_eq!(summation_db(a, b, Complex64::new(2.0, 0.0)), Some(0.0));
}
