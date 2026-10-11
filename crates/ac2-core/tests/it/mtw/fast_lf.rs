//! MTW fast-LF depth policy.

use super::*;

/// Per-stage depth under both policies. Fast LF leaves the full-rate stage alone, keeps
/// every decimated stage's averaging span within the cap (or at the documented floor), and
/// the effective averages it reports drop accordingly.
#[test]
fn fast_lf_depth_table() {
    let cap = DepthPolicy::DEFAULT_MAX_SETTLE_S;
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        for averaging in [
            Averaging::Fifo { blocks: 8 },
            Averaging::Exponential {
                time_constant_s: 0.15,
            },
        ] {
            let eq = engine(sr, averaging, 0).stage_depths();
            let fast = engine_with(sr, averaging, DepthPolicy::fast_lf(), 0).stage_depths();
            for (policy, d) in [("equal", &eq), ("fastlf", &fast)] {
                for (s, d) in d.iter().enumerate() {
                    println!(
                        "sr {sr} {averaging:?} {policy:6} stage {s}: {:?} first block {:.3} s \
                         settle {:.3} s fill {:.3} s eff_avg {:.2} capped {}",
                        d.averaging,
                        d.first_block_s,
                        d.settle_s,
                        d.fill_s,
                        d.steady_eff_avg,
                        d.capped
                    );
                }
            }
            assert_eq!(eq[0], fast[0], "full-rate stage untouched");
            assert!(eq.iter().all(|d| !d.capped));
            let deepest = fast.len() - 1;
            for s in 1..fast.len() {
                let (e, f) = (eq[s], fast[s]);
                assert_eq!(e.first_block_s, f.first_block_s);
                assert!(f.settle_s <= cap + 1e-9, "{sr} stage {s}: {}", f.settle_s);
                assert!(f.settle_s <= e.settle_s + 1e-12);
                if f.capped {
                    assert!(f.steady_eff_avg < e.steady_eff_avg);
                } else {
                    assert_eq!(e, f);
                }
            }
            // Equal confidence takes several seconds in the deepest stage; the cap must bite.
            assert!(eq[deepest].settle_s > cap && fast[deepest].capped);
            assert!(fast[deepest].steady_eff_avg < 0.6 * eq[deepest].steady_eff_avg);
        }
    }
}

/// Feeding exactly the layout's fill time fills the deepest stage, one sample less does
/// not, and no column is left settling; the span after the first block is within 1 s.
#[test]
fn fast_lf_deepest_stage_settles_within_cap() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        let mut m = engine_with(sr, Averaging::Fifo { blocks: 8 }, DepthPolicy::fast_lf(), 0);
        let deepest = m.layout().stages.len() - 1;
        let spec = m.layout().stages[deepest].clone();
        let depth = m.stage_depths()[deepest];
        let StageAveraging::Fifo { blocks } = depth.averaging else {
            panic!("fifo");
        };
        assert!(depth.settle_s <= 1.0, "{sr}: {}", depth.settle_s);
        let fill = spec.pairs_for_blocks(blocks) as usize;
        assert!((fill as f64 / sr - depth.fill_s).abs() < 1e-9);
        // The fill time of every other stage is shorter, so the frame is complete then.
        assert!(
            m.stage_depths()
                .iter()
                .all(|d| d.fill_s <= depth.fill_s + 1e-12)
        );
        let x = Noise(71).vec(fill, 0.2);
        let y = Noise(72).vec(fill, 0.2);
        m.push(0, &x[..fill - 1], &y[..fill - 1], SampleGate::Accept)
            .expect("push");
        let held = |m: &Mtw| m.stage_spectra(deepest).map_or(0, |s| s.blocks);
        assert_eq!(held(&m), blocks as u64 - 1, "{sr}");
        m.push(
            fill as u64 - 1,
            &x[fill - 1..],
            &y[fill - 1..],
            SampleGate::Accept,
        )
        .expect("push");
        assert_eq!(held(&m), blocks as u64, "{sr}");
        let f = m.frame();
        assert!(f.columns.iter().all(|c| c.validity != Validity::Settling));
        // Reported effective averages are the capped stage's, so the higher floor shows.
        let full = m.steady_eff_avg(0);
        let low = valid(&f)
            .filter(|&i| f.freq_hz[i] < m.layout().crossovers[deepest - 1].lo_hz)
            .map(|i| f.eff_avg[i])
            .fold(f64::INFINITY, f64::min);
        assert!(low < full, "{sr}: deep eff_avg {low} vs full-rate {full}");
        println!(
            "sr {sr}: deepest {blocks} blocks, fill {:.3} s (first block {:.3} s + span {:.3} s)",
            depth.fill_s, depth.first_block_s, depth.settle_s
        );
    }
}

/// The floor holds when the cap is tighter than the floor allows, and a large cap or a
/// shallow setting never deepens a stage beyond equal confidence.
#[test]
fn fast_lf_floor_and_never_deeper() {
    let sr = 48_000.0;
    let tight = DepthPolicy::FastLf { max_settle_s: 0.01 };
    let m = engine_with(sr, Averaging::Fifo { blocks: 8 }, tight, 0);
    for d in &m.stage_depths()[1..] {
        assert_eq!(
            d.averaging,
            StageAveraging::Fifo {
                blocks: MIN_FAST_LF_BLOCKS
            }
        );
    }
    let floor_neff: Vec<f64> = m.stage_depths().iter().map(|d| d.steady_eff_avg).collect();
    let e = engine_with(
        sr,
        Averaging::Exponential {
            time_constant_s: 0.15,
        },
        tight,
        0,
    );
    for (s, d) in e.stage_depths().iter().enumerate().skip(1) {
        assert!(
            (d.steady_eff_avg / floor_neff[s] - 1.0).abs() < 1e-6,
            "stage {s}"
        );
    }
    for averaging in [
        Averaging::Fifo { blocks: 1 },
        Averaging::Fifo { blocks: 8 },
        Averaging::Exponential {
            time_constant_s: 0.15,
        },
    ] {
        let eq = engine(sr, averaging, 0).stage_averaging();
        let wide = DepthPolicy::FastLf {
            max_settle_s: 1000.0,
        };
        assert_eq!(engine_with(sr, averaging, wide, 0).stage_averaging(), eq);
        let tight_avg = engine_with(sr, averaging, tight, 0).stage_depths();
        for (d, e) in tight_avg.iter().zip(&eq) {
            match (d.averaging, *e) {
                (StageAveraging::Fifo { blocks: a }, StageAveraging::Fifo { blocks: b }) => {
                    assert!(a <= b)
                }
                (
                    StageAveraging::Exponential { alpha: a },
                    StageAveraging::Exponential { alpha: b },
                ) => assert!(a >= b),
                _ => panic!("kind changed"),
            }
        }
    }
}

#[test]
fn fast_lf_parameter_validation() {
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let p = DepthPolicy::FastLf { max_settle_s: bad };
        let r = Mtw::new(MtwConfig {
            sample_rate_hz: 48_000.0,
            ladder: Ladder::Standard,
            averaging: Averaging::Fifo { blocks: 8 },
            depth: p,
            grid: grid48(),
            delay_samples: 0.0,
        });
        assert!(
            matches!(r, Err(ConfigError::InvalidDepthPolicy(_))),
            "{bad}"
        );
        let mut m = engine(48_000.0, Averaging::Fifo { blocks: 8 }, 0);
        let before = m.stage_averaging();
        assert!(matches!(
            m.set_depth_policy(p),
            Err(ConfigError::InvalidDepthPolicy(_))
        ));
        assert_eq!(m.config().depth, DepthPolicy::EqualConfidence);
        assert_eq!(m.stage_averaging(), before);
    }
    // Averaging is still validated under fast LF.
    let mut m = engine_with(
        48_000.0,
        Averaging::Fifo { blocks: 8 },
        DepthPolicy::fast_lf(),
        0,
    );
    assert!(m.set_averaging(Averaging::Fifo { blocks: 0 }).is_err());
    assert_eq!(m.config().depth, DepthPolicy::fast_lf());
    assert_eq!(DepthPolicy::default(), DepthPolicy::EqualConfidence);
}

/// Switching policy behaves like changing averaging: the averages clear, framing continues,
/// and the result is bit-identical to an engine that had the new policy all along and
/// reset its averages at the same point.
#[test]
fn switching_depth_policy_resets_averages() {
    let sr = 48_000.0;
    let n = 150_000;
    let x = Noise(81).vec(n, 0.2);
    let y = Biquad::peaking(sr, 120.0, 1.0, 4.0).filter(&x);
    let avg = Averaging::Fifo { blocks: 4 };
    let mut a = engine(sr, avg, 0);
    let mut b = engine_with(sr, avg, DepthPolicy::fast_lf(), 0);
    a.push(0, &x[..n / 2], &y[..n / 2], SampleGate::Accept)
        .expect("push");
    b.push(0, &x[..n / 2], &y[..n / 2], SampleGate::Accept)
        .expect("push");
    a.reset_averages();
    b.set_depth_policy(DepthPolicy::EqualConfidence)
        .expect("policy");
    assert_eq!(a.stage_averaging(), b.stage_averaging());
    assert!(b.frame().columns.iter().all(|c| matches!(
        c.validity,
        Validity::Settling | Validity::Thinned | Validity::OutOfBand
    )));
    a.push(n as u64 / 2, &x[n / 2..], &y[n / 2..], SampleGate::Accept)
        .expect("push");
    b.push(n as u64 / 2, &x[n / 2..], &y[n / 2..], SampleGate::Accept)
        .expect("push");
    assert!(same_frame(&a.frame(), &b.frame()));
}
