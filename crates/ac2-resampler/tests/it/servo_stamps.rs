//! The servo's handling of the stamps themselves: outliers, spacing, gaps, coasting,
//! restarts, its settings, and blocks that do not follow each other.

use ac2_resampler::Step;
use ac2_resampler::servo::{
    AudioUse, BadConfig, MediaTime, Mode, NotReady, Servo, ServoConfig, StampUse,
};

use crate::servo::{go, max_diff};
use crate::servo_sim::{FS, Rng, Sim, mean_sd};

/// Stamps with a heavy tail: one in a hundred errs by a further 1 ms sd, from the first
/// stamp on. Acquisition sets those aside, tracking gates them: the phase stays within 4 µs
/// of the same run without the tail (2.7 µs at a stamp per block, 3.9 at a stamp a second),
/// and the step stays smooth.
#[test]
fn heavy_tailed_stamps_barely_move_the_phase() {
    for every in [1, 10] {
        let sim = |tail: bool| {
            let mut sim = Sim::stamps_only(2, 1800.0);
            sim.stamp_every = every;
            sim.stamp_tail = tail.then_some((0.01, 1e-3));
            sim
        };
        let (clean, tailed) = (go(&sim(false)), go(&sim(true)));
        let moved = max_diff(&clean, &tailed);
        let gated = tailed
            .recs
            .iter()
            .filter(|r| r.stamp == Some(StampUse::Outlier))
            .count();
        let step = tailed.step_change_per_s(300.0);
        println!(
            "tail, stamp every {every}: moved {:.2} µs, step ≤ {step:.4} ppm/s (clean {:.4}), \
             {gated} gated",
            moved * 1e6,
            clean.step_change_per_s(300.0)
        );
        assert!(
            moved <= 6e-6,
            "stamp every {every}: moved {:.2} µs",
            moved * 1e6
        );
        assert!(
            step <= 0.05,
            "stamp every {every}: step change {step:.4} ppm/s"
        );
        assert!(gated > 0);
    }
}

/// One stamp 1 ms late at one stamp a second: it moves the phase by 0.14 µs and the step by
/// 0.013 ppm (ungated, on the real stamps: 16 µs and 2.2 ppm).
#[test]
fn one_stamp_1_ms_late_is_gated() {
    let sim = |spike: bool| {
        let mut sim = Sim::stamps_only(3, 1500.0);
        sim.stamp_every = 10;
        sim.stamp_spike = spike.then_some((1000.0, 1e-3));
        sim
    };
    let (clean, spiked) = (go(&sim(false)), go(&sim(true)));
    let moved = max_diff(&clean, &spiked);
    let step = clean
        .recs
        .iter()
        .zip(&spiked.recs)
        .filter_map(|(a, b)| Some((a.step?.get() - b.step?.get()).abs() * 1e6))
        .fold(0.0, f64::max);
    let use_ = spiked
        .recs
        .iter()
        .find(|r| r.t >= 1000.0 && r.stamp.is_some());
    assert_eq!(use_.unwrap().stamp, Some(StampUse::Outlier));
    println!(
        "1 ms spike: phase moved {:.3} µs, step {step:.4} ppm",
        moved * 1e6
    );
    assert!(moved <= 0.25e-6, "phase moved {:.3} µs", moved * 1e6);
    assert!(step <= 0.02, "step moved {step:.4} ppm");
}

/// Stamps from 32- and 64-frame blocks at 96 kHz weigh by their spacing, not by a floor:
/// after the same seconds of noise-free stamps the stated phase sd is the same whatever the
/// block length.
#[test]
fn stamps_weigh_by_their_spacing_down_to_32_frames() {
    let cfg = ServoConfig {
        wander_sd_s: 0.2e-6,
        ..ServoConfig::default()
    };
    let stated = |frames: u64| {
        let mut s = Servo::new(cfg, FS).unwrap();
        let fs = f64::from(FS);
        let start = 1u64 << 36;
        let blocks = 20 * u64::from(FS) / frames;
        for b in 0..=blocks {
            let index = start + b * frames;
            let ns = 1_000_000_000_000 + ((b * frames) as f64 / fs / (1.0 + 7e-6) * 1e9) as u64;
            s.observe_stamp(index, MediaTime::from_nanos(ns));
        }
        let now = MediaTime::from_nanos(1_000_000_000_000 + 20_000_000_000);
        s.status(now).phase_sd_s.unwrap()
    };
    let (a, b, c) = (stated(32), stated(64), stated(9600));
    println!(
        "stated phase sd after 20 s: 32 frames {:.4} µs, 64 {:.4}, 9600 {:.4}",
        a * 1e6,
        b * 1e6,
        c * 1e6
    );
    for (frames, v) in [(32, a), (64, b)] {
        assert!(
            (v / c - 1.0).abs() <= 0.02,
            "{frames} frames: {:.4} µs against {:.4} µs at 9600",
            v * 1e6,
            c * 1e6
        );
    }
}

/// A stamp after 30 s without stamps weighs as one block's, not as the 30 s it follows:
/// with 0.1 s blocks it moves the phase less than the same stamp after 1 s means does.
#[test]
fn a_stamp_after_a_gap_weighs_as_its_own_block() {
    let cfg = ServoConfig {
        stamp_gate: 1e3,
        ..ServoConfig::default()
    };
    let fs = f64::from(FS);
    let moved = |spacing: f64| {
        let line = |s: &mut Servo, t: f64, err: f64| {
            let index = (1u64 << 34) + (t * fs).round() as u64;
            let ns = 1_000_000_000_000 + ((t / (1.0 + 7e-6) + err) * 1e9).round() as u64;
            s.observe_stamp(index, MediaTime::from_nanos(ns))
        };
        let run = |err: f64| {
            let mut s = Servo::new(cfg, FS).unwrap();
            let n = (600.0 / spacing) as u64;
            for i in 0..=n {
                line(&mut s, i as f64 * spacing, 0.0);
            }
            assert_eq!(line(&mut s, 630.0, err), StampUse::Used);
            s.block(630 * u64::from(FS), 64).unwrap().0
        };
        run(20e-6).diff(run(0.0)) / fs
    };
    let (block, mean) = (moved(0.1), moved(1.0));
    println!(
        "a 20 µs stamp error after a 30 s gap moves the phase {:.3} µs (0.1 s blocks), {:.3} \
         µs (1 s means)",
        block * 1e6,
        mean * 1e6
    );
    assert!(block < 0.6 * mean, "{block:e} against {mean:e}");
}

/// Stamps whose index is not after the newest one's (out of order, repeated) are refused
/// and leave the state as it was.
#[test]
fn out_of_order_and_repeated_stamps_are_refused() {
    let mut s = Servo::new(ServoConfig::default(), FS).unwrap();
    let stamp = |i: u64| {
        let ns = 1_000_000_000_000 + (i as f64 * 1e9 / f64::from(FS)) as u64;
        (i + (1 << 40), MediaTime::from_nanos(ns))
    };
    for b in 0..=100u64 {
        let (i, at) = stamp(b * 9600);
        s.observe_stamp(i, at);
    }
    let before = s.clone();
    let (i, at) = stamp(100 * 9600);
    assert_eq!(s.observe_stamp(i, at), StampUse::OutOfOrder);
    let (i, at) = stamp(99 * 9600 + 5);
    assert_eq!(s.observe_stamp(i, at), StampUse::OutOfOrder);
    let k = 1_000 * u64::from(FS) + 100 * 9600;
    assert_eq!(s.block(k, 64), before.clone().block(k, 64));
    let (i, at) = stamp(101 * 9600);
    assert_eq!(s.observe_stamp(i, at), StampUse::Used);
}

/// Stamps stop for a minute while blocks go on: the status says coasting after
/// `coast_after_s`, the stated phase sd grows, the output continues on the held rate (it
/// moved 0.5–5.0 µs against the truth over the minute, five seeds), and tracking resumes
/// when stamps return, within 8.3 µs of the run that never lost them.
#[test]
fn a_minute_without_stamps_coasts() {
    for seed in crate::servo::SEEDS {
        let mut sim = Sim::stamps_only(seed, 1500.0);
        sim.stamp_gaps = vec![(1200.0, 60.0)];
        let coast = go(&sim);
        let plain = go(&Sim::stamps_only(seed, 1500.0));
        let at = |t: f64| coast.recs.iter().find(|r| r.t >= t).unwrap();
        for (t, mode) in [
            (1199.0, Mode::Tracking),
            (1202.0, Mode::Tracking),
            (1204.0, Mode::Coasting),
            (1259.0, Mode::Coasting),
            (1261.0, Mode::Tracking),
        ] {
            assert_eq!(at(t).status.mode, mode, "seed {seed} at {t} s");
        }
        let (sd0, sd1) = (
            at(1199.0).status.phase_sd_s.unwrap(),
            at(1259.0).status.phase_sd_s.unwrap(),
        );
        assert!(sd1 > sd0);
        // Blocks never stopped, and each continued the last.
        let handed: Vec<_> = coast.recs.iter().filter(|r| r.t >= 300.0).collect();
        for w in handed.windows(2) {
            let (a, b) = (w[0], w[1]);
            let next = a
                .pos
                .unwrap()
                .advanced(a.step.unwrap(), crate::servo_sim::BLOCK as u64);
            assert_eq!(next, b.pos, "seed {seed} at {} s", b.t);
        }
        let drift = coast
            .recs
            .iter()
            .filter(|r| (1200.0..1260.0).contains(&r.t))
            .map(|r| r.err_s.unwrap() - at(1200.0).err_s.unwrap())
            .fold(0.0, |m: f64, e| m.max(e.abs()));
        let against = max_diff(&coast, &plain);
        println!(
            "seed {seed}, coast 60 s: stated sd {:.2} → {:.2} µs, drift during {:.2} µs, worst \
             against the run with stamps {:.2} µs",
            sd0 * 1e6,
            sd1 * 1e6,
            drift * 1e6,
            against * 1e6
        );
        assert!(
            drift <= 7.5e-6,
            "seed {seed}: drifted {:.2} µs",
            drift * 1e6
        );
        assert!(against <= 12.5e-6, "seed {seed}: {:.2} µs", against * 1e6);
    }
}

/// The input stops for 2 s and the device numbers its samples afresh: `reset_phase` keeps
/// the rate and places the phase again from a second of stamps.
#[test]
fn reset_phase_reacquires_in_a_second_and_keeps_the_rate() {
    let mut sim = Sim::stamps_only(1, 2400.0);
    sim.gaps = vec![(1800.0, 2.0)];
    let r = go(&sim);
    let back = r.first_block_after(1800.0).unwrap() - 1802.0;
    assert!(
        back <= 1.5,
        "re-acquired {back:.1} s after the input resumed"
    );
    let at = r.recs.iter().find(|x| x.t >= 1802.0 + back).unwrap();
    let rate_err = at.status.rate_ppm.unwrap() - at.true_ppm;
    assert!(rate_err.abs() <= 0.3, "rate {rate_err:.3} ppm off");
    let worst = r
        .recs
        .iter()
        .filter(|x| x.t >= 1802.0)
        .filter_map(|x| x.err_s)
        .fold(0.0, |m: f64, e| m.max(e.abs()));
    assert!(worst <= 75e-6, "worst phase error {:.1} µs", worst * 1e6);
    println!(
        "gap: back {back:.1} s after the input, rate {rate_err:.3} ppm off, worst {:.1} µs",
        worst * 1e6
    );
}

/// After a four-hour outage the crystal has walked: the kept rate's variance has grown with
/// the rate walk over the outage, so the new rate is found within a minute instead of being
/// held against the stamps.
#[test]
fn reset_phase_ages_the_rate_over_the_outage() {
    let cfg = ServoConfig {
        rate_walk_ppm_per_h: 1.0,
        ..ServoConfig::default()
    };
    let fs = f64::from(FS);
    let mut s = Servo::new(cfg, FS).unwrap();
    let mut rng = Rng::new(9);
    let mut feed = |s: &mut Servo, t0: f64, secs: f64, ppm: f64, base: u64| {
        let n = (secs / 0.1) as u64;
        for b in 0..=n {
            let t = b as f64 * 0.1;
            let ns =
                ((t0 + t / (1.0 + ppm * 1e-6) + 5e-6 / 0.1f64.sqrt() * rng.normal()) * 1e9) as u64;
            s.observe_stamp(base + (t * fs) as u64, MediaTime::from_nanos(ns));
        }
    };
    feed(&mut s, 1000.0, 1800.0, 7.0, 1 << 34);
    s.reset_phase();
    let resume = 1000.0 + 1800.0 + 4.0 * 3600.0;
    feed(&mut s, resume, 60.0, 9.0, 1 << 20);
    let now = MediaTime::from_nanos(((resume + 60.0) * 1e9) as u64);
    let rate = s.status(now).rate_ppm.unwrap();
    println!("after 4 h out and 60 s of stamps: rate {rate:.3} ppm (truth 9)");
    assert!((rate - 9.0).abs() <= 0.3, "rate {rate:.3} ppm");
}

/// A device at 2000 ppm (a wrong nominal rate): the step is clamped to the resampler's
/// range and the status says so.
#[test]
fn a_wrong_nominal_rate_saturates() {
    let mut sim = Sim::stamps_only(1, 120.0);
    sim.ppm = 2000.0;
    let r = go(&sim);
    let last = r.recs.last().unwrap();
    assert_eq!(last.status.mode, Mode::Saturated);
    let ppm = last.status.rate_ppm.unwrap();
    assert!((ppm - 2000.0).abs() < 1.0, "rate {ppm} ppm");
    let limit = Step::from_deviation(Step::MAX_DEVIATION).unwrap();
    assert_eq!(last.step, Some(limit));
}

#[test]
fn nothing_is_handed_out_before_acquiring() {
    let mut s = Servo::new(ServoConfig::default(), FS).unwrap();
    assert_eq!(s.block(0, 512), Err(NotReady::Acquiring));
    let at = MediaTime::from_nanos(1_000_000_000);
    assert_eq!(s.observe_audio(1e-3, 1e-6, at, false), AudioUse::Unplaced);
    assert_eq!(s.observe_stamp(96_000, at), StampUse::Started);
    assert_eq!(s.block(96_000, 512), Err(NotReady::Acquiring));
    assert_eq!(s.status(at).mode, Mode::Acquiring);
}

#[test]
fn settings_it_cannot_run_are_refused() {
    let new = |cfg: ServoConfig, rate: u32| Servo::new(cfg, rate).map(|_| ()).unwrap_err();
    let d = ServoConfig::default();
    assert_eq!(new(d, 0), BadConfig::NominalRate);
    assert_eq!(new(d, Servo::MAX_NOMINAL_RATE + 1), BadConfig::NominalRate);
    assert!(Servo::new(d, Servo::MAX_NOMINAL_RATE).is_ok());
    let nan = ServoConfig {
        wander_tau_s: f64::NAN,
        ..d
    };
    assert_eq!(new(nan, FS), BadConfig::WanderTau);
    // The steady phase sd reaches the default 100 µs at a wander of about 237 µs (it grows
    // more slowly than the wander: 14.1 µs at 20, 56 at 112).
    let loud = ServoConfig {
        wander_sd_s: 250e-6,
        ..d
    };
    assert_eq!(new(loud, FS), BadConfig::AcquirePhaseUnreachable);
    assert!(
        Servo::new(
            ServoConfig {
                wander_sd_s: 220e-6,
                ..d
            },
            FS
        )
        .is_ok()
    );
    // The rate settles to 0.037 ppm sd.
    let tight = ServoConfig {
        acquire_rate_ppm: 0.03,
        ..d
    };
    assert_eq!(new(tight, FS), BadConfig::AcquireRateUnreachable);
}

/// Blocks that skip ahead start on the estimate, forget the earlier output (audio for it
/// is unplaced), and the blocks after them continue exactly.
#[test]
fn non_contiguous_blocks_start_on_the_estimate() {
    let sim = Sim::stamps_only(1, 30.0);
    let mut s = Servo::new(sim.cfg, FS).unwrap();
    crate::servo_sim::run(&sim, &mut s);
    let k0 = crate::servo_sim::START_INDEX + 300 * crate::servo_sim::BLOCK as u64;
    // A servo that has handed nothing places any block on its estimate.
    let virgin = s.clone();
    let (p0, step0) = s.block(k0, 512).unwrap();
    let (p1, _) = s.block(k0 + 512, 512).unwrap();
    assert_eq!(Some(p1), p0.advanced(step0, 512));
    // Skip 10 s ahead: placed on the estimate, with no correction carried over.
    let k2 = k0 + 10 * u64::from(FS);
    let (p2, step2) = s.block(k2, 512).unwrap();
    assert_eq!(Ok((p2, step2)), virgin.clone().block(k2, 512));
    let at =
        |k: u64| MediaTime::from_nanos((u128::from(k) * 1_000_000_000 / u128::from(FS)) as u64);
    assert_eq!(
        s.observe_audio(1e-3, 1e-6, at(k0 + 100), false),
        AudioUse::Unplaced
    );
    assert_eq!(
        s.observe_audio(1e-3, 1e-6, at(k2 + 100), false),
        AudioUse::BiasReset
    );
    let (p3, _) = s.block(k2 + 512, 512).unwrap();
    assert_eq!(Some(p3), p2.advanced(step2, 512));
}

/// The measured stamp scatter shows a host whose stamps wander four times more than the
/// config says, while the model's phase sd stays where the config puts it; and it does not
/// read below the phase error it stands for.
#[test]
fn stamp_scatter_shows_worse_stamps() {
    for wander in [15.5e-6, 60e-6] {
        for seed in crate::servo::SEEDS {
            let mut sim = Sim::stamps_only(seed, 3600.0);
            sim.wander_sd_s = wander;
            let r = go(&sim);
            let last = r.recs.last().unwrap().status;
            let (_, actual) = mean_sd(&r.errors(600.0));
            let (scatter, stated) = (last.stamp_scatter_s.unwrap(), last.phase_sd_s.unwrap());
            println!(
                "wander {:.1} µs seed {seed}: scatter {:.2} µs, stated {:.2} µs, actual error \
                 sd {:.2} µs",
                wander * 1e6,
                scatter * 1e6,
                stated * 1e6,
                actual * 1e6
            );
            if wander > 20e-6 {
                assert!(
                    scatter >= 1.5 * stated,
                    "scatter {scatter:e}, stated {stated:e}"
                );
            } else {
                assert!(
                    scatter <= 1.5 * stated,
                    "scatter {scatter:e}, stated {stated:e}"
                );
            }
            assert!(
                scatter >= 0.8 * actual,
                "scatter {scatter:e}, actual {actual:e}"
            );
        }
    }
}
