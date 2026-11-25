//! What a pull refuses: too little input, too little history, positions before index 0.

use std::sync::Arc;

use ac2_resampler::{Error, Kernel, Pos, Resampler, Step};

fn resampler(max_block: usize) -> Resampler {
    Resampler::new(Arc::new(Kernel::measurement()), max_block)
}

#[test]
fn available_counts_outputs_whose_lookahead_is_pushed() {
    let mut r = resampler(256);
    let (look, step) = (r.kernel().lookahead(), Step::ONE);
    let start = Pos::from_index(r.kernel().history());
    assert_eq!(r.available(start, step), 0);
    r.push(&[0.0; 100]);
    // Outputs at history..=99−lookahead.
    assert_eq!(
        r.available(start, step) as u64,
        100 - look - r.kernel().history()
    );
    let mut out = vec![0.0; r.available(start, step) + 1];
    assert_eq!(
        r.pull(start, step, &mut out),
        Err(Error::InputShort {
            requested: out.len(),
            available: out.len() - 1
        })
    );
    // Any fraction past the last covered index is covered too: its taps end on that index's
    // lookahead.
    let slow = Step::new(1.0 - 1e-3).unwrap();
    assert!(r.available(start, slow) >= r.available(start, step));
    assert_eq!(r.available(Pos::new(99 - look, 0.999).unwrap(), step), 1);
    assert_eq!(r.available(Pos::from_index(100 - look), step), 0);
}

#[test]
fn refuses_positions_needing_input_before_index_0_or_dropped_history() {
    let mut r = resampler(64);
    r.push(&[0.5; 64]);
    let early = Pos::new(r.kernel().history() - 1, 0.5).unwrap();
    let mut out = [0.0; 1];
    assert_eq!(
        r.pull(early, Step::ONE, &mut out),
        Err(Error::BeforeHistory {
            pos: early,
            oldest: 0
        })
    );
    r.push(&[0.5; 4096]);
    let oldest = r.oldest();
    assert!(oldest > 0);
    let stale = Pos::from_index(oldest + r.kernel().history() - 1);
    assert!(matches!(
        r.pull(stale, Step::ONE, &mut out),
        Err(Error::BeforeHistory { .. })
    ));
    let ok = Pos::from_index(oldest + r.kernel().history());
    assert!(r.pull(ok, Step::ONE, &mut out).is_ok());
}

#[test]
fn reset_restarts_input_at_index_0() {
    let mut r = resampler(64);
    r.push(&[1.0; 200]);
    r.reset();
    assert_eq!(r.pushed(), 0);
    let start = Pos::from_index(r.kernel().history());
    assert_eq!(r.available(start, Step::ONE), 0);
    let fresh: Vec<f32> = (0..64).map(|i| i as f32).collect();
    r.push(&fresh);
    let mut out = [0.0; 1];
    r.pull(start, Step::ONE, &mut out).unwrap();
    assert_eq!(out[0], fresh[r.kernel().history() as usize]);
}

#[test]
fn a_constant_stays_constant_at_any_offset() {
    let mut r = resampler(512);
    r.push(&[0.25; 512]);
    let step = Step::new(1.0 + 1e-3).unwrap();
    let start = Pos::new(r.kernel().history(), 0.3).unwrap();
    let mut out = vec![0.0; r.available(start, step)];
    r.pull(start, step, &mut out).unwrap();
    // Every phase has unit DC gain; the f64 sum rounds back to the same f32.
    assert!(out.iter().all(|&y| y == 0.25), "{out:?}");
}
