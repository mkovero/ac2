//! End to end: a 1 kHz tone captured on the simulated drifting device, re-timed through the
//! servo and the resampler onto the media clock, against the tone as the media clock sees it.

use std::f64::consts::PI;
use std::sync::Arc;

use ac2_resampler::servo::Servo;
use ac2_resampler::{Kernel, Resampler};

use crate::servo_sim::{BLOCK, BLOCK_S, FS, Sim, run};

const TONE: f64 = 1_000.0;

#[test]
fn a_tone_lands_on_the_media_clock_within_the_stated_phase() {
    let sim = Sim::stamps_only(4, 600.0);
    let mut servo = Servo::new(sim.cfg, FS).unwrap();
    let (r, truth) = run(&sim, &mut servo);
    let fs = f64::from(FS);
    // Five whole blocks with truth well past them.
    let n = r.recs.len();
    let blocks = n - 8..n - 3;
    let first = &r.recs[blocks.start];
    let pos0 = first.pos.unwrap();
    let kernel = Arc::new(Kernel::measurement());
    // Device samples from well before the first position to well past the last.
    let base = pos0.index() - 2 * kernel.history();
    let count = 6 * BLOCK;
    let mut res = Resampler::new(kernel, count, 0).unwrap();
    // The device sample `j`'s media time (s after the start) from the piecewise-linear truth;
    // the start is a whole second, so the tone's phase there is whole cycles.
    let mut b = blocks.start - 1;
    let input: Vec<f32> = (base..base + count as u64)
        .map(|j| {
            while truth.x[b + 1].whole < j {
                b += 1;
            }
            let (x0, x1) = (truth.x[b], truth.x[b + 1]);
            let into = (j - x0.whole) as f64 - x0.frac;
            let span = (x1.whole - x0.whole) as f64 + x1.frac - x0.frac;
            let t = b as f64 * BLOCK_S + into / span * BLOCK_S;
            (2.0 * PI * TONE * (t - t.floor())).sin() as f32
        })
        .collect();
    res.push(&input);
    let mut pos = pos0.shifted(-(base as f64)).unwrap();
    let mut out = vec![0.0f32; 5 * BLOCK];
    for (i, k) in blocks.clone().enumerate() {
        let step = r.recs[k].step.unwrap();
        pos = res
            .pull(pos, step, &mut out[i * BLOCK..(i + 1) * BLOCK])
            .unwrap();
    }
    // The phase of the output against sin(ω·i/fs) from the block start, which is media time
    // (blocks.start · 0.1 s) after a whole second: whole cycles at 1 kHz.
    let w = 2.0 * PI * TONE / fs;
    let (mut s, mut c) = (0.0, 0.0);
    for (i, &y) in out.iter().enumerate() {
        s += f64::from(y) * (w * i as f64).sin();
        c += f64::from(y) * (w * i as f64).cos();
    }
    let measured = c.atan2(s);
    // A position `e` seconds ahead of the truth plays the tone `e` later: phase +ω·e.
    let errs: Vec<f64> = blocks.clone().map(|k| r.recs[k].err_s.unwrap()).collect();
    let mean_err = errs.iter().sum::<f64>() / errs.len() as f64;
    let expected = 2.0 * PI * TONE * mean_err;
    assert!(
        (measured - expected).abs() <= 1e-4,
        "phase {measured:.6} rad, the handed positions say {expected:.6}"
    );
    println!(
        "1 kHz after 10 min stamps only: {:.3}° (positions say {:.3}°)",
        measured.to_degrees(),
        expected.to_degrees(),
    );
}
