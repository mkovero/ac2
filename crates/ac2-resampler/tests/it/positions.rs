//! Output positions far from index 0 and steps that change between pulls, against the
//! analytic signal at the positions the fixed-point arithmetic reached.

use std::f64::consts::PI;
use std::sync::Arc;

use ac2_resampler::{Kernel, Pos, Resampler, Step};

const FS: f64 = 48_000.0;
const PHASE: f64 = 0.3;

fn tone(freq: f64, n: usize) -> Vec<f32> {
    let w = 2.0 * PI * freq / FS;
    (0..n)
        .map(|j| (w * j as f64 + PHASE).sin() as f32)
        .collect()
}

/// Peak error, dB re the tone, of `out` against the tone at `pos`, `pos + step`, …, with
/// `origin` the position of the tone's sample 0.
fn error_db(freq: f64, origin: Pos, pos: Pos, step: Step, out: &[f32]) -> f64 {
    let w = 2.0 * PI * freq / FS;
    let mut peak = 0.0f64;
    for (i, &y) in out.iter().enumerate() {
        let t = pos.advanced(step, i as u64).unwrap().diff(origin);
        peak = peak.max((f64::from(y) - (w * t + PHASE).sin()).abs());
    }
    20.0 * peak.log10()
}

/// Over 2³² samples pushed, the ring wrapped two million times: the same input gives the
/// same bits at index 2³² + 12345 as at index 0, and both match the tone.
#[test]
fn far_from_index_0_the_output_is_the_same_bits() {
    const BASE: u64 = (1 << 32) + 12_345;
    const BLOCK: usize = 1024;
    const FREQ: f64 = 15_887.0;
    let kernel = Arc::new(Kernel::measurement());
    let mut far = Resampler::new(kernel.clone(), BLOCK, 0).unwrap();
    let mut near = Resampler::new(kernel.clone(), BLOCK, 0).unwrap();
    // A block longer than the ring keeps only its tail, so a 1 Mi-sample push costs one
    // ring's worth of writes.
    let zeros = vec![0.0f32; 1 << 20];
    while far.pushed() + zeros.len() as u64 <= BASE {
        far.push(&zeros);
    }
    far.push(&zeros[..(BASE - far.pushed()) as usize]);
    assert_eq!(far.pushed(), BASE);

    let input = tone(FREQ, 16 * BLOCK);
    let step = Step::new(1.0 - 2.7e-4).unwrap();
    let at = kernel.history() + 13;
    let (mut p_far, mut p_near) = (
        Pos::new(BASE + at, 0.613).unwrap(),
        Pos::new(at, 0.613).unwrap(),
    );
    let (mut worst, mut total) = (f64::NEG_INFINITY, 0);
    for block in input.chunks(BLOCK) {
        far.push(block);
        near.push(block);
        let n = far.available(p_far, step).unwrap();
        assert_eq!(near.available(p_near, step), Ok(n));
        let (mut a, mut b) = (vec![0.0f32; n], vec![0.0f32; n]);
        let next_far = far.pull(p_far, step, &mut a).unwrap();
        p_near = near.pull(p_near, step, &mut b).unwrap();
        assert!(a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()));
        worst = worst.max(error_db(FREQ, Pos::from_index(BASE), p_far, step, &a));
        p_far = next_far;
        assert_eq!(p_far.diff(p_near), BASE as f64);
        total += n;
    }
    println!("{total} outputs at index 2³² + 12345: worst {worst:.1} dB");
    assert!(total > 15 * BLOCK);
    // Measured −131.8 dB.
    assert!(worst <= -130.3, "{worst:.1} dB");
}

/// A servo changes the step every block: each pull continues from the position the last one
/// returned, and every output matches the tone at its exact position.
#[test]
fn steps_changing_between_pulls_land_on_the_tone() {
    let kernel = Arc::new(Kernel::measurement());
    let steps = [
        Step::new(1.0 + 1e-3).unwrap(),
        Step::new(1.0 - 1e-3).unwrap(),
        Step::from_deviation(3.3e-6).unwrap(),
        Step::ONE,
        Step::from_deviation(-7.7e-4).unwrap(),
        Step::new(1.0 + 2e-5).unwrap(),
    ];
    let sizes = [17, 64, 5, 200, 33, 1];
    let mut worst = f64::NEG_INFINITY;
    for freq in [1_000.0, 15_887.0] {
        let input = tone(freq, 12_000);
        let mut r = Resampler::new(kernel.clone(), input.len(), 0).unwrap();
        r.push(&input);
        let origin = Pos::from_index(0);
        let mut pos = Pos::new(kernel.history(), 0.77).unwrap();
        let mut pulls = 0;
        for (&step, &size) in steps.iter().cycle().zip(sizes.iter().cycle()) {
            let n = size.min(r.available(pos, step).unwrap());
            if n == 0 {
                break;
            }
            let mut out = vec![0.0f32; n];
            let next = r.pull(pos, step, &mut out).unwrap();
            assert_eq!(Some(next), pos.advanced(step, n as u64));
            worst = worst.max(error_db(freq, origin, pos, step, &out));
            pos = next;
            pulls += 1;
        }
        assert!(pulls > 100, "{pulls}");
    }
    println!("steps changing per pull: worst {worst:.1} dB");
    // Measured −132.3 dB.
    assert!(worst <= -130.8, "{worst:.1} dB");
}
