//! What `max_block` and `max_backlog` guarantee, at sizes that fill a power-of-two ring
//! exactly, so no rounding up of the capacity can hide a shortfall.

use std::sync::Arc;

use ac2_resampler::{Kernel, Pos, Resampler, Step};

const BLOCK: usize = 100;
const BACKLOG: usize = 100;

fn signal() -> Vec<f32> {
    // Not periodic in the ring's length: overwritten history would change the output.
    (0..4000)
        .map(|i| (i as f32 * 0.0123).sin() * 0.8 + (i as f32 * 0.31).cos() * 0.1)
        .collect()
}

/// The resampler under test and one that holds the whole input, as the reference.
fn pair(input: &[f32]) -> (Arc<Kernel>, Resampler, Resampler) {
    let kernel = Arc::new(Kernel::measurement());
    // 256 = 2⁸: the ring is exactly this size, one sample more than the guarantee needs.
    assert_eq!(BLOCK + BACKLOG + kernel.taps(), 256);
    let r = Resampler::new(kernel.clone(), BLOCK, BACKLOG).unwrap();
    let mut reference = Resampler::new(kernel.clone(), input.len(), 0).unwrap();
    reference.push(input);
    (kernel, r, reference)
}

fn same(r: &Resampler, reference: &Resampler, pos: Pos, step: Step, n: usize) -> Pos {
    let (mut got, mut want) = (vec![0.0f32; n], vec![0.0f32; n]);
    let next = r.pull(pos, step, &mut got).unwrap();
    assert_eq!(reference.pull(pos, step, &mut want), Ok(next));
    assert!(
        got.iter()
            .zip(&want)
            .all(|(a, b)| a.to_bits() == b.to_bits()),
        "at {pos:?}"
    );
    next
}

/// The consumer leaves `BACKLOG` samples' worth of output unpulled after every push.
#[test]
fn a_full_backlog_at_an_exact_power_of_two() {
    let input = signal();
    let (kernel, mut r, reference) = pair(&input);
    let (history, look) = (kernel.history(), kernel.lookahead());
    let mut pos = Pos::new(history, 0.41).unwrap();
    let (mut pulled, mut at_edge) = (0, 0);
    for block in input.chunks(BLOCK) {
        r.push(block);
        // The pull below starts where the last round stopped: `BACKLOG` before the frontier
        // as it stood before this push, the deepest the guarantee allows.
        if pos.index() - history == r.oldest() + 1 {
            at_edge += 1;
        }
        let frontier = Pos::from_index(r.pushed().saturating_sub(look));
        let keep = frontier.shifted(-(BACKLOG as f64)).unwrap_or(pos);
        // Step 1 from fraction 0.41 lands exactly on `keep + 0.41`, index `frontier − BACKLOG`.
        while pos < keep {
            pos = same(&r, &reference, pos, Step::ONE, 1);
            pulled += 1;
        }
    }
    assert!(pulled > 3500, "{pulled}");
    // Every round once the ring is full reads the oldest sample but one: the slack is the
    // single sample `taps` has over the `taps − 1` the arithmetic needs.
    assert!(at_edge > 30, "{at_edge}");
}

/// The consumer pulls everything, then re-anchors `BACKLOG` samples backwards.
#[test]
fn a_backwards_step_at_an_exact_power_of_two() {
    let input = signal();
    let (kernel, mut r, reference) = pair(&input);
    let (history, look) = (kernel.history(), kernel.lookahead());
    let first = Pos::new(history, 0.41).unwrap();
    let step = Step::new(1.0 + 7e-4).unwrap();
    let mut pos = first;
    let mut rounds = 0;
    for block in input.chunks(BLOCK) {
        r.push(block);
        let n = r.available(pos, step).unwrap();
        assert_eq!(reference.available(pos, step).map(|m| m >= n), Ok(true));
        same(&r, &reference, pos, step, n);
        let frontier = Pos::from_index(r.pushed().saturating_sub(look));
        // Back to `BACKLOG` before the frontier, the farthest the guarantee allows.
        pos = frontier
            .shifted(-(BACKLOG as f64) + 0.41)
            .filter(|&p| p >= first)
            .unwrap_or(first);
        if pos > first {
            rounds += 1;
        }
    }
    assert!(rounds > 35, "{rounds}");
}
