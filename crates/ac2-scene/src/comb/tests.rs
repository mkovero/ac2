use super::*;
use crate::grid::nearest_column;

const FULL: Range = Range::new(20.0, 20_000.0);

/// 1/48 octave from 20 Hz to 20 kHz.
fn grid() -> Vec<f64> {
    (0..)
        .map(|i| 20.0 * 2f64.powf(f64::from(i) / 48.0))
        .take_while(|f| *f <= 20_000.0)
        .collect()
}

/// A direct sound plus a copy 0.9 as loud 1 ms later: notches at odd multiples of 500 Hz.
fn comb(f: &[f64]) -> Vec<f64> {
    f.iter()
        .map(|f| {
            let w = 2.0 * std::f64::consts::PI * f * 1e-3;
            10.0 * (1.0 + 0.81 + 1.8 * w.cos()).log10()
        })
        .collect()
}

/// Room ripple: 6 dB bumps at 1.15, 1.45 and 1.85 kHz — 300 Hz, then 400 Hz apart.
fn ripple(f: &[f64]) -> Vec<f64> {
    f.iter()
        .map(|f| {
            [1150.0, 1450.0, 1850.0]
                .iter()
                .map(|c| 6.0 * (-0.5 * ((f - c) / 40.0f64).powi(2)).exp())
                .sum()
        })
        .collect()
}

fn end(f: &[f64], hz: f64) -> End {
    let col = nearest_column(f, hz).expect("a column");
    End { col, hz: f[col] }
}

#[test]
fn a_comb_repeats_its_spacing() {
    let f = grid();
    let v = comb(&f);
    let at = |a, b, shown, s| evidence(&f, &v, shown, 3.0, s, end(&f, a), end(&f, b));
    assert_eq!(at(500.0, 1500.0, FULL, None), Some(Comb::Confirmed));
    // 1/6 octave at 1.5 kHz hides combs under about 350 Hz: 1 kHz stays readable.
    let sixth = Some(SmoothingFraction::Sixth);
    assert_eq!(at(1500.0, 500.0, FULL, sixth), Some(Comb::Confirmed));
    // With nothing shown above 3 kHz the third notch is the one below, at 500 Hz.
    let low = Range::new(200.0, 3000.0);
    assert_eq!(at(1500.0, 2500.0, low, None), Some(Comb::Confirmed));
    // Without 500 Hz shown either, 1.5 and 2.5 kHz have nothing to confirm them.
    let mid = Range::new(1000.0, 3000.0);
    assert_eq!(
        at(1500.0, 2500.0, mid, None),
        Some(Comb::Uneven(Landmark::Notch))
    );
}

#[test]
fn uneven_ripple_is_no_comb() {
    let f = grid();
    let v = ripple(&f);
    let at = |s| evidence(&f, &v, FULL, 3.0, s, end(&f, 1150.0), end(&f, 1450.0));
    // The next peak is 400 Hz on, not 300: no third tooth.
    assert_eq!(at(None), Some(Comb::Uneven(Landmark::Peak)));
    // Under 1/6 octave the 300 Hz is finer than the smoothing resolves at 1.45 kHz: that
    // is said first.
    let Some(Comb::Smoothed { fraction, below_hz }) = at(Some(SmoothingFraction::Sixth)) else {
        panic!("{:?}", at(Some(SmoothingFraction::Sixth)));
    };
    assert_eq!(fraction, SmoothingFraction::Sixth);
    // 2 · 1.45 kHz · (2^(1/12) − 2^(−1/12)).
    let hi = f[nearest_column(&f, 1450.0).expect("a column")];
    assert!((below_hz - 2.0 * hi * 0.115_59).abs() < 0.1, "{below_hz}");
    // 1/24 octave resolves it: back to the third tooth.
    assert_eq!(
        at(Some(SmoothingFraction::TwentyFourth)),
        Some(Comb::Uneven(Landmark::Peak))
    );
}

#[test]
fn only_like_landmarks_on_two_columns_space_a_comb() {
    let f = grid();
    let v = comb(&f);
    let at = |a, b| evidence(&f, &v, FULL, 3.0, None, end(&f, a), end(&f, b));
    // A notch to a peak, to a column that is neither, or to itself.
    assert_eq!(at(500.0, 1000.0), None);
    assert_eq!(at(500.0, 700.0), None);
    assert_eq!(at(700.0, 1200.0), None);
    assert_eq!(at(500.0, 500.0), None);
}

#[test]
fn the_limit_is_twice_the_band() {
    // 1/3 octave at 1 kHz is 231.6 Hz wide.
    let l = smoothing_limit_hz(SmoothingFraction::Third, 1000.0);
    assert!((l - 463.1).abs() < 0.1, "{l}");
}
