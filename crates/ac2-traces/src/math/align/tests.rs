use super::*;
use ac2_proto::units::{Hz, SessionEpoch};

fn freqs() -> Vec<f64> {
    (0..480)
        .map(|i| 20.0 * 2f64.powf(f64::from(i) / 48.0))
        .collect()
}

/// Columns of `h(f)` (magnitude dB, phase degrees), γ² 0.9.
fn columns(f: &[f64], h: impl Fn(f64) -> Complex64) -> Columns {
    Columns {
        mag_db: f
            .iter()
            .map(|&f| (20.0 * h(f).norm().log10()) as f32)
            .collect(),
        phase_deg: Some(f.iter().map(|&f| h(f).arg().to_degrees() as f32).collect()),
        coherence: Some(vec![0.9; f.len()]),
    }
}

fn band() -> AlignBand {
    AlignBand {
        lo: Hz(60.0),
        hi: Hz(120.0),
    }
}

#[test]
fn delay_and_polarity_come_back_through_the_operands_own_delays() {
    let f = freqs();
    // A reached the mic 13.7 ms in, B 10 ms in and inverted; each was measured with its
    // own arrival removed, so both columns read flat.
    let ca = columns(&f, |_| Complex64::new(1.0, 0.0));
    let cb = columns(&f, |_| Complex64::new(-1.0, 0.0));
    let epoch = Some(SessionEpoch(3));
    let a = Input {
        columns: &ca,
        delay: 0.0137,
        time_base: epoch,
    };
    let b = Input {
        columns: &cb,
        delay: 0.010,
        time_base: epoch,
    };
    let (r, st, cv) = align(&a, &b, &f, band(), Seconds(0.05)).expect("both operands are usable");
    let AlignOutcome::Fit { fit } = st.outcome else {
        panic!("{:?}", st.outcome)
    };
    assert_eq!(fit.best.delayed, AlignSide::B);
    assert!(fit.best.inverted);
    assert!((fit.best.delay.0 - 0.0037).abs() < 0.0037 * 0.01, "{fit:?}");
    assert_eq!(r.delay, Seconds(0.0137));
    assert_eq!(r.phase, PhaseBasis::SharedTimeBase);
    // Aligned, the two add fully: +6 dB everywhere, and in phase with A.
    for i in 0..f.len() {
        assert!((r.columns.mag_db[i] - 6.0206).abs() < 0.01, "{i}");
        assert!(cv.phase_after[i].abs() < 0.5, "{i}");
        assert!(cv.sum_after[i] > -0.01);
    }
    assert!(fit.before.worst_db < -3.0, "{:?}", fit.before);
    assert!(fit.after.mean_db > -0.01);
}

#[test]
fn an_alignment_needs_phase_and_one_time_base() {
    let f = freqs();
    let ca = columns(&f, |_| Complex64::new(1.0, 0.0));
    let mut no_phase = ca.clone();
    no_phase.phase_deg = None;
    let inp = |c, e| Input {
        columns: c,
        delay: 0.0,
        time_base: e,
    };
    let e = Some(SessionEpoch(1));
    assert_eq!(
        align(&inp(&ca, e), &inp(&no_phase, e), &f, band(), Seconds(0.05)).expect_err("refused"),
        MathError::NoPhase(1)
    );
    assert_eq!(
        align(
            &inp(&ca, e),
            &inp(&ca, Some(SessionEpoch(2))),
            &f,
            band(),
            Seconds(0.05)
        )
        .expect_err("refused"),
        MathError::NoSharedTimeBase
    );
    assert_eq!(
        align(&inp(&ca, None), &inp(&ca, None), &f, band(), Seconds(0.05)).expect_err("refused"),
        MathError::NoSharedTimeBase
    );
}

#[test]
fn a_band_off_the_grid_publishes_the_plain_sum() {
    let f = freqs();
    let ca = columns(&f, |_| Complex64::new(1.0, 0.0));
    let e = Some(SessionEpoch(1));
    let a = Input {
        columns: &ca,
        delay: 0.0,
        time_base: e,
    };
    let far = AlignBand {
        lo: Hz(30_000.0),
        hi: Hz(40_000.0),
    };
    let (r, st, _) = align(&a, &a, &f, far, Seconds(0.05)).expect("both operands are usable");
    assert_eq!(
        st.outcome,
        AlignOutcome::NoFit {
            why: AlignNoFit::FewColumns { found: 0 }
        }
    );
    assert!((r.columns.mag_db[100] - 6.0206).abs() < 0.01);
}
