use super::*;

fn quiet(coh: f64, rms: f64) -> Reading {
    Reading {
        ref_peak_dbfs: Some(-20.0),
        meas_rms_dbfs: Some(rms),
        coherence: Some(coh),
        ..Reading::default()
    }
}

fn hot(peak: f64) -> Reading {
    Reading {
        ref_peak_dbfs: Some(peak),
        meas_rms_dbfs: Some(-30.0),
        coherence: Some(0.95),
        ..Reading::default()
    }
}

/// Steps `h` with `r` every 0.1 s from `t0` to `t1` (inclusive of `t0`).
fn run(h: &mut Hold, t0: f64, t1: f64, r: &Reading, s: &AdviceSettings) {
    let mut t = t0;
    while t <= t1 + 1e-9 {
        h.step(t, r, s);
        t += 0.1;
    }
}

#[test]
fn defaults_are_named_choices() {
    let s = AdviceSettings::default();
    assert!(s.on);
    assert!(is_choice(&REF_HEADROOM_DB, s.ref_headroom_db));
    assert!(is_choice(&LOW_COHERENCE, s.low_coherence));
    assert!(is_choice(&LOW_LEVEL_DBFS, s.low_level_dbfs));
    assert_eq!(step_choice(&LOW_LEVEL_DBFS, -50.0, 1), -45.0);
    assert_eq!(step_choice(&LOW_LEVEL_DBFS, -50.0, -1), -55.0);
    assert_eq!(step_choice(&REF_HEADROOM_DB, 12.0, 1), 12.0);
    // A hand-edited value between choices steps to its neighbours.
    assert_eq!(step_choice(&REF_HEADROOM_DB, 4.0, 1), 6.0);
    assert_eq!(step_choice(&REF_HEADROOM_DB, 4.0, -1), 2.0);
    assert_eq!(
        ref_headroom_text(3.0),
        "reference peak within 3 dB of full scale"
    );
    assert_eq!(low_coherence_text(0.7), "coherence under γ² 0.70");
    assert_eq!(
        low_level_text(-50.0),
        "measurement input under −50.0 dBFS RMS"
    );
}

#[test]
fn a_reference_near_full_scale_advises_lowering_at_once() {
    let s = AdviceSettings::default();
    let mut h = Hold::default();
    h.step(0.0, &hot(-2.4), &s);
    let a = h.shown().expect("lower");
    assert_eq!(
        text(&a),
        (
            "LOWER STIMULUS · ref peak −2.4 dBFS".to_string(),
            "within 3 dB of 0 dBFS".to_string()
        )
    );
    // The headline keeps the highest peak: it only ever moves up while shown.
    h.step(0.1, &hot(-1.1), &s);
    h.step(0.2, &hot(-2.9), &s);
    assert_eq!(
        text(&h.shown().expect("advice")).0,
        "LOWER STIMULUS · ref peak −1.1 dBFS"
    );
    // Just under the threshold is not enough to start it.
    let mut h = Hold::default();
    h.step(0.0, &hot(-3.5), &s);
    assert_eq!(h.shown(), None);
}

#[test]
fn lower_holds_within_the_hysteresis_and_for_the_hold_time() {
    let s = AdviceSettings::default();
    let mut h = Hold::default();
    h.step(0.0, &hot(-2.0), &s);
    // Peaks swinging between −2 and −5.5 dBFS (inside threshold + hysteresis): stays, the
    // clear timer never starts.
    for i in 1..100 {
        let p = if i % 2 == 0 { -2.5 } else { -5.5 };
        h.step(f64::from(i) * 0.1, &hot(p), &s);
        assert!(h.shown().is_some(), "flickered off at step {i}");
    }
    // Back well under: still shown for HOLD_S, then gone.
    run(&mut h, 10.0, 10.0 + HOLD_S - 0.2, &hot(-12.0), &s);
    assert!(h.shown().is_some());
    run(
        &mut h,
        10.0 + HOLD_S - 0.1,
        10.0 + HOLD_S + 0.2,
        &hot(-12.0),
        &s,
    );
    assert_eq!(h.shown(), None);
}

#[test]
fn low_coherence_with_a_quiet_input_advises_raising_after_it_settles() {
    let s = AdviceSettings::default();
    let mut h = Hold::default();
    let r = quiet(0.42, -64.3);
    run(&mut h, 0.0, RAISE_AFTER_S - 0.2, &r, &s);
    assert_eq!(
        h.shown(),
        None,
        "a just-started stimulus is still averaging"
    );
    run(&mut h, RAISE_AFTER_S - 0.1, RAISE_AFTER_S + 0.1, &r, &s);
    let a = h.shown().expect("raise");
    // Aims 3 dB past −50: −47 − (−64.3) = 17.3 → 18 dB.
    assert_eq!(
        text(&a),
        (
            "RAISE STIMULUS · about +18 dB".to_string(),
            "γ² 0.42 · meas −64.3 dBFS RMS".to_string()
        )
    );
}

#[test]
fn either_condition_alone_gives_no_raise() {
    let s = AdviceSettings::default();
    for r in [quiet(0.95, -70.0), quiet(0.3, -30.0)] {
        let mut h = Hold::default();
        run(&mut h, 0.0, 10.0, &r, &s);
        assert_eq!(h.shown(), None, "{r:?}");
    }
    let mut h = Hold::default();
    run(
        &mut h,
        0.0,
        10.0,
        &Reading {
            coherence: None,
            ..quiet(0.3, -70.0)
        },
        &s,
    );
    assert_eq!(h.shown(), None, "no transfer frame, no coherence");
}

#[test]
fn a_brief_dip_never_shows_raise() {
    let s = AdviceSettings::default();
    let mut h = Hold::default();
    // Low for 1.5 s, fine for 0.5 s, low again for 1.5 s: never 2 s in a row.
    run(&mut h, 0.0, 1.5, &quiet(0.4, -60.0), &s);
    run(&mut h, 1.6, 2.1, &quiet(0.9, -60.0), &s);
    run(&mut h, 2.2, 3.6, &quiet(0.4, -60.0), &s);
    assert_eq!(h.shown(), None);
}

#[test]
fn raise_step_and_text_hold_through_jitter() {
    let s = AdviceSettings::default();
    let mut h = Hold::default();
    run(&mut h, 0.0, 3.0, &quiet(0.42, -64.3), &s);
    let first = text(&h.shown().expect("advice"));
    // Coherence and level jitter (inside the hysteresis, and the step moves < 3 dB): the
    // banner's text stays exactly the same.
    for (i, (c, rms)) in [(0.5, -63.0), (0.73, -65.5), (0.6, -63.5), (0.74, -62.8)]
        .iter()
        .enumerate()
    {
        h.step(3.1 + i as f64 * 0.1, &quiet(*c, *rms), &s);
        assert_eq!(text(&h.shown().expect("advice")), first, "step {i}");
    }
    // A step that moved by 3 dB or more is shown.
    h.step(4.0, &quiet(0.5, -68.0), &s);
    assert_eq!(
        text(&h.shown().expect("advice")).0,
        "RAISE STIMULUS · about +21 dB"
    );
    // Above the release (γ² ≥ 0.75): held for HOLD_S, then gone.
    run(&mut h, 4.1, 4.1 + HOLD_S - 0.2, &quiet(0.8, -68.0), &s);
    assert!(h.shown().is_some());
    run(
        &mut h,
        4.1 + HOLD_S - 0.1,
        4.1 + HOLD_S + 0.2,
        &quiet(0.8, -68.0),
        &s,
    );
    assert_eq!(h.shown(), None);
}

#[test]
fn the_max_level_caps_the_raise_and_says_so() {
    let s = AdviceSettings::default();
    let stim = |level: f64| Stimulus {
        level_dbfs: level,
        max_level_dbfs: -50.0,
    };
    let with = |level: f64| Reading {
        stimulus: Some(stim(level)),
        ..quiet(0.42, -64.3)
    };
    // Room for the whole step.
    let mut h = Hold::default();
    run(&mut h, 0.0, 3.0, &with(-80.0), &s);
    assert_eq!(
        text(&h.shown().expect("advice")).0,
        "RAISE STIMULUS · about +18 dB"
    );
    // 4.5 dB of room: +4 dB, never above the max level, and the text says the rest is gain.
    let mut h = Hold::default();
    run(&mut h, 0.0, 3.0, &with(-54.5), &s);
    assert_eq!(
        text(&h.shown().expect("advice")),
        (
            "RAISE STIMULUS · +4 dB to max level".to_string(),
            "then raise the meas gain".to_string()
        )
    );
    assert_eq!(
        raise_by(18.0, Some(stim(-54.5))),
        RaiseBy::ToMax {
            by_db: 4.0,
            max_level_dbfs: -50.0
        }
    );
    // At the max level: nothing to raise, the input gain is what is left.
    let mut h = Hold::default();
    run(&mut h, 0.0, 3.0, &with(-50.4), &s);
    assert_eq!(
        text(&h.shown().expect("advice")),
        (
            "STIMULUS AT MAX LEVEL · −50.0 dBFS".to_string(),
            "raise the meas gain".to_string()
        )
    );
    // The operator raising the stimulus updates the cap at once (no flicker: a deliberate
    // change).
    let mut h = Hold::default();
    run(&mut h, 0.0, 3.0, &with(-60.0), &s);
    assert_eq!(
        text(&h.shown().expect("advice")).0,
        "RAISE STIMULUS · +10 dB to max level"
    );
    h.step(3.1, &with(-51.0), &s);
    assert_eq!(
        text(&h.shown().expect("advice")).0,
        "RAISE STIMULUS · +1 dB to max level"
    );
}

#[test]
fn a_fault_clears_the_advice_and_off_shows_none() {
    let s = AdviceSettings::default();
    let mut h = Hold::default();
    h.step(0.0, &hot(-1.0), &s);
    assert!(h.shown().is_some());
    h.step(
        0.1,
        &Reading {
            protection: ProtectionFlags::CLIP,
            ..hot(-0.1)
        },
        &s,
    );
    assert_eq!(h.shown(), None);
    let off = AdviceSettings { on: false, ..s };
    let mut h = Hold::default();
    run(&mut h, 0.0, 5.0, &quiet(0.3, -70.0), &off);
    h.step(5.1, &hot(-0.5), &off);
    assert_eq!(h.shown(), None);
}

#[test]
fn lower_wins_over_raise() {
    let s = AdviceSettings::default();
    let mut h = Hold::default();
    run(&mut h, 0.0, 3.0, &quiet(0.4, -65.0), &s);
    assert!(matches!(h.shown(), Some(Advice::Raise { .. })));
    h.step(
        3.1,
        &Reading {
            ref_peak_dbfs: Some(-1.0),
            ..quiet(0.4, -65.0)
        },
        &s,
    );
    assert!(matches!(h.shown(), Some(Advice::Lower { .. })));
}

#[test]
fn pane_coherence_is_the_median_of_the_judged_columns() {
    let v = ValidityMask::NONE;
    let coh = [0.9, 0.2, f32::NAN, 0.8, 0.1, 0.0];
    let validity = [
        v,
        v,
        ValidityMask::BELOW_FLOOR,
        v,
        ValidityMask::SETTLING,
        ValidityMask::OUT_OF_BAND,
    ];
    // Judged: 0.9, 0.2, NaN below floor → 0, 0.8 → median of [0, 0.2, 0.8, 0.9] = 0.5.
    let m = pane_coherence(&coh, &validity).expect("judged columns");
    assert!((m - 0.5).abs() < 1e-6, "{m}");
    assert_eq!(pane_coherence(&[0.5], &[ValidityMask::NO_REFERENCE]), None);
}

/// A held average (weak reference) freezes the advice: neither started nor cleared on the
/// coherence from before the drop.
#[test]
fn a_held_average_freezes_the_advice() {
    let s = AdviceSettings::default();
    let weak = |r: Reading| Reading {
        protection: ProtectionFlags::WEAK_REFERENCE,
        ..r
    };
    let mut h = Hold::default();
    run(&mut h, 0.0, 10.0, &weak(quiet(0.3, -70.0)), &s);
    assert_eq!(h.shown(), None);
    run(&mut h, 10.1, 13.0, &quiet(0.3, -70.0), &s);
    let shown = h.shown().expect("raise");
    run(&mut h, 13.1, 30.0, &weak(quiet(0.95, -30.0)), &s);
    assert_eq!(h.shown(), Some(shown));
}
