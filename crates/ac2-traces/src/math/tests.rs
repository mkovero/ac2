//! Analytic checks: every expected value is a closed-form function of the operands.
#![allow(clippy::unwrap_used)]

use std::f64::consts::PI;

use ac2_core::average::DelayReference;
use num_complex::Complex64;

use super::*;

fn grid() -> GridDef {
    GridDef::Log {
        ppo: 24,
        k_min: -120,
        k_max: 119,
    }
}

const EPOCH: Option<SessionEpoch> = Some(SessionEpoch(3));

/// Columns of `h(f)` (with its own delay already removed).
fn tf(freqs: &[f64], h: impl Fn(f64) -> Complex64, coh: f32) -> Columns {
    let z: Vec<Complex64> = freqs.iter().map(|f| h(*f)).collect();
    Columns {
        mag_db: z.iter().map(|z| (20.0 * z.norm().log10()) as f32).collect(),
        phase_deg: Some(z.iter().map(|z| z.arg().to_degrees() as f32).collect()),
        coherence: Some(vec![coh; freqs.len()]),
    }
}

/// First-order low-pass at `fc`.
fn lowpass(fc: f64) -> impl Fn(f64) -> Complex64 {
    move |f| Complex64::new(1.0, 0.0) / Complex64::new(1.0, f / fc)
}

/// First-order high-pass at `fc`.
fn highpass(fc: f64) -> impl Fn(f64) -> Complex64 {
    move |f| Complex64::new(0.0, f / fc) / Complex64::new(1.0, f / fc)
}

fn input(c: &Columns, delay: f64, time_base: Option<SessionEpoch>) -> Input<'_> {
    Input {
        columns: c,
        delay,
        time_base,
    }
}

fn phase_err(got: f32, want: f64) -> f64 {
    let d = (f64::from(got) - want).rem_euclid(360.0);
    d.min(360.0 - d)
}

fn check(r: &MathResult, freqs: &[f64], want: impl Fn(f64) -> Complex64) {
    for (i, f) in freqs.iter().enumerate() {
        let w = want(*f);
        let m = r.columns.mag_db[i];
        assert!(
            (f64::from(m) - 20.0 * w.norm().log10()).abs() < 1e-3,
            "{f} Hz: {m} dB vs {}",
            20.0 * w.norm().log10()
        );
        let p = r.columns.phase_deg.as_ref().unwrap()[i];
        assert!(
            phase_err(p, w.arg().to_degrees()) < 1e-2,
            "{f} Hz: {p}° vs {}°",
            w.arg().to_degrees()
        );
    }
}

/// `A ÷ B` of two known filters on one time base is their ratio per column, magnitude and
/// phase, with A's later arrival (`τa − τb`) in the phase: the result states delay 0.
#[test]
fn ratio_of_two_filters() {
    let g = grid();
    let f = frequencies(&g);
    let (ta, tb) = (0.0123, 0.0103);
    let a = tf(&f, lowpass(1000.0), 0.9);
    let b = tf(&f, highpass(200.0), 0.6);
    let r = transfer(
        Combine::Binary(MathOp::Divide),
        &[input(&a, ta, EPOCH), input(&b, tb, EPOCH)],
        &g,
        &f,
        DelayReference::Trace(0),
    )
    .unwrap();
    assert_eq!(r.phase, PhaseBasis::SharedTimeBase);
    assert_eq!(r.delay, Seconds(0.0));
    check(&r, &f, |x| {
        lowpass(1000.0)(x) / highpass(200.0)(x)
            * Complex64::from_polar(1.0, -2.0 * PI * x * (ta - tb))
    });
    // A ratio is as trustworthy as its less coherent operand.
    assert!(r.columns.coherence.unwrap().iter().all(|c| *c == 0.6));
}

/// Without a shared time base the ratio is of each operand as aligned by its own delay, and
/// says so.
#[test]
fn ratio_across_time_bases_is_marked() {
    let g = grid();
    let f = frequencies(&g);
    let a = tf(&f, lowpass(1000.0), 0.9);
    let b = tf(&f, highpass(200.0), 0.9);
    let r = transfer(
        Combine::Binary(MathOp::Divide),
        &[input(&a, 0.0123, EPOCH), input(&b, 0.0, None)],
        &g,
        &f,
        DelayReference::Trace(0),
    )
    .unwrap();
    assert_eq!(r.phase, PhaseBasis::OwnAlignments);
    check(&r, &f, |x| lowpass(1000.0)(x) / highpass(200.0)(x));
}

/// `A × B` is the cascade: the product of the filters, its delay the sum of theirs.
#[test]
fn product_is_the_cascade() {
    let g = grid();
    let f = frequencies(&g);
    let a = tf(&f, lowpass(1000.0), 0.9);
    let b = tf(&f, highpass(200.0), 0.9);
    let r = transfer(
        Combine::Binary(MathOp::Multiply),
        &[input(&a, 0.002, EPOCH), input(&b, 0.003, EPOCH)],
        &g,
        &f,
        DelayReference::Trace(0),
    )
    .unwrap();
    assert!((r.delay.0 - 0.005).abs() < 1e-12);
    check(&r, &f, |x| lowpass(1000.0)(x) * highpass(200.0)(x));
}

/// Two copies of one flat response, B arriving `τ` later, sum to the comb
/// `|1 + e^{−j2πfτ}| = 2|cos(πfτ)|` (with phase `−πfτ` off the notches) referred to A's
/// delay; their difference is `2|sin(πfτ)|`.
#[test]
fn sum_of_two_delayed_copies_is_a_comb() {
    let g = grid();
    let f = frequencies(&g);
    let tau = 0.000_37;
    let flat = tf(&f, |_| Complex64::new(1.0, 0.0), 0.95);
    let ins = [input(&flat, 0.010, EPOCH), input(&flat, 0.010 + tau, EPOCH)];
    let r = transfer(
        Combine::Binary(MathOp::Add),
        &ins,
        &g,
        &f,
        DelayReference::Trace(0),
    )
    .unwrap();
    assert_eq!(r.delay, Seconds(0.010));
    assert!(r.columns.coherence.is_none(), "a sum has no coherence");
    for (i, x) in f.iter().enumerate() {
        let want = 2.0 * (PI * x * tau).cos().abs();
        if want < 1e-3 {
            continue;
        }
        assert!(
            (f64::from(r.columns.mag_db[i]) - 20.0 * want.log10()).abs() < 1e-3,
            "{x} Hz"
        );
    }
    check(&r, &f, |x| {
        Complex64::new(1.0, 0.0) + Complex64::from_polar(1.0, -2.0 * PI * x * tau)
    });
    let d = transfer(
        Combine::Binary(MathOp::Subtract),
        &ins,
        &g,
        &f,
        DelayReference::Trace(0),
    )
    .unwrap();
    check(&d, &f, |x| {
        Complex64::new(1.0, 0.0) - Complex64::from_polar(1.0, -2.0 * PI * x * tau)
    });
    // Referred to a fixed delay instead, the same sum's phase moves by that delay.
    let r2 = transfer(
        Combine::Binary(MathOp::Add),
        &ins,
        &g,
        &f,
        DelayReference::Fixed(0.011),
    )
    .unwrap();
    assert_eq!(r2.delay, Seconds(0.011));
    check(&r2, &f, |x| {
        (Complex64::new(1.0, 0.0) + Complex64::from_polar(1.0, -2.0 * PI * x * tau))
            * Complex64::from_polar(1.0, 2.0 * PI * x * 0.001)
    });
}

/// A sum needs the operands' relative arrival: across time bases, or without phase, it is
/// refused, never computed from each operand's own alignment.
#[test]
fn sum_without_a_shared_time_base_is_refused() {
    let g = grid();
    let f = frequencies(&g);
    let a = tf(&f, lowpass(1000.0), 0.9);
    let mut target = tf(&f, lowpass(500.0), 0.9);
    target.phase_deg = None;
    let other_epoch = Some(SessionEpoch(2));
    for op in [MathOp::Add, MathOp::Subtract] {
        assert_eq!(
            transfer(
                Combine::Binary(op),
                &[input(&a, 0.0, EPOCH), input(&a, 0.0, other_epoch)],
                &g,
                &f,
                DelayReference::Trace(0),
            ),
            Err(MathError::NoSharedTimeBase)
        );
        assert_eq!(
            transfer(
                Combine::Binary(op),
                &[input(&a, 0.0, EPOCH), input(&target, 0.0, None)],
                &g,
                &f,
                DelayReference::Trace(0),
            ),
            Err(MathError::NoPhase(1))
        );
    }
    assert!(
        MathError::NoSharedTimeBase
            .to_string()
            .contains("relative arrival is unknown")
    );
    // A ratio with an operand without phase is a magnitude comparison.
    let r = transfer(
        Combine::Binary(MathOp::Divide),
        &[input(&a, 0.0, EPOCH), input(&target, 0.0, None)],
        &g,
        &f,
        DelayReference::Trace(0),
    )
    .unwrap();
    assert_eq!(r.phase, PhaseBasis::NoPhase);
    assert!(r.columns.phase_deg.is_none());
    for (i, x) in f.iter().enumerate() {
        let want = 20.0 * (lowpass(1000.0)(*x) / lowpass(500.0)(*x)).norm().log10();
        assert!((f64::from(r.columns.mag_db[i]) - want).abs() < 1e-3);
    }
}

/// ±3 dB operands average by power to `10·lg((10^0.3 + 10^−0.3)/2)`; two copies of one
/// path 0.5 ms apart average complex to `|cos(πfτ)|`.
#[test]
fn averages_match_the_spatial_average() {
    let g = grid();
    let f = frequencies(&g);
    let up = tf(&f, |_| Complex64::new(10f64.powf(3.0 / 20.0), 0.0), 0.9);
    let down = tf(&f, |_| Complex64::new(10f64.powf(-3.0 / 20.0), 0.0), 0.9);
    let r = transfer(
        Combine::Average(AverageMethod::Power),
        &[input(&up, 0.01, EPOCH), input(&down, 0.01, EPOCH)],
        &g,
        &f,
        DelayReference::Trace(0),
    )
    .unwrap();
    let want = 10.0 * ((10f64.powf(0.3) + 10f64.powf(-0.3)) / 2.0).log10();
    assert!(
        r.columns
            .mag_db
            .iter()
            .all(|m| (f64::from(*m) - want).abs() < 1e-4)
    );
    let flat = tf(&f, |_| Complex64::new(1.0, 0.0), 0.9);
    let tau = 0.0005;
    let c = transfer(
        Combine::Average(AverageMethod::Complex),
        &[input(&flat, 0.01, EPOCH), input(&flat, 0.01 + tau, EPOCH)],
        &g,
        &f,
        DelayReference::Trace(0),
    )
    .unwrap();
    for (i, x) in f.iter().enumerate() {
        let want = (PI * x * tau).cos().abs();
        if want > 1e-3 {
            assert!((f64::from(c.columns.mag_db[i]) - 20.0 * want.log10()).abs() < 1e-3);
        }
    }
    // Across time bases only the power average exists, without phase.
    let p = transfer(
        Combine::Average(AverageMethod::Power),
        &[input(&up, 0.01, EPOCH), input(&down, 0.0, None)],
        &g,
        &f,
        DelayReference::Trace(0),
    )
    .unwrap();
    assert_eq!(p.phase, PhaseBasis::NoPhase);
    assert!((f64::from(p.columns.mag_db[10]) - want).abs() < 1e-4);
    assert_eq!(
        transfer(
            Combine::Average(AverageMethod::Complex),
            &[input(&up, 0.01, EPOCH), input(&down, 0.0, None)],
            &g,
            &f,
            DelayReference::Trace(0),
        ),
        Err(MathError::NoSharedTimeBase)
    );
}

/// A gap in either operand stays a gap in the result.
#[test]
fn gaps_stay_gaps() {
    let g = grid();
    let f = frequencies(&g);
    let a = tf(&f, lowpass(1000.0), 0.9);
    let mut b = tf(&f, lowpass(2000.0), 0.9);
    b.mag_db[5] = f32::NAN;
    if let Some(p) = b.phase_deg.as_mut() {
        p[5] = f32::NAN;
    }
    for op in [
        MathOp::Divide,
        MathOp::Multiply,
        MathOp::Add,
        MathOp::Subtract,
    ] {
        let r = transfer(
            Combine::Binary(op),
            &[input(&a, 0.0, EPOCH), input(&b, 0.0, EPOCH)],
            &g,
            &f,
            DelayReference::Trace(0),
        )
        .unwrap();
        assert!(r.columns.mag_db[5].is_nan(), "{op:?}");
        assert!(r.columns.mag_db[6].is_finite(), "{op:?}");
    }
}

/// Levels: `A − B` is the level difference, `A + B` the power sum, the average the power
/// mean; ratios and products of levels are refused.
#[test]
fn level_math() {
    let lv = |v: &[f32]| Columns {
        mag_db: v.to_vec(),
        phase_deg: None,
        coherence: None,
    };
    let a = lv(&[-20.0, -40.0, f32::NAN]);
    let b = lv(&[-26.0, -40.0, -30.0]);
    let ins = [input(&a, 0.0, None), input(&b, 0.0, None)];
    let d = levels(Combine::Binary(MathOp::Subtract), &ins).unwrap();
    assert_eq!(d.columns.mag_db[0], 6.0);
    assert_eq!(d.columns.mag_db[1], 0.0);
    assert!(d.columns.mag_db[2].is_nan());
    assert_eq!(d.phase, PhaseBasis::NoPhase);
    let s = levels(Combine::Binary(MathOp::Add), &ins).unwrap();
    let want = 10.0 * (10f64.powf(-2.0) + 10f64.powf(-2.6)).log10();
    assert!((f64::from(s.columns.mag_db[0]) - want).abs() < 1e-5);
    // Two equal levels sum to 3.01 dB more.
    assert!((f64::from(s.columns.mag_db[1]) + 40.0 - 10.0 * 2f64.log10()).abs() < 1e-5);
    let m = levels(Combine::Average(AverageMethod::Power), &ins).unwrap();
    let want = 10.0 * ((10f64.powf(-2.0) + 10f64.powf(-2.6)) / 2.0).log10();
    assert!((f64::from(m.columns.mag_db[0]) - want).abs() < 1e-5);
    for op in [MathOp::Divide, MathOp::Multiply] {
        assert_eq!(
            levels(Combine::Binary(op), &ins),
            Err(MathError::NotForLevels(op))
        );
    }
    assert_eq!(
        levels(Combine::Average(AverageMethod::Complex), &ins),
        Err(MathError::PowerOnly)
    );
}
