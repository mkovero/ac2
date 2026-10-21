use super::iso226::{FREQS, REF_INDEX, contour, loudness_shift, spl};
use super::*;

#[test]
fn a_tilt_is_its_slope_per_octave_through_zero_at_the_pivot() {
    assert_eq!(tilt_db(1000.0, -1.0, 1000.0), 0.0);
    assert!((tilt_db(4000.0, -1.0, 1000.0) + 2.0).abs() < 1e-12);
    assert!((tilt_db(125.0, -1.0, 1000.0) - 3.0).abs() < 1e-12);
    assert!((tilt_db(400.0, 0.5, 200.0) - 0.5).abs() < 1e-12);
}

#[test]
fn the_shelf_has_its_gain_below_half_at_the_corner_and_none_above() {
    let f = [5.0, 100.0, 20_000.0];
    let s = lf_shelf_db(&f, 6.0, 100.0).expect("shelf");
    assert!((s[0] - 6.0).abs() < 0.01, "{s:?}");
    assert!((s[1] - 3.0).abs() < 1e-9, "{s:?}");
    assert!(s[2].abs() < 0.01, "{s:?}");
    let cut = lf_shelf_db(&f, -4.0, 100.0).expect("shelf");
    assert!((cut[1] + 2.0).abs() < 1e-9, "{cut:?}");
}

#[test]
fn the_shelf_never_overshoots() {
    let f: Vec<f64> = (0..400)
        .map(|k| 10.0 * 2f64.powf(f64::from(k) / 40.0))
        .collect();
    let s = lf_shelf_db(&f, 8.0, 120.0).expect("shelf");
    assert!(s.windows(2).all(|w| w[1] <= w[0] + 1e-12));
    assert!(s.iter().all(|v| (-1e-9..=8.0 + 1e-9).contains(v)));
}

#[test]
fn a_shelf_of_no_gain_is_flat_and_a_bad_one_is_refused() {
    assert_eq!(
        lf_shelf_db(&[50.0, 500.0], 0.0, 100.0),
        Some(vec![0.0, 0.0])
    );
    assert_eq!(lf_shelf_db(&[50.0], 40.0, 100.0), None);
    assert_eq!(lf_shelf_db(&[50.0], 6.0, 0.0), None);
}

#[test]
fn a_contour_crosses_1_khz_at_its_own_loudness_level() {
    // The phon's definition; the 2003 formula's rounded constants leave about 0.01 dB.
    assert_eq!(FREQS[REF_INDEX], 1000.0);
    for phon in [20.0, 40.0, 60.0, 80.0, 90.0, 100.0] {
        let at = spl(REF_INDEX, phon);
        assert!((at - phon).abs() < 0.05, "{phon} phon: {at} dB at 1 kHz");
    }
}

#[test]
fn louder_contours_are_flatter_at_the_low_end() {
    for (lo, hi) in [(20.0, 40.0), (40.0, 80.0), (80.0, 100.0)] {
        let (a, b) = (contour(lo), contour(hi));
        for i in 0..REF_INDEX {
            assert!(
                b[i] - a[i] > 0.0 && b[i] - a[i] < hi - lo,
                "{} Hz: {} → {}",
                FREQS[i],
                a[i],
                b[i]
            );
        }
    }
}

#[test]
fn the_shift_is_zero_at_1_khz_and_for_no_level_change_and_reverses_with_the_levels() {
    let s = loudness_shift(85.0, 100.0);
    let back = loudness_shift(100.0, 85.0);
    assert_eq!(s[REF_INDEX], 0.0);
    for (a, b) in s.iter().zip(&back) {
        assert!((a + b).abs() < 1e-12);
    }
    assert!(loudness_shift(70.0, 70.0).iter().all(|v| *v == 0.0));
}

#[test]
fn played_louder_the_lows_seem_louder_and_the_presence_region_slightly_quieter() {
    let s = loudness_shift(85.0, 100.0);
    let at = |f: f64| s[FREQS.iter().position(|x| *x == f).expect("iso freq")];
    assert!((at(20.0) - 7.9).abs() < 0.1, "{}", at(20.0));
    assert!((at(100.0) - 4.7).abs() < 0.1, "{}", at(100.0));
    assert!(at(4000.0) < 0.0 && at(4000.0) > -1.0, "{}", at(4000.0));
    // Monotonic from 20 Hz up to 1 kHz: the lower, the more.
    assert!(s[..=REF_INDEX].windows(2).all(|w| w[0] > w[1]));
}

#[test]
fn matches_refgen_target_curves() {
    use ac2_testkit::GoldenSet;
    let gs = GoldenSet::load("target_curves").expect("golden set");
    assert_eq!(gs.f64("iso_freq_hz").expect("freqs"), FREQS.to_vec());
    for phon in [20, 40, 60, 80, 85, 90, 100] {
        gs.assert_f64(&format!("contour_{phon}"), &contour(f64::from(phon)));
    }
    for (a, b) in [(85, 100), (100, 85), (60, 90)] {
        gs.assert_f64(
            &format!("shift_{a}_{b}"),
            &loudness_shift(f64::from(a), f64::from(b)),
        );
    }
    let f = gs.f64("tilt_freq_hz").expect("tilt freqs");
    for (name, slope, pivot, gain, corner) in [
        ("tilt_m1_1k", -1.0, 1000.0, 0.0, 1.0),
        ("tilt_m1_1k_shelf_p6_100", -1.0, 1000.0, 6.0, 100.0),
        ("tilt_p05_200_shelf_m4_60", 0.5, 200.0, -4.0, 60.0),
    ] {
        let shelf = lf_shelf_db(&f, gain, corner).expect("shelf");
        let y: Vec<f64> = f
            .iter()
            .zip(&shelf)
            .map(|(f, s)| tilt_db(*f, slope, pivot) + s)
            .collect();
        gs.assert_f64(name, &y);
    }
}
