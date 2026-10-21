use super::*;
use ac2_proto::model::{FreqRange, Resolution};
use ac2_proto::units::Hz;

/// A 10 Hz bin grid to 20 kHz.
fn bins() -> Vec<f64> {
    (0..2000).map(|i| f64::from(i) * 10.0 + 10.0).collect()
}

/// A floor at −90 dB with a ±1 dB ripple: what an averaged noise floor looks like.
fn floor(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| -90.0 + if i % 2 == 0 { 1.0 } else { -1.0 })
        .collect()
}

/// A tone at bin `i` (`level`) with its two neighbours `drop` dB lower.
fn tone(level: &mut [f32], i: usize, peak: f32, drop: f32) {
    level[i] = peak;
    level[i - 1] = peak - drop;
    level[i + 1] = peak - drop;
}

fn narrow<'a>(freqs: &'a [f64], level: &'a [f32]) -> PeakInput<'a> {
    PeakInput {
        freqs,
        level,
        validity: None,
        unresolved: None,
        narrow: true,
    }
}

fn bin_of(freqs: &[f64], hz: f64) -> usize {
    freqs.iter().position(|f| *f == hz).expect("on the grid")
}

#[test]
fn two_tones_loudest_first() {
    let f = bins();
    let mut l = floor(f.len());
    tone(&mut l, bin_of(&f, 3000.0), -35.0, 6.0);
    tone(&mut l, bin_of(&f, 1000.0), -20.0, 6.0);
    let p = find_peaks(&narrow(&f, &l));
    assert_eq!(p.len(), 2, "{p:?}");
    assert_eq!(p[0].freq_hz, 1000.0);
    assert_eq!(p[0].level, -20.0);
    assert_eq!(p[1].freq_hz, 3000.0);
    assert_eq!(p[1].level, -35.0);
}

#[test]
fn narrowband_frequency_is_the_parabola_vertex() {
    // y = −20 − 6 (x − 0.3)² around the 1 kHz bin: the vertex is 0.3 bin (3 Hz) up.
    let f = bins();
    let mut l = floor(f.len());
    let i = bin_of(&f, 1000.0);
    let y = |x: f64| (-20.0 - 6.0 * (x - 0.3).powi(2)) as f32;
    l[i - 1] = y(-1.0);
    l[i] = y(0.0);
    l[i + 1] = y(1.0);
    let p = find_peaks(&narrow(&f, &l));
    assert!((p[0].freq_hz - 1003.0).abs() < 1e-3, "{p:?}");
    // The level is the column's, what the curve shows there.
    assert_eq!(p[0].level, y(0.0) as f64);
}

#[test]
fn rta_band_reads_its_centre() {
    let f = [500.0, 630.0, 800.0, 1000.0, 1250.0, 1600.0, 2000.0];
    let l = [-60.0, -60.0, -45.0, -30.0, -42.0, -60.0, -60.0];
    let p = find_peaks(&PeakInput {
        narrow: false,
        ..narrow(&f, &l)
    });
    assert_eq!(
        p,
        [Peak {
            freq_hz: 1000.0,
            level: -30.0
        }]
    );
}

#[test]
fn a_peak_must_stand_out_by_the_prominence() {
    let f = bins();
    let mut l = vec![-90.0f32; f.len()];
    // 4 dB over the floor: ripple, no mark.
    tone(&mut l, bin_of(&f, 500.0), -86.0, 4.0);
    // 8 dB: marked.
    tone(&mut l, bin_of(&f, 2000.0), -82.0, 8.0);
    // A shoulder maximum on a big peak's flank, 3 dB over the dip between: not its own
    // peak, though far above the floor.
    let k = bin_of(&f, 8000.0);
    l[k] = -20.0;
    for (d, v) in [
        (1, -26.0),
        (2, -32.0),
        (3, -38.0),
        (4, -35.0),
        (5, -50.0),
        (6, -70.0),
    ] {
        l[k + d] = v;
        l[k - d] = -20.0 - 12.0 * d as f32;
    }
    let p = find_peaks(&narrow(&f, &l));
    // The big peak's skirt is lopsided, so its vertex is a little above its bin.
    let at: Vec<f64> = p.iter().map(|p| p.freq_hz.round()).collect();
    assert_eq!(at, [8002.0, 2000.0], "{p:?}");
}

#[test]
fn peaks_closer_than_the_spacing_merge_into_the_stronger() {
    let f = bins();
    // 1000 and 1080 Hz: 0.11 octave apart, one peak.
    let mut l = floor(f.len());
    tone(&mut l, bin_of(&f, 1000.0), -30.0, 10.0);
    tone(&mut l, bin_of(&f, 1080.0), -25.0, 10.0);
    let p = find_peaks(&narrow(&f, &l));
    assert_eq!(p.len(), 1, "{p:?}");
    assert_eq!(p[0].freq_hz, 1080.0);
    // 1000 and 1200 Hz: 0.26 octave apart, two.
    let mut l = floor(f.len());
    tone(&mut l, bin_of(&f, 1000.0), -30.0, 10.0);
    tone(&mut l, bin_of(&f, 1200.0), -25.0, 10.0);
    assert_eq!(find_peaks(&narrow(&f, &l)).len(), 2);
}

#[test]
fn gaps_invalid_and_unresolved_columns_hold_no_peak() {
    let f = bins();
    let mut l = floor(f.len());
    let (a, b, c) = (bin_of(&f, 1000.0), bin_of(&f, 4000.0), bin_of(&f, 100.0));
    tone(&mut l, a, -20.0, 10.0);
    tone(&mut l, b, -20.0, 10.0);
    tone(&mut l, c, -20.0, 10.0);
    l[a] = f32::NAN;
    let mut validity = vec![ValidityMask::NONE; f.len()];
    validity[b] = ValidityMask::INSUFFICIENT_RESOLUTION;
    let unresolved = Unresolved {
        resolution: Resolution::FortyEighth,
        ranges: vec![FreqRange {
            lo: Hz(0.0),
            hi: Hz(200.0),
        }],
    };
    let p = find_peaks(&PeakInput {
        validity: Some(&validity),
        unresolved: Some(&unresolved),
        ..narrow(&f, &l)
    });
    // The neighbours of the gap and of the invalid column are the tone's skirt, now maxima
    // beside nothing: they stand only 10 dB over the floor on one side, which is enough —
    // but never at the flagged column itself.
    assert!(
        p.iter().all(|p| p.freq_hz != 1000.0 && p.freq_hz != 4000.0),
        "{p:?}"
    );
    assert!(p.iter().all(|p| p.freq_hz > 200.0), "{p:?}");
}

/// Frames of six peaks where the fifth and sixth trade places by a dB each frame.
fn frame(flip: bool) -> Vec<Peak> {
    let mut v = vec![
        Peak {
            freq_hz: 100.0,
            level: -10.0,
        },
        Peak {
            freq_hz: 400.0,
            level: -15.0,
        },
        Peak {
            freq_hz: 1000.0,
            level: -20.0,
        },
        Peak {
            freq_hz: 2500.0,
            level: -25.0,
        },
        Peak {
            freq_hz: 6000.0,
            level: if flip { -31.0 } else { -29.5 },
        },
        Peak {
            freq_hz: 12000.0,
            level: if flip { -29.5 } else { -31.0 },
        },
    ];
    v.sort_by(|a, b| b.level.total_cmp(&a.level));
    v
}

#[test]
fn marks_stay_put_while_two_peaks_trade_places() {
    let mut t = PeakTracker::default();
    t.update(&frame(false));
    let first: Vec<f64> = t.peaks().iter().map(|p| p.freq_hz).collect();
    assert_eq!(first, [100.0, 400.0, 1000.0, 2500.0, 6000.0]);
    for k in 0..10 {
        t.update(&frame(k % 2 == 0));
        let now: Vec<f64> = t.peaks().iter().map(|p| p.freq_hz).collect();
        assert_eq!(now, first, "frame {k}");
    }
    // A peak that beats a mark by more than the head start takes its place.
    let mut f = frame(false);
    f[5].level = -26.0;
    t.update(&f);
    let now: Vec<f64> = t.peaks().iter().map(|p| p.freq_hz).collect();
    assert_eq!(now, [100.0, 400.0, 1000.0, 2500.0, 12000.0]);
}

#[test]
fn a_mark_that_drifts_within_the_spacing_keeps_its_row() {
    let mut t = PeakTracker::default();
    let a = |hz, l| Peak {
        freq_hz: hz,
        level: l,
    };
    t.update(&[a(1000.0, -20.0), a(3000.0, -20.5)]);
    // Within a dB of each other: the rows stay, though the order of levels flips.
    t.update(&[a(3010.0, -19.8), a(1005.0, -20.4)]);
    let now: Vec<f64> = t.peaks().iter().map(|p| p.freq_hz).collect();
    assert_eq!(now, [1005.0, 3010.0]);
    // Ahead by more than ORDER_DB: it moves up.
    t.update(&[a(3000.0, -18.0), a(1000.0, -20.0)]);
    let now: Vec<f64> = t.peaks().iter().map(|p| p.freq_hz).collect();
    assert_eq!(now, [3000.0, 1000.0]);
    // Nothing found: nothing marked.
    t.update(&[]);
    assert!(t.peaks().is_empty());
}

/// The magnitude of a direct sound plus a copy `gain` as loud, `tau` seconds later, on a
/// 1/48-octave grid from 20 Hz to 20 kHz: `|1 + g·e^(−j2πfτ)|`, dB.
fn comb(tau: f64, gain: f64) -> (Vec<f64>, Vec<f64>) {
    let f: Vec<f64> = (0..)
        .map(|i| 20.0 * 2f64.powf(f64::from(i) / 48.0))
        .take_while(|f| *f <= 20_000.0)
        .collect();
    let m = f
        .iter()
        .map(|f| {
            let w = 2.0 * std::f64::consts::PI * f * tau;
            10.0 * (1.0 + gain * gain + 2.0 * gain * w.cos()).log10()
        })
        .collect();
    (f, m)
}

#[test]
fn a_combs_dips_sit_on_its_notches() {
    // 1 ms: notches at odd multiples of 1/(2τ) = 500 Hz.
    let (f, m) = comb(1e-3, 0.9);
    let dips = find_extremes(&f, &m, Extreme::Min, true);
    let mut t = PeakTracker::new(Extreme::Min);
    t.update(&dips);
    assert_eq!(t.peaks().len(), TRACKED);
    for p in t.peaks() {
        let k = (p.freq_hz / 1000.0 - 0.5).round();
        let notch = (k + 0.5) * 1000.0;
        assert!(
            (p.freq_hz - notch).abs() < 0.01 * notch,
            "{p:?} is not on a notch"
        );
        // 1 − 0.9 = 0.1: −20 dB at the notch, read near it on the grid.
        assert!(p.level < -12.0, "{p:?}");
    }
    // Deepest first.
    assert!(t.peaks().windows(2).all(|w| w[0].level <= w[1].level));
    let spacing = comb_spacing(&dips).expect("a comb");
    assert!((spacing - 1000.0).abs() < 10.0, "{spacing}");
    // The peaks between, at whole multiples of 1 kHz.
    let peaks = find_extremes(&f, &m, Extreme::Max, true);
    assert!(!peaks.is_empty());
    for p in &peaks {
        let k = (p.freq_hz / 1000.0).round();
        assert!((p.freq_hz - k * 1000.0).abs() < 0.01 * p.freq_hz, "{p:?}");
    }
}

#[test]
fn uneven_dips_are_no_comb() {
    let d = |hz| Peak {
        freq_hz: hz,
        level: -20.0,
    };
    assert_eq!(
        comb_spacing(&[d(500.0), d(1500.0), d(2500.0)]),
        None,
        "too few"
    );
    assert_eq!(
        comb_spacing(&[d(500.0), d(1500.0), d(2600.0), d(4500.0)]),
        None
    );
    // A room-mode dip beside a comb does not hide it.
    let s = comb_spacing(&[d(80.0), d(500.0), d(1500.0), d(2510.0), d(3490.0)]);
    assert!(s.is_some_and(|s| (s - 996.67).abs() < 0.1), "{s:?}");
}
