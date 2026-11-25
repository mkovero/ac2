//! Output does not depend on how input and output are split into calls.

use std::sync::Arc;

use ac2_resampler::{Kernel, KernelSpec, Pos, Resampler, Step};
use proptest::prelude::*;

/// Everything from one push and one pull.
fn whole(kernel: &Arc<Kernel>, input: &[f32], start: Pos, step: Step) -> Vec<f32> {
    let mut r = Resampler::new(kernel.clone(), input.len());
    r.push(input);
    let mut out = vec![0.0; r.available(start, step)];
    r.pull(start, step, &mut out).unwrap();
    out
}

/// Input pushed in `pushes`-sized blocks; after each, outputs pulled in `pulls`-sized pieces
/// (cycling) as long as the input covers them.
fn chunked(
    kernel: &Arc<Kernel>,
    input: &[f32],
    start: Pos,
    step: Step,
    pushes: &[usize],
    pulls: &[usize],
) -> Vec<f32> {
    let mut r = Resampler::new(kernel.clone(), *pushes.iter().max().unwrap_or(&1));
    let (mut out, mut pos, mut at) = (Vec::new(), start, 0);
    let mut pull_sizes = pulls.iter().cycle();
    for &len in pushes.iter().cycle() {
        if at == input.len() {
            break;
        }
        let end = (at + len).min(input.len());
        r.push(&input[at..end]);
        at = end;
        loop {
            let n = (*pull_sizes.next().unwrap()).min(r.available(pos, step));
            if n == 0 {
                break;
            }
            let mut piece = vec![0.0; n];
            pos = r.pull(pos, step, &mut piece).unwrap();
            out.extend(piece);
        }
    }
    out
}

fn kernel() -> Arc<Kernel> {
    // Small table so the property runs fast in a debug build; the code path is the preset's.
    Arc::new(
        Kernel::new(KernelSpec {
            half_length: 6,
            phases: 64,
            beta: 8.0,
        })
        .unwrap(),
    )
}

proptest! {
    #[test]
    fn split_calls_give_identical_output(
        input in prop::collection::vec(-1.0f32..1.0, 20..600),
        frac in 0.0f64..1.0,
        ppm in -1000.0f64..1000.0,
        pushes in prop::collection::vec(1usize..90, 1..8),
        pulls in prop::collection::vec(1usize..90, 1..8),
    ) {
        let kernel = kernel();
        let start = Pos::new(kernel.history() + 1, frac).unwrap();
        let step = Step::new(1.0 + ppm * 1e-6).unwrap();
        let reference = whole(&kernel, &input, start, step);
        let split = chunked(&kernel, &input, start, step, &pushes, &pulls);
        prop_assert_eq!(reference.len(), split.len());
        for (a, b) in reference.iter().zip(&split) {
            prop_assert_eq!(a.to_bits(), b.to_bits());
        }
    }
}

#[test]
fn preset_split_into_odd_blocks_matches_one_call() {
    let kernel = Arc::new(Kernel::measurement());
    let input: Vec<f32> = (0..5000).map(|i| ((i as f32) * 0.0371).sin()).collect();
    let start = Pos::new(40, 0.613).unwrap();
    let step = Step::new(1.0 - 7.3e-4).unwrap();
    let reference = whole(&kernel, &input, start, step);
    let split = chunked(
        &kernel,
        &input,
        start,
        step,
        &[1, 97, 256, 13],
        &[5, 300, 1, 64],
    );
    assert_eq!(reference.len(), split.len());
    assert!(
        reference
            .iter()
            .zip(&split)
            .all(|(a, b)| a.to_bits() == b.to_bits())
    );
}
