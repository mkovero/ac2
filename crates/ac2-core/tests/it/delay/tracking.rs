//! Delay finder tracking through the stream API, and the tracker's rules.

use super::*;

/// Push a scene through a [`DelayStream`] in 1024-sample blocks; return each window's
/// result.
fn stream_results(cfg: FinderConfig, r: &[f32], m: &[f32]) -> Vec<FinderResult> {
    let mut s = DelayStream::new(cfg).expect("config");
    let mut out = Vec::new();
    for (i, (rc, mc)) in r.chunks(1024).zip(m.chunks(1024)).enumerate() {
        let start = (i * 1024) as u64;
        s.push_ref(Block { start, samples: rc });
        s.push_meas(Block { start, samples: mc });
        while let Some(res) = s.poll().expect("config") {
            out.push(res);
        }
    }
    out
}

fn track(results: &[FinderResult], agreement: Agreement) -> Vec<Option<f64>> {
    let mut t = Tracker::new(agreement);
    results.iter().map(|r| t.observe(r)).collect()
}

#[test]
fn tracking_full_locks_in_two_windows_and_relocks_after_a_step() {
    let cfg = full_cfg(2400);
    let lm = cfg.observation_len();
    assert_eq!(lm, 12_000);
    let n_win = 6;
    // windows start at 2400 (the first index whose largest lag has ref)
    let change = 2400 + 3 * lm;
    let n = 2400 + n_win * lm + 2400 + 1024;
    let x = pink(n, 0.1, 99);
    let (d1, d2) = (412.3, 1187.8);
    let m1 = paths(&x, &[(d1, db(-6.0)), (d1 + 300.0, -1.0)]);
    let m2 = paths(&x, &[(d2, db(-6.0)), (d2 + 300.0, -1.0)]);
    let noise = pink(n, 0.1 * db(-30.0), 1234);
    let m: Vec<f64> = (0..n)
        .map(|i| if i < change { m1[i] } else { m2[i] } + noise[i])
        .collect();
    let results = stream_results(cfg, &to_f32(&x), &to_f32(&m));
    assert!(results.len() >= n_win, "{} windows", results.len());
    for w in results.windows(2) {
        assert_eq!(w[0].meas_window.end, w[1].meas_window.start, "back to back");
    }
    assert_eq!(results[3].meas_window.start, change as u64);
    let moves = track(&results, Agreement::for_band(Band::FullRange, FS));
    println!("full tracking moves: {moves:?}");
    assert_eq!(moves[0], None);
    assert_eq!(
        moves[1].map(f64::round),
        Some(412.0),
        "lock after two windows"
    );
    assert_eq!(moves[2], None);
    assert_eq!(
        moves[3], None,
        "first window after the step is only pending"
    );
    assert_eq!(
        moves[4].map(f64::round),
        Some(1188.0),
        "relock two windows after the step"
    );
}

#[test]
fn tracking_never_moves_on_ambiguous_results() {
    let cfg = full_cfg(2400);
    let n = 2400 + 4 * 12_000 + 2400 + 1024;
    let (r, m) = scene(n, &[(300.0, db(-12.3)), (500.0, 1.0)], -30.0, 77);
    let results = stream_results(cfg, &r, &m);
    assert!(results.len() >= 4);
    for res in &results {
        assert!(
            matches!(res.outcome, Outcome::Ambiguous { .. }),
            "{:?}",
            res.outcome
        );
    }
    let moves = track(&results, Agreement::for_band(Band::FullRange, FS));
    assert!(moves.iter().all(Option::is_none), "{moves:?}");
}

/// A hand-made result for rule tests.
fn result(outcome: Outcome, window: std::ops::Range<u64>) -> FinderResult {
    FinderResult {
        outcome,
        candidates: vec![],
        confidence: Confidence {
            psr_db: 40.0,
            psr_acq_db: 25.0,
            band_snr_db: 30.0,
            excited_fraction: 1.0,
            pulse_width: 4.0,
            nominal_width: 4.0,
            period: None,
            refinement_window: None,
        },
        band: Band::FullRange,
        meas_window: window,
    }
}

fn arrival(d: f64) -> Arrival {
    Arrival {
        delay: d.round() as i64,
        delay_frac: d,
        level_db: 0.0,
        phase_deg: 0.0,
        uncertainty: 0.1,
        misfit: 0.0,
        refined: true,
    }
}

fn accepted(d: f64, k: u64) -> FinderResult {
    result(
        Outcome::Accepted {
            first: arrival(d),
            strongest: arrival(d),
        },
        k * 100..(k + 1) * 100,
    )
}

#[test]
fn tracker_rules() {
    let full = Agreement::for_band(Band::FullRange, FS);
    let mut t = Tracker::new(full);
    // an ambiguous window between two agreeing ones clears the pending result
    assert_eq!(t.observe(&accepted(1000.2, 0)), None);
    let amb = result(
        Outcome::Ambiguous {
            reasons: vec![AmbiguityReason::CloseArrivals],
            ranked: [arrival(1000.2)].into_iter().collect(),
            strongest: arrival(1000.2),
        },
        100..200,
    );
    assert_eq!(t.observe(&amb), None);
    assert_eq!(t.observe(&accepted(1000.2, 2)), None);
    // overlapping windows are ignored
    assert_eq!(
        t.observe(&result(
            Outcome::Accepted {
                first: arrival(1000.0),
                strongest: arrival(1000.0)
            },
            250..350
        )),
        None
    );
    assert_eq!(t.observe(&accepted(1000.9, 3)), Some(1001.0));
    assert_eq!(t.held(), Some(1001));
    // a refusal clears pending too
    assert_eq!(t.observe(&accepted(1000.9, 4)), None);
    assert_eq!(
        t.observe(&result(
            Outcome::NoEstimate {
                reasons: vec![NoEstimateReason::LowPsr]
            },
            500..600
        )),
        None
    );
    assert_eq!(t.observe(&accepted(2000.0, 6)), None);
    assert_eq!(t.held(), Some(1001));
    // ±1 sample in the full band: 2.2 samples apart never agree
    assert_eq!(t.observe(&accepted(2002.2, 7)), None);
    assert_eq!(t.observe(&accepted(2000.0, 8)), None);
    t.reset();
    assert_eq!(t.observe(&accepted(2000.0, 9)), None);
    assert_eq!(t.observe(&accepted(2000.4, 10)), Some(2000.0));
}

#[test]
fn tracker_sub_agreement_is_a_tenth_of_a_millisecond() {
    let sub = Agreement::for_band(Band::Sub, FS);
    assert!((sub.samples - 4.8).abs() < 1e-12);
    let mut t = Tracker::new(sub);
    assert_eq!(t.observe(&accepted(1000.0, 0)), None);
    assert_eq!(t.observe(&accepted(1004.7, 1)), Some(1005.0));
    assert_eq!(t.observe(&accepted(1000.0, 2)), None);
    assert_eq!(
        t.observe(&accepted(1004.9, 3)),
        None,
        "4.9 samples > 0.1 ms"
    );
    // the same 4.7-sample spread never agrees in the full band
    let mut f = Tracker::new(Agreement::for_band(Band::FullRange, FS));
    assert_eq!(f.observe(&accepted(1000.0, 0)), None);
    assert_eq!(f.observe(&accepted(1004.7, 1)), None);
}

/// Sub band through the stream at the default 4 s observation: lock after two windows (8 s).
#[test]
fn tracking_sub_locks_in_two_windows() {
    let mut cfg = FinderConfig::new(FS, Band::Sub);
    cfg.search = SearchRange {
        min: -9600,
        max: 9600,
    };
    let lm = cfg.observation_len();
    assert_eq!(lm, 192_000);
    let n = 9600 + 2 * lm + 9600 + 1024;
    let (r, m) = scene(n, &[(-600.3, 1.0)], -40.0, 4242);
    let results = stream_results(cfg, &r, &m);
    assert_eq!(results.len(), 2);
    for res in &results {
        let a = res.accepted().expect("accepted");
        assert!((a.delay_frac + 600.3).abs() <= 4.8, "{a:?}");
    }
    let moves = track(&results, Agreement::for_band(Band::Sub, FS));
    assert_eq!(moves[0], None);
    assert!(
        matches!(moves[1], Some(d) if (d + 600.0).abs() <= 5.0),
        "{moves:?}"
    );
}

#[test]
fn tracker_inserts_the_fraction_once_it_repeats() {
    let mut t = Tracker::new(Agreement::for_band(Band::FullRange, FS));
    // 0.3 samples apart: agreeing, but only to the whole sample
    assert_eq!(t.observe(&accepted(1000.2, 0)), None);
    assert_eq!(t.observe(&accepted(1000.45, 1)), Some(1000.0));
    // within a tenth of a sample: the pair's fractional mean
    assert_eq!(t.observe(&accepted(1000.36, 2)), None);
    let fine = t.observe(&accepted(1000.40, 3)).expect("fractional insert");
    assert!((fine - 1000.38).abs() < 1e-9, "{fine}");
    assert_eq!(t.held(), Some(1000));
    // a fraction that stays within the agreement of the insert does not move it
    assert_eq!(t.observe(&accepted(1000.44, 4)), None);
    assert_eq!(t.observe(&accepted(1000.47, 5)), None);
    // nor does a looser pair on the same whole sample
    assert_eq!(t.observe(&accepted(1000.1, 6)), None);
    assert_eq!(t.observe(&accepted(1000.4, 7)), None);
    // a fraction across the half-sample, near the insert, stays put although it rounds up
    assert_eq!(t.observe(&accepted(1000.43, 8)), None);
    assert_eq!(t.observe(&accepted(1000.50, 9)), None);
    assert_eq!(t.held(), Some(1001));
    // the arrival moves on: the new fraction is inserted
    assert_eq!(t.observe(&accepted(1003.71, 10)), None);
    let fine = t
        .observe(&accepted(1003.75, 11))
        .expect("fractional insert");
    assert!((fine - 1003.73).abs() < 1e-9, "{fine}");
}
