//! The servo as the joiner runs it: [`ServoConfig::cross_rig`], one stamp a second, a remote
//! device at +7.3 ppm and stamps erring by the two hosts' wander and white part together.

use ac2_resampler::servo::{MediaTime, Servo, ServoConfig};

use crate::servo::{SEEDS, go};
use crate::servo_sim::{FS, Run, Sim};

/// One stamp a second (every tenth 0.1 s block), at the figures `cross_rig` is built from.
fn joiner(seed: u64, secs: f64) -> Sim {
    let cfg = ServoConfig::cross_rig();
    let mut sim = Sim::stamps_only(seed, secs);
    sim.ppm = 7.3;
    sim.drift_ppm_per_h = 0.0;
    sim.wander_sd_s = cfg.wander_sd_s;
    sim.white_s = cfg.stamp_white_s;
    sim.stamp_every = 10;
    sim.cfg = cfg;
    sim
}

/// The note's estimate of the rate sd over a span `t` of stamps: two wander samples `t`
/// apart, at the wander's sd and correlation time.
fn wander_rate_sd_ppm(t: f64) -> f64 {
    let cfg = ServoConfig::cross_rig();
    cfg.wander_sd_s * (2.0 * (1.0 - (-t / cfg.wander_tau_s).exp())).sqrt() / t * 1e6
}

fn rate_sd_at(r: &Run, t: f64) -> f64 {
    let rec = r.recs.iter().find(|x| x.t >= t).unwrap();
    rec.status.rate_sd_ppm.unwrap()
}

/// Acquired (the first block handed) at the 23rd stamp, 23.0 s, on every seed (the rate sd
/// is the model's, the same for any stamps that pass the gate), against the default config's
/// 11.3 s at ten stamps a second: the wider wander keeps the rate sd over 1 ppm for longer.
/// The rate errs by up to 1.0 ppm then (one sd).
#[test]
fn acquires_at_one_stamp_a_second() {
    for seed in SEEDS {
        let r = go(&joiner(seed, 60.0));
        let acquired = r.first_block_after(0.0).unwrap();
        let at = r.recs.iter().find(|x| x.t >= acquired).unwrap().status;
        let sd = at.rate_sd_ppm.unwrap();
        let err = at.rate_ppm.unwrap() - 7.3;
        println!(
            "seed {seed}: acquired {acquired:.1} s, rate sd {sd:.3} ppm, rate error {err:+.3} ppm"
        );
        assert!(
            (15.0..=30.0).contains(&acquired),
            "acquired after {acquired:.1} s"
        );
        assert!(sd <= 1.0, "rate sd {sd:.3} ppm at acquisition");
        assert!(
            err.abs() <= 3.0 * sd,
            "rate {err:+.3} ppm off at sd {sd:.3}"
        );
    }
}

/// The rate sd falls with the span of stamps about as the wander alone predicts: at 28,
/// 100 and 280 s the model reads 0.88, 0.38 and 0.16 ppm against the estimate's 0.82, 0.37
/// and 0.16 (early on the white part and the prior still add to the wander).
#[test]
fn rate_sd_falls_with_the_span_of_stamps() {
    let r = go(&joiner(1, 300.0));
    let mut last = f64::INFINITY;
    for t in [28.0, 100.0, 280.0] {
        let sd = rate_sd_at(&r, t);
        let est = wander_rate_sd_ppm(t);
        println!("{t:.0} s: rate sd {sd:.3} ppm, estimate {est:.3} ppm");
        assert!(
            (0.6 * est..=1.3 * est).contains(&sd),
            "rate sd {sd:.3} ppm at {t} s, estimate {est:.3}"
        );
        assert!(sd < last);
        last = sd;
    }
    assert!(rate_sd_at(&r, 100.0) < 0.5);
}

/// `reset` forgets the rate and its sd; `reset_phase` keeps both, the sd growing only by the
/// rate walk until stamps return.
#[test]
fn rate_sd_follows_the_rate_over_resets() {
    let mut s = Servo::new(ServoConfig::cross_rig(), FS).unwrap();
    let now = MediaTime::from_nanos;
    assert_eq!(s.status(now(1_000_000_000)).rate_sd_ppm, None);
    for j in 0..=120u64 {
        let ns = 1_000_000_000 + (j as f64 / (1.0 + 7.3e-6) * 1e9) as u64;
        s.observe_stamp((1 << 34) + j * FS as u64, now(ns));
    }
    let at = now(121_000_000_000);
    let before = s.status(at);
    let sd = before.rate_sd_ppm.unwrap();
    assert!(sd > 0.0 && sd < 0.5, "rate sd {sd:.3} ppm after 120 s");
    s.reset_phase();
    let kept = s.status(at);
    assert_eq!(kept.rate_ppm, before.rate_ppm);
    let kept_sd = kept.rate_sd_ppm.unwrap();
    assert!(
        kept_sd >= sd && kept_sd < sd * 1.01,
        "kept {kept_sd} from {sd}"
    );
    // An hour later the walk (0.1 ppm/h) has added in quadrature.
    let later = s.status(now(3_721_000_000_000)).rate_sd_ppm.unwrap();
    let walked = (sd * sd + 0.1f64.powi(2)).sqrt();
    assert!(
        (later - walked).abs() < 0.01,
        "{later:.4} against {walked:.4}"
    );
    s.reset();
    assert_eq!(s.status(at).rate_sd_ppm, None);
    assert_eq!(s.status(at).rate_ppm, None);
}

/// A 5.6 ppm rate step (an interface relocking) five minutes in: the model's crystal walks
/// 0.1 ppm an hour, so the filter takes the step for wander at first and its rate follows
/// over minutes: 6 % of the step after 20 s, 14 % after 60 s, 63 % only after 7.5 min,
/// while the phase errs by up to 0.49 ms on the way. Why the joiner re-acquires on its rate
/// windows rather than waiting for the servo.
#[test]
fn a_rate_step_is_followed_only_slowly() {
    let step_at = 300.0;
    let mut sim = joiner(1, 900.0);
    sim.rate_step = Some((step_at, 5.6));
    let r = go(&sim);
    let rate_at = |t: f64| {
        r.recs
            .iter()
            .find(|x| x.t >= t)
            .unwrap()
            .status
            .rate_ppm
            .unwrap()
    };
    let before = rate_at(step_at - 1.0);
    let moved = |t: f64| (rate_at(t) - before) / 5.6;
    let followed = |share: f64| {
        r.recs
            .iter()
            .find(|x| x.t >= step_at && (x.status.rate_ppm.unwrap() - before) / 5.6 >= share)
            .map(|x| x.t - step_at)
    };
    let worst_ms = r
        .recs
        .iter()
        .filter(|x| x.t >= step_at)
        .filter_map(|x| x.err_s)
        .fold(0.0, |m: f64, e| m.max(e.abs()))
        * 1e3;
    println!(
        "step 5.6 ppm: moved {:.0} % at 20 s, {:.0} % at 60 s; 63 % after {:?} s, 90 % after \
         {:?} s; worst phase error {worst_ms:.3} ms",
        moved(step_at + 20.0) * 100.0,
        moved(step_at + 60.0) * 100.0,
        followed(0.63),
        followed(0.9),
    );
    assert!(
        moved(step_at + 20.0) < 0.2,
        "moved {:.0} % in 20 s",
        moved(step_at + 20.0) * 100.0
    );
    assert!(followed(0.63).is_none_or(|t| t > 180.0));
}
