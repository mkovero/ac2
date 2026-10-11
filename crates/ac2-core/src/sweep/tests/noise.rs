//! The noise floor and the averaged noise windows.

use super::*;

#[test]
fn noise_reads_as_floor_and_repeats_lower_it() {
    let s = spec(ess(50.0, 6000.0, 1.0));
    let floor = |repeats: usize| {
        let (reference, mic) = record(&s, repeats, |x| x.to_vec(), 3e-3, 7);
        let r = analyse_recording(&s, &reference, &mic, repeats).expect("analysis");
        assert_eq!(r.repeats, repeats);
        let (mut valid, mut measured, mut gap) = (0usize, 0usize, Vec::new());
        for c in &r.harmonics {
            for (l, f) in c.level_db.iter().zip(&c.floor_db) {
                if l.is_finite() {
                    measured += 1;
                    valid += usize::from(is_valid(*l, *f));
                    gap.push(l - f);
                }
            }
        }
        // A linear system has no harmonics: the windows hold noise like the floor's.
        assert!(valid * 20 <= measured, "{valid} of {measured} valid");
        gap.sort_by(f64::total_cmp);
        let median = gap[gap.len() / 2];
        assert!(median.abs() < 1.5, "median level − floor {median}");
        let f2 = &r.harmonics[0].floor_db;
        let mut v: Vec<f64> = f2.iter().copied().filter(|x| x.is_finite()).collect();
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    let (one, four) = (floor(1), floor(4));
    eprintln!("noise floor: 1 repeat {one:.1} dB, 4 repeats {four:.1} dB");
    // Four repeats: 6 dB less noise.
    assert!((one - four - 6.0).abs() < 1.5, "{one} vs {four}");
}

/// Deterministic 1/f noise (Kellet's three-pole pinking filter on Gaussian noise).
fn pink(len: usize, seed: u64) -> Vec<f64> {
    let mut g = Noise(seed);
    let mut b = [0.0f64; 3];
    (0..len)
        .map(|_| {
            let x = g.gauss();
            b[0] = 0.99765 * b[0] + 0.099_046 * x;
            b[1] = 0.963 * b[1] + 0.296_516_4 * x;
            b[2] = 0.57 * b[2] + 1.052_691_3 * x;
            b[0] + b[1] + b[2] + 0.1848 * x
        })
        .collect()
}

#[test]
fn averaged_noise_windows_steady_the_floor() {
    // A capped window in a minimum post-roll, as a long default sweep has; the floor of a
    // low fundamental's H2 is read the way the analysis reads it.
    let w_len = (MAX_WINDOW_S * FS) as usize;
    let w = taper(w_len, w_len / 3, w_len / 7);
    let nw = 4 * w_len.next_power_of_two();
    let end = ((MIN_POST_ROLL_S - NOISE_MARGIN_S) * FS) as i64;
    let count = ((MIN_POST_ROLL_S - NOISE_MARGIN_S - NOISE_REGION_START * MIN_POST_ROLL_S)
        / MAX_WINDOW_S) as usize;
    assert!(count >= 3, "{count} windows");
    let min_hz = 2.0 * DISTORTION_MIN_CELLS / MAX_WINDOW_S;
    let floor = |count: usize, seed: u64| {
        let h = pink((MIN_POST_ROLL_S * FS) as usize, seed);
        let p = noise_power(&h, end, count, &w, nw);
        db10(band_power(
            &p,
            FS / nw as f64,
            2.0 * 25.0,
            FLOOR_BAND_OCT,
            min_hz,
        ))
    };
    let spread = |count: usize| {
        let v: Vec<f64> = (1..=32).map(|seed| floor(count, seed)).collect();
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        (v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
    };
    let (one, many) = (spread(1), spread(count));
    eprintln!("noise floor spread: 1 window {one:.2} dB, {count} windows {many:.2} dB");
    assert!(many < 0.7 * one, "{one} vs {many}");
}
