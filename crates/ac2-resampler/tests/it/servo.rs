//! The clock servo against the simulated rig (`servo_sim`): the figures measured on the rigs
//! as the noise, the truth known exactly. Each assertion message carries the measured value.

use ac2_resampler::servo::{AudioUse, MediaTime, Mode, NotReady, Servo, ServoConfig};

use crate::servo_sim::{FS, Run, Sim, mean_sd, run};

fn servo(cfg: ServoConfig) -> Servo {
    Servo::new(cfg, FS).unwrap()
}

fn go(sim: &Sim) -> Run {
    run(sim, &mut servo(sim.cfg)).0
}

fn with_audio(seed: u64) -> Sim {
    let mut sim = Sim::stamps_only(seed, 3600.0);
    sim.path = Box::new(|_| Some(1e-3));
    sim
}

/// Largest |a − b| over blocks both handed, seconds.
fn max_diff(a: &Run, b: &Run) -> f64 {
    a.recs
        .iter()
        .zip(&b.recs)
        .filter_map(|(x, y)| Some((x.err_s? - y.err_s?).abs()))
        .fold(0.0, f64::max)
}

/// The CM4-like device (+7 ppm, 0.25 ppm/h, stamp wander 15.5 µs sd over 100 s) for an
/// hour, stamps only.
#[test]
fn stamps_only_tracks_within_the_wander() {
    let r = go(&Sim::stamps_only(1, 3600.0));
    let acquired = r.first_block_after(0.0).unwrap();
    assert!(acquired <= 20.0, "acquired after {acquired} s");
    let (mean, sd) = mean_sd(&r.errors(300.0));
    assert!(sd <= 25e-6, "phase error sd {:.2} µs", sd * 1e6);
    // The mean of a Gauss–Markov wander over an hour has sd σ·√(2τ/T) = 3.7 µs.
    assert!(mean.abs() <= 11e-6, "phase error mean {:.2} µs", mean * 1e6);
    let (rate_mean, rate_sd) = mean_sd(&r.rate_errors(300.0));
    assert!(rate_sd <= 0.3, "rate error sd {rate_sd:.3} ppm");
    assert!(rate_mean.abs() <= 0.1, "rate error mean {rate_mean:.3} ppm");
    let smooth = r.step_change_per_s(300.0);
    assert!(smooth <= 0.1, "step changed {smooth:.4} ppm in a second");
    let last = r.recs.last().unwrap().status;
    assert_eq!(last.mode, Mode::Tracking);
    assert!(!last.audio && last.audio_sd_s.is_none());
    let stated = last.phase_sd_s.unwrap();
    assert!(
        (5e-6..=25e-6).contains(&stated),
        "stated phase sd {:.2} µs",
        stated * 1e6
    );
    println!(
        "acquired {acquired} s; error {:.2} ± {:.2} µs (stated {:.2}); rate {rate_mean:.3} ± \
         {rate_sd:.3} ppm; step change ≤ {smooth:.4} ppm/s",
        mean * 1e6,
        sd * 1e6,
        stated * 1e6
    );
}

/// The stamps-only time constant: every stamp suddenly reads 100 µs late, and the output
/// follows it to 63 % in a minute or two.
#[test]
fn stamps_only_follow_a_step_in_a_minute_or_two() {
    let base = Sim::stamps_only(1, 2000.0);
    let mut stepped = Sim::stamps_only(1, 2000.0);
    stepped.stamp_step = Some((1200.0, 100e-6));
    let (a, b) = (go(&base), go(&stepped));
    let t63 = a
        .recs
        .iter()
        .zip(&b.recs)
        .find(|(x, y)| x.t >= 1200.0 && (x.err_s.unwrap() - y.err_s.unwrap()).abs() >= 63e-6)
        .map(|(x, _)| x.t - 1200.0)
        .unwrap();
    assert!((40.0..=150.0).contains(&t63), "63 % after {t63:.1} s");
    println!("stamps-only step response: 63 % after {t63:.1} s");
}

/// A transfer function runs the whole hour: the output's phase follows the audio within
/// the audio's precision at short times and the stamps' line at long ones.
#[test]
fn audio_holds_the_short_term_within_2_us() {
    let r = go(&with_audio(1));
    let (_, hp) = mean_sd(&r.high_passed(300.0, 180.0));
    assert!(
        hp <= 2e-6,
        "phase error faster than 3 min: sd {:.3} µs",
        hp * 1e6
    );
    let (mean, sd) = mean_sd(&r.errors(300.0));
    assert!(mean.abs() <= 11e-6, "phase error mean {:.2} µs", mean * 1e6);
    let last = r.recs.last().unwrap().status;
    assert!(last.audio);
    let audio_sd = last.audio_sd_s.unwrap();
    assert!(audio_sd <= 1e-6, "stated audio sd {:.3} µs", audio_sd * 1e6);
    let smooth = r.step_change_per_s(300.0);
    assert!(smooth <= 0.1, "step changed {smooth:.4} ppm in a second");
    println!(
        "with audio: short-term sd {:.3} µs, error {:.2} ± {:.2} µs, stated audio sd {:.3} µs, \
         step change ≤ {smooth:.4} ppm/s",
        hp * 1e6,
        mean * 1e6,
        sd * 1e6,
        audio_sd * 1e6
    );
}

/// The path grows by 0.16 ms (1 °C over 30 m) over 1, 5 or 20 minutes. The difference from
/// the same run with a still path is what leaks into the phase; the filter is linear, so it
/// does not depend on the noise.
#[test]
fn path_drift_leaks_a_stated_part_into_the_phase() {
    let still = go(&with_audio(1));
    let mut leaks = Vec::new();
    for (ramp, bound) in [(60.0, 120e-6), (300.0, 95e-6), (1200.0, 40e-6)] {
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
    let as_restart = max_diff(&dropout(0.0, true), &jumped);
    assert!(
        as_restart <= 1e-9,
        "gated jump vs flagged restart: {:.4} µs",
        as_restart * 1e6
    );
    let lost = max_diff(&dropout(0.0, false), &jumped);
    assert!(lost <= 8e-6, "against unchanged audio {:.3} µs", lost * 1e6);
    let (_, sd) = mean_sd(&jumped.errors(300.0));
    assert!(sd <= 25e-6, "phase error sd {:.2} µs", sd * 1e6);
    // The short-term figure grows honestly while audio is away.
    let at = |t: f64| jumped.recs.iter().find(|r| r.t >= t).unwrap().status;
    let (before, during) = (at(1199.0), at(1499.0));
    assert!(before.audio && !during.audio);
    let (b, d) = (before.audio_sd_s.unwrap(), during.audio_sd_s.unwrap());
    assert!(d > 5.0 * b, "audio sd {:.3} → {:.3} µs", b * 1e6, d * 1e6);
    println!(
        "dropout: gated jump = restart within {:.4} µs, {:.3} µs from unchanged audio; audio sd \
         {:.3} → {:.2} µs over 5 min; phase sd {:.2} µs",
        as_restart * 1e6,
        lost * 1e6,
        b * 1e6,
        d * 1e6,
        sd * 1e6
    );
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
    assert!(
        r.recs
            .iter()
            .filter_map(|x| x.step)
            .all(|s| s.get() <= 1.001)
    );
}

#[test]
fn nothing_is_handed_out_before_acquiring() {
    let mut s = servo(ServoConfig::default());
    assert_eq!(s.block(0, 512), Err(NotReady::Acquiring));
    let at = MediaTime::from_nanos(1_000_000_000);
    assert_eq!(s.observe_audio(1e-3, 1e-6, at, false), AudioUse::Unplaced);
    s.observe_stamp(96_000, at);
    assert_eq!(s.block(96_000, 512), Err(NotReady::Acquiring));
    assert_eq!(s.status(at).mode, Mode::Acquiring);
    assert!(Servo::new(ServoConfig::default(), 0).is_err());
    let cfg = ServoConfig {
        wander_tau_s: f64::NAN,
        ..ServoConfig::default()
    };
    assert!(Servo::new(cfg, FS).is_err());
}
