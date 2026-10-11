//! Delay finder on synthetic scenes: index/sign invariance, refusal paths, merged lobes
//! and the auto band.

use super::*;

#[test]
fn index_invariance_and_sign() {
    let (r, m) = scene(60_000, &[(321.4, 1.0)], -30.0, 11);
    let cfg = full_cfg(2400);
    let (rb, mb) = full_window(&r, &m, 20_000, 2400);
    let base = find(rb, mb, &cfg).expect("config");
    let first = *base.accepted().expect("accepted");
    assert!((first.delay_frac - 321.4).abs() < 0.1, "{first:?}");
    assert_eq!(first.delay, 321);

    // shifting both block starts by the same amount changes nothing
    let shift = 7_000_000_123u64;
    let moved = find(
        Block {
            start: rb.start + shift,
            ..rb
        },
        Block {
            start: mb.start + shift,
            ..mb
        },
        &cfg,
    )
    .expect("config");
    assert_eq!(moved.outcome, base.outcome);
    assert_eq!(moved.meas_window, mb.start + shift..mb.end_index() + shift);

    // swapping the roles of ref and meas negates a single-path delay
    let swapped = find(
        Block {
            start: 20_000 - 2400,
            samples: &m[20_000 - 2400..32_000 + 2400],
        },
        Block {
            start: 20_000,
            samples: &r[20_000..32_000],
        },
        &cfg,
    )
    .expect("config");
    let neg = swapped.accepted().expect("accepted when swapped");
    assert!((neg.delay_frac + 321.4).abs() < 0.1, "{neg:?}");
}

trait EndIndex {
    fn end_index(&self) -> u64;
}

impl EndIndex for Block<'_> {
    fn end_index(&self) -> u64 {
        self.start + self.samples.len() as u64
    }
}

fn blk(samples: &[f32], start: u64) -> Block<'_> {
    Block { start, samples }
}

fn no_estimate(r: &FinderResult) -> &[NoEstimateReason] {
    match &r.outcome {
        Outcome::NoEstimate { reasons } => reasons,
        other => panic!("expected NoEstimate, got {other:?}"),
    }
}

#[test]
fn refusal_paths() {
    let cfg = full_cfg(2400);
    let (r, m) = scene(40_000, &[(100.0, 1.0)], -30.0, 5);
    let zeros = vec![0.0f32; 20_000];

    let res = find(blk(&zeros, 0), blk(&m[0..12_000], 2400), &cfg).expect("config");
    assert_eq!(no_estimate(&res), &[NoEstimateReason::NoReference]);
    let res = find(blk(&r[0..16_800], 0), blk(&zeros[0..12_000], 2400), &cfg).expect("config");
    assert_eq!(no_estimate(&res), &[NoEstimateReason::NoSignal]);
    let res = find(blk(&zeros, 0), blk(&zeros, 0), &cfg).expect("config");
    assert_eq!(
        no_estimate(&res),
        &[NoEstimateReason::NoReference, NoEstimateReason::NoSignal]
    );
    // N₂ = 8192 at 48 kHz full range
    let res = find(
        blk(&r[0..12_000], 0),
        blk(&m[2400..2400 + 8000], 2400),
        &cfg,
    )
    .expect("config");
    assert_eq!(no_estimate(&res), &[NoEstimateReason::ObservationTooShort]);
    // ref far away from every lag of the search range
    let res = find(blk(&r[0..12_000], 0), blk(&m[20_000..32_000], 20_000), &cfg).expect("config");
    assert_eq!(no_estimate(&res), &[NoEstimateReason::InsufficientOverlap]);
    // given periodic excitation: P ≤ span + tail
    let mut pcfg = cfg;
    pcfg.excitation_period = Some(8192);
    let (rb, mb) = full_window(&r, &m, 10_000, 2400);
    let res = find(rb, mb, &pcfg).expect("config");
    assert!(
        no_estimate(&res).contains(&NoEstimateReason::PeriodicExcitation { period: 8192 }),
        "{res:?}"
    );
    assert_eq!(res.confidence.period, Some(8192));
    // invalid configuration is an error, not a result
    let mut bad = cfg;
    bad.search = SearchRange { min: 5, max: -5 };
    assert!(find(rb, mb, &bad).is_err());
}

#[test]
fn ambiguous_lists_rule_pick_first() {
    // direct at −12.3 dB: within ±2 dB of the threshold → BorderlineLevel
    let (r, m) = scene(60_000, &[(300.0, db(-12.3)), (500.0, 1.0)], -30.0, 21);
    let (rb, mb) = full_window(&r, &m, 20_000, 2400);
    let res = find(rb, mb, &full_cfg(2400)).expect("config");
    match &res.outcome {
        Outcome::Ambiguous {
            reasons,
            ranked,
            strongest,
        } => {
            assert!(
                reasons.contains(&AmbiguityReason::BorderlineLevel),
                "{reasons:?}"
            );
            assert!(ranked.len() <= 3 && !ranked.is_empty());
            assert!((strongest.delay_frac - 500.0).abs() < 0.5);
            assert!(ranked.iter().any(|a| (a.delay_frac - 300.0).abs() < 1.0));
            assert_eq!(res.pick(), ranked.first());
        }
        other => panic!("expected Ambiguous, got {other:?}"),
    }
}

/// Filter by a complex response H(f) (circular).
fn respond(x: &[f64], h: impl Fn(f64) -> Complex64) -> Vec<f64> {
    let n = x.len();
    let mut planner = RealFftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let mut buf = x.to_vec();
    let mut spec = fwd.make_output_vec();
    fwd.process(&mut buf, &mut spec).expect("fft");
    for (k, s) in spec.iter_mut().enumerate() {
        *s *= h(k as f64 * FS / n as f64);
    }
    let last = spec.len() - 1;
    spec[0].im = 0.0;
    spec[last].im = 0.0;
    let mut out = vec![0.0; n];
    inv.process(&mut spec, &mut out).expect("ifft");
    out.iter().map(|v| v / n as f64).collect()
}

/// The acoustic sum of a Linkwitz–Riley 4th-order crossover at `f0`: a 2nd-order allpass
/// with Q = 1/√2. Flat in level, but its group delay (≈ 0.17 ms at 2.6 kHz) spreads one
/// arrival over several full-band pulse widths around f0.
fn lr4_sum(f: f64, f0: f64) -> Complex64 {
    let s = Complex64::new(0.0, f / f0);
    let q = std::f64::consts::FRAC_1_SQRT_2;
    (s * s - s / q + 1.0) / (s * s + s / q + 1.0)
}

/// The rig case: a three-way box (crossovers 430 Hz and 2.6 kHz) at 3.32 ms, floor bounce
/// 3.5 ms later at −23 dB, a late reflection at +17.9 ms, −18 dB. The floor bounce is far
/// outside the pulse width and does not merge; the 2.6 kHz crossover inside the full band
/// does. The finder must call the one peak a merged lobe and list it alone, never accept it
/// (its centre is not the first arrival's delay) and never invent a second arrival.
#[test]
fn crossover_in_band_is_a_merged_lobe_listed_alone() {
    let n = 60_000;
    let d0 = 0.00332 * FS;
    let arrivals = [
        (d0, 1.0),
        (d0 + 0.0035 * FS, db(-23.0)),
        (d0 + 0.0179 * FS, db(-18.0)),
    ];
    let x = pink(n, 0.1, 41);
    let room = paths(&x, &arrivals);
    let noise = pink(n, 0.1 * db(-30.0), 42);
    let build = |h: &dyn Fn(f64) -> Complex64| -> Vec<f32> {
        let m = respond(&room, h);
        to_f32(&m.iter().zip(&noise).map(|(a, b)| a + b).collect::<Vec<_>>())
    };
    let r = to_f32(&x);
    let cfg = full_cfg(2400);

    // Point source, same room: accepted at the direct sound.
    let m = build(&|_| Complex64::new(1.0, 0.0));
    let (rb, mb) = full_window(&r, &m, 20_000, 2400);
    let res = find(rb, mb, &cfg).expect("config");
    let first = res
        .accepted()
        .unwrap_or_else(|| panic!("{:?}", res.outcome));
    assert!((first.delay_frac - d0).abs() < 0.5, "{first:?}");

    // Three-way box: one merged peak, listed alone, explained by MergedLobe.
    let m = build(&|f| lr4_sum(f, 430.0) * lr4_sum(f, 2600.0));
    let (rb, mb) = full_window(&r, &m, 20_000, 2400);
    let res = find(rb, mb, &cfg).expect("config");
    match &res.outcome {
        Outcome::Ambiguous {
            reasons,
            ranked,
            strongest,
        } => {
            assert_eq!(reasons, &[AmbiguityReason::MergedLobe]);
            assert_eq!(ranked.len(), 1, "{ranked:?}");
            assert_eq!(ranked[0], *strongest);
            assert!(ranked[0].misfit > 0.10, "{:?}", ranked[0]);
            // The peak's centre lags the onset: accepting it would miss the full-band
            // 1-sample tolerance.
            assert!(ranked[0].delay_frac - d0 > 1.0, "{:?}", ranked[0]);
            assert!(res.candidates.contains(&ranked[0]));
        }
        other => panic!("expected Ambiguous{{MergedLobe}}, got {other:?}"),
    }
}

/// Filter by a zero-phase magnitude response.
fn shape(x: &[f64], mag: impl Fn(f64) -> f64) -> Vec<f64> {
    let n = x.len();
    let mut planner = RealFftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let mut buf = x.to_vec();
    let mut spec = fwd.make_output_vec();
    fwd.process(&mut buf, &mut spec).expect("fft");
    for (k, s) in spec.iter_mut().enumerate() {
        *s *= mag(k as f64 * FS / n as f64);
    }
    let mut out = vec![0.0; n];
    inv.process(&mut spec, &mut out).expect("ifft");
    out.iter().map(|v| v / n as f64).collect()
}

#[test]
fn auto_band_picks_from_the_excitation() {
    let cfg = full_cfg(2400);
    let n = 40_000;
    let blocks = |r: &[f32], m: &[f32]| -> (Vec<f32>, Vec<f32>) {
        (
            r[5000 - 2400..5000 + 24_000 + 2400].to_vec(),
            m[5000..5000 + 24_000].to_vec(),
        )
    };
    let run = |r: &[f32], m: &[f32]| {
        let (rb, mb) = blocks(r, m);
        find_auto(
            Block {
                start: 5000 - 2400,
                samples: &rb,
            },
            Block {
                start: 5000,
                samples: &mb,
            },
            &cfg,
        )
        .expect("config")
    };
    // pink: full range answers first
    let (r, m) = scene(n, &[(100.0, 1.0)], -30.0, 31);
    let res = run(&r, &m);
    assert_eq!(res.band, Band::FullRange);
    assert!(
        res.accepted().is_some_and(|a| a.delay == 100),
        "{:?}",
        res.outcome
    );
    // excitation rolling off above 1.2 kHz (8th-order), ref with a white floor 70 dB down:
    // full range refuses, mid answers
    let lp = |f: f64| 1.0 / (1.0 + (f / 1200.0).powi(16)).sqrt();
    let x = shape(&pink(n, 0.1, 32), lp);
    let mut m = paths(&x, &[(100.0, 1.0)]);
    for (v, e) in m.iter_mut().zip(shape(&pink(n, 0.003, 33), lp)) {
        *v += e;
    }
    let mut floor = Noise(34);
    let r: Vec<f64> = x
        .iter()
        .map(|v| v + 0.1 * db(-70.0) * floor.gauss())
        .collect();
    let full = {
        let (rb, mb) = blocks(&to_f32(&r), &to_f32(&m));
        find(
            Block {
                start: 5000 - 2400,
                samples: &rb,
            },
            Block {
                start: 5000,
                samples: &mb,
            },
            &cfg,
        )
        .expect("config")
    };
    println!(
        "full band on 1.2 kHz excitation: {:?} {:?}",
        full.outcome, full.confidence
    );
    assert!(!no_estimate(&full).is_empty());
    let res = run(&to_f32(&r), &to_f32(&m));
    assert_eq!(res.band, Band::Mid);
    assert!(
        res.accepted().is_some_and(|a| a.delay == 100),
        "{:?}",
        res.outcome
    );
}
