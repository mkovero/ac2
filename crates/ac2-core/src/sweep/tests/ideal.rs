//! Harmonics separated against the emitted sweep: the reference path's own distortion stays
//! out of the measurement's, distortion both paths share is reported in full, the reference's
//! own harmonics are reported, and the alignment holds at a fractional delay.

use super::spectral::fft_forward;
use super::*;

/// `x` delayed by `d` samples (any real `d ≥ 0`), band-limited: a linear phase on the
/// zero-padded spectrum.
fn shifted(x: &[f64], d: f64) -> Vec<f64> {
    let n = 2 * x.len().next_power_of_two();
    let s: Vec<Complex64> = fft_forward(x, n)
        .iter()
        .enumerate()
        .map(|(k, z)| z * Complex64::from_polar(1.0, -TAU * k as f64 * d / n as f64))
        .collect();
    let mut y = fft_inverse(s, n);
    y.truncate(x.len());
    y
}

/// Coefficients of `x + a2·x² + a3·x³` that give H2 and H3 at `h2_db`, `h3_db` re the
/// fundamental at the test amplitude (the cubic's compression of the fundamental is
/// negligible at these levels).
fn coefficients(h2_db: f64, h3_db: f64) -> (f64, f64) {
    let a = amp();
    (
        2.0 * 10f64.powf(h2_db / 20.0) / a,
        4.0 * 10f64.powf(h3_db / 20.0) / (a * a),
    )
}

/// Analytic H2 and H3 of the polynomial at the test amplitude, dB re the fundamental.
fn exact(a2: f64, a3: f64) -> (f64, f64) {
    let (_, h2, h3) = analytic(amp(), a2, a3);
    (20.0 * h2.log10(), 20.0 * h3.log10())
}

/// Fundamentals checked: H3 of the top one still below the sweep's fade-out.
const BAND: (f64, f64) = (100.0, 6000.0 / 3.0 / 1.15);

fn sweep() -> SweepSpec {
    spec(ess(50.0, 6000.0, 3.0))
}

/// Columns of `c` whose level counts as distortion.
fn valid_count(c: &HarmonicCurve) -> usize {
    c.level_db
        .iter()
        .zip(&c.floor_db)
        .filter(|(l, f)| is_valid(**l, **f))
        .count()
}

#[test]
fn the_reference_paths_distortion_stays_out_of_the_measurement() {
    // The loopback distorts (H2 at −60 dB), the measured path less (H2 −70 dB, H3 −65 dB).
    // Divided by the recorded reference, the measurement's H2 would read the difference of
    // the two paths' H2, about −60.9 dB for these signs; against the emitted sweep it is the
    // measured path's own.
    let s = sweep();
    let (r2, _) = coefficients(-60.0, -200.0);
    let (a2, a3) = coefficients(-70.0, -65.0);
    let x = emitted(&s, 1, 0.37);
    let reference = delayed(&poly(&x, r2, 0.0), REF_DELAY, REF_GAIN);
    let mut mic = delayed(&poly(&x, a2, a3), MIC_DELAY, MIC_GAIN);
    Noise(31).add(&mut mic, 1e-7);
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    let (h2, h3) = exact(a2, a3);
    let e2 = max_err(&r, &r.harmonics[0], BAND.0, BAND.1, |_| h2);
    let e3 = max_err(&r, &r.harmonics[1], BAND.0, BAND.1, |_| h3);
    eprintln!("ref H2 −60, meas H2 {h2:.1} H3 {h3:.1}: max error H2 {e2:.3} dB, H3 {e3:.3}");
    assert!(e2 <= 0.1 && e3 <= 0.1, "H2 {e2:.3} dB, H3 {e3:.3} dB");

    // The reference's own: its H2, and no H3.
    let (rh2, _) = exact(r2, 0.0);
    let er = max_err(&r, &r.reference_harmonics[0], BAND.0, BAND.1, |_| rh2);
    eprintln!("reference H2 {rh2:.1} dBr: max error {er:.3} dB");
    assert!(er <= 0.1, "reference H2 {er:.3} dB");
    let h3_valid = valid_count(&r.reference_harmonics[1]);
    assert!(h3_valid <= 5, "reference H3 valid in {h3_valid} columns");
}

#[test]
fn distortion_both_paths_share_is_reported_in_full() {
    // The distortion arises before the split (the interface's DAC): the reference carries it
    // as much as the measurement. Divided by the reference it would cancel; against the
    // emitted sweep both inputs read it.
    let s = sweep();
    let (a2, a3) = coefficients(-60.0, -70.0);
    let x = poly(&emitted(&s, 1, 0.37), a2, a3);
    let reference = delayed(&x, REF_DELAY, REF_GAIN);
    let mut mic = delayed(&x, MIC_DELAY, MIC_GAIN);
    Noise(37).add(&mut mic, 1e-7);
    let r = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    let (h2, h3) = exact(a2, a3);
    for (name, c) in [
        ("measurement", &r.harmonics),
        ("reference", &r.reference_harmonics),
    ] {
        let e2 = max_err(&r, &c[0], BAND.0, BAND.1, |_| h2);
        let e3 = max_err(&r, &c[1], BAND.0, BAND.1, |_| h3);
        eprintln!("shared H2 {h2:.1} H3 {h3:.1}, {name}: max error H2 {e2:.3} dB, H3 {e3:.3}");
        assert!(
            e2 <= 0.1 && e3 <= 0.1,
            "{name}: H2 {e2:.3} dB, H3 {e3:.3} dB"
        );
    }
}

#[test]
fn the_emitted_sweep_aligns_to_a_fractional_delay() {
    // Both paths a whole number of samples and 0.37 more behind the emitted sweep: the
    // alignment finds the reference's onset to a fraction of a sample, and the harmonics of
    // either input read as without it.
    let s = sweep();
    let (r2, _) = coefficients(-60.0, -200.0);
    let (a2, a3) = coefficients(-70.0, -65.0);
    let lead = 0.37;
    let x = emitted(&s, 2, lead);
    let frac = 0.37;
    let reference: Vec<f64> = shifted(&poly(&x, r2, 0.0), REF_DELAY as f64 + frac)
        .iter()
        .map(|v| REF_GAIN * v)
        .collect();
    let mut mic: Vec<f64> = shifted(&poly(&x, a2, a3), MIC_DELAY as f64 + frac)
        .iter()
        .map(|v| MIC_GAIN * v)
        .collect();
    Noise(41).add(&mut mic, 1e-7);
    let r = analyse_recording(&s, &reference, &mic, 2).expect("analysis");
    let want = (lead * FS + REF_DELAY as f64 + frac) / FS;
    let err = (r.first_onset_s - want) * FS;
    eprintln!("onset error {err:.4} samples");
    assert!(err.abs() <= 0.01, "onset off by {err:.4} samples");
    let arrival = r.arrival_s * FS - (MIC_DELAY - REF_DELAY) as f64;
    assert!(arrival.abs() <= 0.01, "arrival off by {arrival:.4} samples");
    let (h2, h3) = exact(a2, a3);
    let (rh2, _) = exact(r2, 0.0);
    let e2 = max_err(&r, &r.harmonics[0], BAND.0, BAND.1, |_| h2);
    let e3 = max_err(&r, &r.harmonics[1], BAND.0, BAND.1, |_| h3);
    let er = max_err(&r, &r.reference_harmonics[0], BAND.0, BAND.1, |_| rh2);
    eprintln!("fractional delay: max error H2 {e2:.3} dB, H3 {e3:.3}, reference H2 {er:.3}");
    assert!(
        e2 <= 0.1 && e3 <= 0.1 && er <= 0.1,
        "H2 {e2:.3}, H3 {e3:.3}, ref H2 {er:.3}"
    );
}
