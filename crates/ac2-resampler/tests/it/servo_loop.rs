//! The servo's audio sign, pinned without the simulator's own assumption: a tone captured on
//! a drifting device goes through the real [`Resampler`] at the positions the servo hands,
//! its delay against the same tone on the media clock is measured from the output's phase,
//! and that delay closes the loop.

use std::f64::consts::PI;
use std::sync::Arc;

use ac2_resampler::servo::{AudioUse, MediaTime, Servo, ServoConfig};
use ac2_resampler::{Kernel, KernelSpec, Pos, Resampler};

use crate::servo_sim::{Rng, mean_sd};

/// A low rate keeps the run short; the servo's figures are in seconds.
const FS: u32 = 2_000;
const BLOCK: usize = 200;
const BLOCK_S: f64 = 0.1;
const SECS: f64 = 600.0;
/// Media time zero of the run: a whole second.
const T0_NS: u64 = 1_700_000_000_000_000_000;
/// The device's first sample, above 2³².
const J0: u64 = 5_000_000_000;
const PPM: f64 = 7.0;
const TONE: f64 = 250.0;
/// The acoustic path: a quarter period of the tone, so its phase sits far from the wrap.
const PATH_S: f64 = 1e-3;

/// How the delay reaches the servo.
#[derive(Clone, Copy, PartialEq)]
enum Audio {
    None,
    AsMeasured,
    Negated,
}

struct Outcome {
    /// Handed minus true device position at each block start from 300 s on, seconds.
    errors: Vec<f64>,
    used: usize,
    refused: u64,
}

/// Media time (s after T0) at which device sample `j` was captured.
fn captured(j: u64) -> f64 {
    (j - J0) as f64 / (f64::from(FS) * (1.0 + PPM * 1e-6))
}

fn run(audio: Audio) -> Outcome {
    let fs = f64::from(FS);
    let cfg = ServoConfig::default();
    let mut servo = Servo::new(cfg, FS).unwrap();
    let kernel = Arc::new(
        Kernel::new(KernelSpec {
            half_length: 8,
            phases: 256,
            beta: 8.0,
        })
        .unwrap(),
    );
    let mut res = Resampler::new(kernel, 2 * BLOCK, 4 * BLOCK).unwrap();
    let mut rng = Rng::new(11);
    let (sd, tau) = (15.5e-6, 100.0);
    let a = (-BLOCK_S / tau).exp();
    let mut wander = sd * rng.normal();
    let white = 5e-6 / BLOCK_S.sqrt();
    let k0 = T0_NS / 1_000_000_000 * u64::from(FS);
    let mut pushed = J0;
    let mut window: Vec<f32> = Vec::new();
    let mut window_start: Option<usize> = None;
    let mut out = vec![0.0f32; BLOCK];
    let mut outcome = Outcome {
        errors: Vec::new(),
        used: 0,
        refused: 0,
    };
    let n = (SECS / BLOCK_S) as usize;
    for b in 0..n {
        let tb = b as f64 * BLOCK_S;
        // The block's first whole device sample and its capture time, as stamped.
        let j = J0 + (tb * fs * (1.0 + PPM * 1e-6)).ceil() as u64;
        let stamp = captured(j) + wander + white * rng.normal();
        servo.observe_stamp(
            j,
            MediaTime::from_nanos(T0_NS + (stamp * 1e9).round() as u64),
        );
        wander = a * wander + sd * (1.0 - a * a).sqrt() * rng.normal();
        // The device has captured up to the end of this block, and a little beyond, which
        // the kernel's lookahead reads.
        let upto = J0 + ((tb + BLOCK_S) * fs * (1.0 + PPM * 1e-6)) as u64 + 64;
        let input = tone(2.0 * PI * TONE * (captured(pushed) - PATH_S), {
            2.0 * PI * TONE / (fs * (1.0 + PPM * 1e-6))
        })
        .take((upto - pushed) as usize)
        .map(|(sin, _)| sin as f32)
        .collect::<Vec<_>>();
        res.push(&input);
        pushed = upto;
        let first = k0 + (b * BLOCK) as u64;
        let Ok((pos, step)) = servo.block(first, BLOCK) else {
            window.clear();
            window_start = None;
            continue;
        };
        let truth = tb * fs * (1.0 + PPM * 1e-6);
        let err = ((pos.index() - J0) as f64 + pos.fraction() - truth) / fs;
        if tb >= 300.0 {
            outcome.errors.push(err);
        }
        let local = Pos::new(pos.index() - J0, pos.fraction()).unwrap();
        res.pull(local, step, &mut out).unwrap();
        window_start.get_or_insert(b * BLOCK);
        window.extend_from_slice(&out);
        if window.len() < FS as usize {
            continue;
        }
        // The output's phase against the media clock's tone over one second (whole cycles):
        // a delay d plays sin(ω(τ − d)).
        let start = window_start.take().unwrap();
        let w = 2.0 * PI * TONE / fs;
        let (mut s, mut c) = (0.0, 0.0);
        for (&y, (sin, cos)) in window.iter().zip(tone(w * start as f64, w)) {
            s += f64::from(y) * sin;
            c += f64::from(y) * cos;
        }
        window.clear();
        let delay = -c.atan2(s) / (2.0 * PI * TONE);
        let centre = (start as f64 + fs / 2.0) / fs;
        let at = MediaTime::from_nanos(T0_NS + (centre * 1e9) as u64);
        let fed = match audio {
            Audio::None => continue,
            Audio::AsMeasured => delay,
            Audio::Negated => -delay,
        };
        if tb >= 60.0 {
            match servo.observe_audio(fed, 0.1e-6, at, false) {
                AudioUse::Used => outcome.used += 1,
                AudioUse::BiasReset | AudioUse::Unplaced => {}
            }
        }
    }
    outcome.refused = servo.status(MediaTime::from_nanos(T0_NS)).audio_refused;
    outcome
}

/// `(sin, cos)` of `phase + n·step` for n = 0, 1, …: a rotating phasor, exact at the start
/// and drifting by about 1e-16 per sample, nothing over the few thousand samples asked of it.
fn tone(phase: f64, step: f64) -> impl Iterator<Item = (f64, f64)> {
    let (ds, dc) = step.sin_cos();
    std::iter::successors(Some(phase.sin_cos()), move |&(s, c)| {
        Some((s * dc + c * ds, c * dc - s * ds))
    })
}

/// Errors minus their centred moving average over three minutes.
fn fast_sd(e: &[f64]) -> f64 {
    let half = (90.0 / BLOCK_S) as usize;
    let fast: Vec<f64> = (half..e.len() - half)
        .map(|i| e[i] - e[i - half..=i + half].iter().sum::<f64>() / (2 * half + 1) as f64)
        .collect();
    mean_sd(&fast).1
}

/// The same stamps alone, with the delay as measured, and with it negated: as measured it
/// halves the phase error faster than three minutes; negated it runs away to a millisecond.
#[test]
fn the_measured_delay_closes_the_loop_and_its_sign_is_pinned() {
    let stamps = run(Audio::None);
    let right = run(Audio::AsMeasured);
    let flipped = run(Audio::Negated);
    let (s, r, f) = (
        fast_sd(&stamps.errors),
        fast_sd(&right.errors),
        fast_sd(&flipped.errors),
    );
    let worst = |o: &Outcome| o.errors.iter().fold(0.0, |m: f64, e| m.max(e.abs()));
    println!(
        "phase error faster than 3 min: stamps only {:.2} µs, audio {:.3} µs ({} used, {} \
         refused), audio negated {:.2} µs ({} used, {} refused); worst {:.1} / {:.1} / {:.1} µs",
        s * 1e6,
        r * 1e6,
        right.used,
        right.refused,
        f * 1e6,
        flipped.used,
        flipped.refused,
        worst(&stamps) * 1e6,
        worst(&right) * 1e6,
        worst(&flipped) * 1e6
    );
    // As measured: every observation fits, and the phase holds tighter than on stamps.
    assert_eq!(right.refused, 0);
    assert!(right.used >= 500, "{} used", right.used);
    assert!(
        r <= 0.7 * s,
        "fast part {:.3} µs, stamps only {:.3}",
        r * 1e6,
        s * 1e6
    );
    assert!(worst(&right) <= worst(&stamps));
    // Negated, the loop feeds its own error back with the wrong sign and runs away.
    assert!(
        worst(&flipped) >= 100e-6,
        "negated: worst {:.1} µs",
        worst(&flipped) * 1e6
    );
}
