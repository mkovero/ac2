//! The clock servo against the simulated rig (`servo_sim`): the figures measured on the rigs
//! as the noise, the truth known exactly. Each assertion message carries the measured value;
//! the bounds sit about 1.5 times above the worst of the seeds.

use ac2_resampler::servo::{AudioUse, Mode};

use crate::servo_sim::{Run, Sim, mean_sd};

pub fn go(sim: &Sim) -> Run {
    let mut servo = ac2_resampler::servo::Servo::new(sim.cfg, crate::servo_sim::FS).unwrap();
    crate::servo_sim::run(sim, &mut servo).0
}

pub const SEEDS: [u64; 5] = [1, 2, 3, 4, 5];
/// The stamp wander on the two hosts measured: hardware PTP, software PTP.
pub const WANDERS: [f64; 2] = [15.5e-6, 27.8e-6];

fn stamps_only(seed: u64, wander: f64) -> Sim {
    let mut sim = Sim::stamps_only(seed, 3600.0);
    sim.wander_sd_s = wander;
    sim
}

fn with_audio(seed: u64) -> Sim {
    let mut sim = Sim::stamps_only(seed, 3600.0);
    sim.path = Box::new(|_| Some(1e-3));
    sim
}

/// Largest |a − b| over blocks both handed, seconds.
pub fn max_diff(a: &Run, b: &Run) -> f64 {
    a.recs
        .iter()
        .zip(&b.recs)
        .filter_map(|(x, y)| Some((x.err_s? - y.err_s?).abs()))
        .fold(0.0, f64::max)
}

/// The CM4-like device (+7 ppm, 0.25 ppm/h) for an hour on stamps alone, five seeds at each
/// host's wander. Worst over the seeds (µs, ppm, ppm/s): at 15.5 µs, error sd 13.0, peak to
/// peak 52, mean 17.8, rate sd 0.041, step change 0.025; at 27.8 µs, 18.1, 76, 20.0, 0.045,
/// 0.034. Acquired after 11.3 s.
#[test]
fn stamps_only_over_seeds_and_both_wanders() {
    // Per wander: error sd, peak-to-peak, |mean|, rate sd, step change per second.
    let bounds = [
        (20e-6, 78e-6, 27e-6, 0.062, 0.037),
        (27e-6, 114e-6, 30e-6, 0.068, 0.05),
    ];
    for (&wander, &(b_sd, b_pp, b_mean, b_rate, b_step)) in WANDERS.iter().zip(&bounds) {
        for seed in SEEDS {
            let r = go(&stamps_only(seed, wander));
            let acquired = r.first_block_after(0.0).unwrap();
            let (mean, sd) = mean_sd(&r.errors(300.0));
            let pp = r.peak_to_peak(300.0);
            let (_, rate_sd) = mean_sd(&r.rate_errors(300.0));
            let step = r.step_change_per_s(300.0);
            let last = r.recs.last().unwrap().status;
            println!(
                "wander {:.1} µs seed {seed}: acquired {acquired:.1} s, error {:+.2} ± {:.2} µs, \
                 p-p {:.1} µs, rate sd {rate_sd:.3} ppm, step ≤ {step:.4} ppm/s",
                wander * 1e6,
                mean * 1e6,
                sd * 1e6,
                pp * 1e6,
            );
            let what = format!("wander {:.1} µs seed {seed}", wander * 1e6);
            assert!(acquired <= 17.0, "{what}: acquired after {acquired} s");
            assert!(sd <= b_sd, "{what}: error sd {:.2} µs", sd * 1e6);
            assert!(pp <= b_pp, "{what}: peak-to-peak {:.1} µs", pp * 1e6);
            assert!(mean.abs() <= b_mean, "{what}: mean {:.2} µs", mean * 1e6);
            assert!(rate_sd <= b_rate, "{what}: rate sd {rate_sd:.3} ppm");
            assert!(step <= b_step, "{what}: step change {step:.4} ppm/s");
            assert_eq!(last.mode, Mode::Tracking);
            assert!(!last.audio && last.audio_sd_s.is_none());
        }
    }
}

/// The stamps-only time constant: every stamp suddenly reads 100 µs late, and the output
/// follows it to 63 % in about three minutes (184–186 s), at a stamp per block or a stamp a
/// second: the stamp gate, which takes the step's first stamps as outliers, delays that by
/// under two seconds.
#[test]
fn stamps_only_follow_a_step_in_about_three_minutes() {
    let mut t63s = Vec::new();
    for (every, gate) in [(1, 4.0), (10, 4.0), (1, 1e3), (10, 1e3)] {
        let sim = |step: bool| {
            let mut sim = Sim::stamps_only(1, 2000.0);
            sim.stamp_every = every;
            sim.stamp_step = step.then_some((1200.0, 100e-6));
            sim.cfg.stamp_gate = gate;
            sim
        };
        let (a, b) = (go(&sim(false)), go(&sim(true)));
        let t63 = a
            .recs
            .iter()
            .zip(&b.recs)
            .find(|(x, y)| x.t >= 1200.0 && (x.err_s.unwrap() - y.err_s.unwrap()).abs() >= 63e-6)
            .map(|(x, _)| x.t - 1200.0)
            .unwrap();
        let outliers = b
            .recs
            .iter()
            .filter(|r| r.stamp == Some(ac2_resampler::servo::StampUse::Outlier))
            .count();
        println!(
            "stamp every {every} blocks, gate {gate}: 63 % after {t63:.1} s, {outliers} stamps \
             gated"
        );
        assert!((150.0..=220.0).contains(&t63), "63 % after {t63:.1} s");
        assert!(gate > 100.0 || outliers > 0);
        t63s.push(t63);
    }
    for (gated, free) in [(t63s[0], t63s[2]), (t63s[1], t63s[3])] {
        assert!(
            (gated - free).abs() <= 2.0,
            "gated {gated:.1} s, ungated {free:.1} s"
        );
    }
}

/// A transfer function runs the whole hour over a path jittering 5 µs per observation (the
/// default's indoor air): the phase faster than three minutes holds within 0.89–1.14 µs sd
/// at 15.5 µs of stamp wander and 1.57–1.99 µs at 27.8, under the stated audio sd, while the
/// long term stays the stamps'.
#[test]
fn audio_holds_the_short_term_over_seeds() {
    for wander in WANDERS {
        let label = format!("wander {:.1} µs", wander * 1e6);
        for seed in SEEDS {
            let mut sim = with_audio(seed);
            sim.wander_sd_s = wander;
            sim.path_jitter_s = 5e-6;
            let r = go(&sim);
            let (_, hp) = mean_sd(&r.high_passed(300.0, 180.0));
            let (mean, sd) = mean_sd(&r.errors(300.0));
            let last = r.recs.last().unwrap().status;
            let stated = last.audio_sd_s.unwrap();
            let step = r.step_change_per_s(300.0);
            println!(
                "{label} seed {seed}: short-term sd {:.3} µs, error {:+.2} ± {:.2} µs, stated \
                 audio sd {:.3} µs, step ≤ {step:.4} ppm/s",
                hp * 1e6,
                mean * 1e6,
                sd * 1e6,
                stated * 1e6,
            );
            let bound = if wander < 20e-6 { 1.7e-6 } else { 3e-6 };
            assert!(
                hp <= bound,
                "{label} seed {seed}: short-term {:.3} µs",
                hp * 1e6
            );
            assert!(
                stated >= hp,
                "{label} seed {seed}: stated {:.3} µs",
                stated * 1e6
            );
            assert!(
                mean.abs() <= 27e-6,
                "{label} seed {seed}: mean {:.2} µs",
                mean * 1e6
            );
            assert!(
                step <= 0.022,
                "{label} seed {seed}: step change {step:.4} ppm/s"
            );
            assert!(last.audio && last.audio_refused == 0);
        }
    }
}

/// The path grows by 0.16 ms (1 °C over 30 m) over 1, 5 or 20 minutes. The difference from
/// the same run with a still path is what leaks into the phase; the filter is linear, so it
/// does not depend on the noise.
#[test]
fn path_drift_leaks_a_stated_part_into_the_phase() {
    let still = go(&with_audio(1));
    let mut leaks = Vec::new();
    for (ramp, bound) in [(60.0, 70e-6), (300.0, 64e-6), (1200.0, 37e-6)] {
        let mut sim = with_audio(1);
        sim.path = Box::new(move |t| Some(1e-3 + 0.16e-3 * ((t - 1200.0) / ramp).clamp(0.0, 1.0)));
        let leak = max_diff(&still, &go(&sim));
        assert!(
            leak <= bound,
            "0.16 ms over {ramp} s: {:.1} µs into the phase",
            leak * 1e6
        );
        leaks.push((ramp, leak * 1e6));
    }
    println!("path change of 160 µs over (s, peak µs into θ): {leaks:.1?}");
}

/// Audio stops for five minutes and comes back with the path 1 ms longer (the tracker
/// re-anchored on another arrival): the gate sets the bias afresh, exactly as a restart flag
/// would, and the phase does not move. Against audio that comes back unchanged it loses only
/// what the old bias knew, which is less than the stamps-only error.
#[test]
fn audio_dropout_and_a_1_ms_jump_leave_the_phase_alone() {
    let dropout = |jump: f64, flagged: bool| {
        let mut sim = with_audio(1);
        sim.path = Box::new(move |t| match t {
            t if t < 1200.0 => Some(1e-3),
            t if t < 1500.0 => None,
            _ => Some(1e-3 + jump),
        });
        sim.restart_at = flagged.then_some(1500.0);
        go(&sim)
    };
    let jumped = dropout(1e-3, false);
    let reset_at = jumped
        .recs
        .iter()
        .find(|r| r.t >= 1500.0 && r.audio.is_some())
        .unwrap();
    assert_eq!(reset_at.audio, Some(AudioUse::BiasReset));
    assert_eq!(jumped.recs.last().unwrap().status.audio_refused, 1);
    let as_restart = max_diff(&dropout(0.0, true), &jumped);
    assert!(
        as_restart <= 1e-9,
        "gated jump vs flagged restart: {:.4} µs",
        as_restart * 1e6
    );
    let lost = max_diff(&dropout(0.0, false), &jumped);
    assert!(lost <= 8e-6, "against unchanged audio {:.3} µs", lost * 1e6);
    // The short-term figure grows honestly while audio is away.
    let at = |t: f64| jumped.recs.iter().find(|r| r.t >= t).unwrap().status;
    let (before, during) = (at(1199.0), at(1499.0));
    assert!(before.audio && !during.audio);
    let (b, d) = (before.audio_sd_s.unwrap(), during.audio_sd_s.unwrap());
    assert!(d > 5.0 * b, "audio sd {:.3} → {:.3} µs", b * 1e6, d * 1e6);
    println!(
        "dropout: gated jump = restart within {:.4} µs, {:.3} µs from unchanged audio; audio sd \
         {:.3} → {:.2} µs over 5 min",
        as_restart * 1e6,
        lost * 1e6,
        b * 1e6,
        d * 1e6,
    );
}

/// The air moves: the path jitters by 30 µs per observation (outdoors, wind). Configured for
/// it, audio contributes throughout and nothing is refused; on the default's 5 µs, 45 % of
/// the observations are refused, and counted, while the rest still contribute. Either way
/// the stated short-term figure covers what the phase does faster than three minutes
/// (measured 0.91–1.27 µs configured, 1.27–1.82 µs on the default).
#[test]
fn a_jittering_path_keeps_contributing_and_says_how_well() {
    for (cfg_jitter, label) in [(30e-6, "configured 30 µs"), (5e-6, "default 5 µs")] {
        for seed in SEEDS {
            let mut sim = with_audio(seed);
            sim.path_jitter_s = 30e-6;
            sim.cfg.path_jitter_s = cfg_jitter;
            let r = go(&sim);
            let (_, hp) = mean_sd(&r.high_passed(300.0, 180.0));
            let late = || r.recs.iter().filter(|x| x.t >= 300.0);
            let contributing =
                late().filter(|x| x.status.audio).count() as f64 / late().count() as f64;
            let stated = late()
                .filter_map(|x| x.status.audio_sd_s)
                .fold(f64::INFINITY, f64::min);
            let refused = r.recs.last().unwrap().status.audio_refused;
            println!(
                "{label}, path jitter 30 µs, seed {seed}: short-term sd {:.2} µs, stated audio \
                 sd ≥ {:.2} µs, contributing {:.1} %, refused {refused}",
                hp * 1e6,
                stated * 1e6,
                contributing * 100.0,
            );
            let what = format!("{label} seed {seed}");
            assert!(stated >= hp, "{what}: stated {:.2} µs", stated * 1e6);
            if cfg_jitter > 10e-6 {
                assert!(hp <= 1.9e-6, "{what}: short-term {:.2} µs", hp * 1e6);
                assert!(contributing >= 0.99 && refused == 0, "{what}");
            } else {
                assert!(hp <= 2.7e-6, "{what}: short-term {:.2} µs", hp * 1e6);
                assert!(contributing >= 0.9, "{what}: {contributing}");
                assert!((1000..2500).contains(&refused), "{what}: refused {refused}");
            }
        }
    }
}

/// Every observation disagrees with the last by a millisecond (two arrivals the tracker
/// swaps between): each is refused, the status says audio is not contributing although
/// observations arrive, and counts them.
#[test]
fn refused_audio_is_not_reported_as_contributing() {
    let mut sim = with_audio(1);
    sim.secs = 600.0;
    sim.path = Box::new(|t| {
        Some(
            1e-3 + if (t as u64).is_multiple_of(2) {
                0.0
            } else {
                1e-3
            },
        )
    });
    let r = go(&sim);
    let seen = r.recs.iter().filter(|x| x.audio.is_some()).count() as u64;
    let last = r.recs.last().unwrap().status;
    assert!(!last.audio);
    assert!(r.recs.iter().all(|x| x.audio != Some(AudioUse::Used)));
    assert!(r.recs.iter().all(|x| !x.status.audio));
    // All but the first, which anchors the bias.
    assert_eq!(last.audio_refused, seen - 1);
}
