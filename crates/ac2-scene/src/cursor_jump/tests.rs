use super::*;

/// 1/48-octave columns from 20 Hz to 20 kHz, as a transfer grid has.
fn grid() -> Vec<f64> {
    let n = (48.0 * (1000.0f64).log2()).round() as usize;
    (0..=n).map(|k| 20.0 * 2f64.powf(k as f64 / 48.0)).collect()
}

/// A bump of `db` at `f0`, Gaussian in octaves with standard deviation `width_oct`.
fn bump(f: f64, f0: f64, db: f64, width_oct: f64) -> f64 {
    let x = (f / f0).log2() / width_oct;
    db * (-0.5 * x * x).exp()
}

const FULL: Range = Range::new(20.0, 20_000.0);

/// Within half a column (1/96 octave) of `want`.
fn near(got: f64, want: f64) -> bool {
    (got / want).log2().abs() <= 1.0 / 96.0
}

#[test]
fn prominence_decides_which_bumps_count() {
    let f = grid();
    let v: Vec<f64> = f
        .iter()
        .map(|&x| bump(x, 100.0, 6.0, 0.2) + bump(x, 2000.0, 2.0, 0.2))
        .collect();
    let at3 = landmarks(&f, &v, FULL, Landmark::Peak, 3.0);
    assert_eq!(at3.len(), 1, "{at3:?}");
    assert!(near(at3[0], 100.0), "{at3:?}");
    let at1 = landmarks(&f, &v, FULL, Landmark::Peak, 1.0);
    assert_eq!(at1.len(), 2, "{at1:?}");
    assert!(near(at1[1], 2000.0), "{at1:?}");
    // The same curve upside down: its bumps are notches. The level between them stands
    // 2 dB over the shallower one: a peak at 1 dB, none at 3 dB.
    let inv: Vec<f64> = v.iter().map(|x| -x).collect();
    assert_eq!(landmarks(&f, &inv, FULL, Landmark::Notch, 1.0), at1);
    assert_eq!(landmarks(&f, &inv, FULL, Landmark::Peak, 1.0).len(), 1);
    assert!(landmarks(&f, &inv, FULL, Landmark::Peak, 3.0).is_empty());
}

#[test]
fn comb_notches_are_stepped_one_by_one() {
    // A copy 1 ms late summed in: |1 + e^(−j2πfτ)| has its notches at 500, 1500, 2500 Hz …
    let f = grid();
    let tau = 0.001;
    let v: Vec<f64> = f
        .iter()
        .map(|&x| {
            20.0 * (2.0 * (std::f64::consts::PI * x * tau).cos().abs())
                .max(1e-6)
                .log10()
        })
        .collect();
    let shown = Range::new(200.0, 4000.0);
    let up = |from| jump(&f, &v, shown, from, Landmark::Notch, Step::Up, 3.0);
    let first = up(None).expect("first notch");
    assert!(near(first, 500.0), "{first}");
    let second = up(Some(first)).expect("second notch");
    assert!(near(second, 1500.0), "{second}");
    let third = up(Some(second)).expect("third notch");
    assert!(near(third, 2500.0), "{third}");
    let fourth = up(Some(third)).expect("fourth notch");
    assert!(near(fourth, 3500.0), "{fourth}");
    assert_eq!(up(Some(fourth)), None);
    let down = jump(&f, &v, shown, Some(third), Landmark::Notch, Step::Down, 3.0);
    assert_eq!(down, Some(second));
    assert_eq!(
        jump(&f, &v, shown, None, Landmark::Notch, Step::Down, 3.0),
        Some(fourth)
    );
    // The peaks between them, at 1, 2, 3 kHz (and 0 Hz, off the range).
    let p = landmarks(&f, &v, shown, Landmark::Peak, 3.0);
    assert_eq!(p.len(), 3, "{p:?}");
    assert!(near(p[0], 1000.0) && near(p[2], 3000.0), "{p:?}");
    // A cursor between two notches goes to the nearer one each way.
    let from = Some(1000.0);
    let up = jump(&f, &v, shown, from, Landmark::Notch, Step::Up, 3.0).expect("up");
    let down = jump(&f, &v, shown, from, Landmark::Notch, Step::Down, 3.0).expect("down");
    assert!(near(up, 1500.0) && near(down, 500.0), "{down} {up}");
}

#[test]
fn only_the_shown_range_counts() {
    let f = grid();
    let v: Vec<f64> = f
        .iter()
        .map(|&x| bump(x, 100.0, 6.0, 0.2) + bump(x, 2000.0, 6.0, 0.2))
        .collect();
    // Zoomed past the 100 Hz bump: only the other one.
    let p = landmarks(&f, &v, Range::new(500.0, 20_000.0), Landmark::Peak, 3.0);
    assert_eq!(p.len(), 1);
    assert!(near(p[0], 2000.0));
    // The range edge cuts a bump on its way down: its top is off screen, and the column at
    // the edge is no peak (whether the curve turns there is not shown).
    let p = landmarks(&f, &v, Range::new(2100.0, 20_000.0), Landmark::Peak, 0.5);
    assert!(p.is_empty(), "{p:?}");
    // A curve rising to the end of its columns has no peak at the end.
    let rising: Vec<f64> = f.iter().map(|x| x.log2()).collect();
    assert!(landmarks(&f, &rising, FULL, Landmark::Peak, 0.0).is_empty());
    assert!(landmarks(&f, &rising, FULL, Landmark::Notch, 0.0).is_empty());
}

#[test]
fn gaps_and_flat_curves() {
    let f = grid();
    let flat = vec![-3.0; f.len()];
    assert!(landmarks(&f, &flat, FULL, Landmark::Peak, 0.0).is_empty());
    assert!(landmarks(&f, &flat, FULL, Landmark::Notch, 0.0).is_empty());
    assert_eq!(
        jump(&f, &flat, FULL, Some(1000.0), Landmark::Peak, Step::Up, 3.0),
        None
    );
    let mut v: Vec<f64> = f.iter().map(|&x| bump(x, 1000.0, 6.0, 0.2)).collect();
    let top = f
        .iter()
        .position(|x| near(*x, 1000.0))
        .expect("1 kHz column");
    // A blanked column away from the bump changes nothing.
    v[top + 60] = f64::NAN;
    let p = landmarks(&f, &v, FULL, Landmark::Peak, 3.0);
    assert_eq!(p, vec![f[top]]);
    // Blank the top itself: the columns beside the gap are its edges, not peaks.
    v[top] = f64::NAN;
    assert!(landmarks(&f, &v, FULL, Landmark::Peak, 3.0).is_empty());
    // A flat top counts once, at its first column.
    let mut v: Vec<f64> = f.iter().map(|&x| bump(x, 1000.0, 6.0, 0.2)).collect();
    v[top + 1] = v[top];
    assert_eq!(landmarks(&f, &v, FULL, Landmark::Peak, 3.0), vec![f[top]]);
    // An all-NaN curve, and mismatched lengths, find nothing and do not panic.
    let nan = vec![f64::NAN; f.len()];
    assert!(landmarks(&f, &nan, FULL, Landmark::Peak, 0.0).is_empty());
    assert!(landmarks(&f, &v[..10], FULL, Landmark::Peak, 0.0).is_empty());
    assert!(landmarks(&[], &[], FULL, Landmark::Peak, 0.0).is_empty());
}

#[test]
fn prominence_setting() {
    assert_eq!(DEFAULT_PROMINENCE_DB, 3.0);
    assert_eq!(step_prominence(3.0, 1), 6.0);
    assert_eq!(step_prominence(3.0, -1), 2.0);
    assert_eq!(step_prominence(1.0, -1), 1.0);
    assert_eq!(step_prominence(20.0, 1), 20.0);
    assert_eq!(step_prominence(4.5, 1), 6.0);
    assert_eq!(step_prominence(4.5, -1), 3.0);
    assert_eq!(step_prominence(3.0, 0), 3.0);
    assert_eq!(prominence_text(3.0), "stands out by 3 dB");
    assert_eq!(prominence_text(10.0), "stands out by 10 dB");
    assert_eq!(
        none_text(Landmark::Peak, Step::Up, 3.0),
        "no higher peak in view stands out by 3 dB"
    );
    assert_eq!(
        none_text(Landmark::Notch, Step::Down, 6.0),
        "no lower notch in view stands out by 6 dB"
    );
}
