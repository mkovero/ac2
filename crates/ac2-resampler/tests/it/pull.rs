//! What a pull refuses: too little input, too little history, positions before index 0; and
//! `available` refusing the same.

use std::sync::Arc;

use ac2_resampler::{Error, Kernel, Pos, Resampler, Step};

fn resampler(max_block: usize) -> Resampler {
    Resampler::new(Arc::new(Kernel::measurement()), max_block, 0).unwrap()
}

#[test]
fn available_counts_outputs_whose_lookahead_is_pushed() {
    let mut r = resampler(256);
    let (look, step) = (r.kernel().lookahead(), Step::ONE);
    let start = Pos::from_index(r.kernel().history());
    assert_eq!(r.available(start, step), Ok(0));
    r.push(&[0.0; 100]);
    // Outputs at history..=99−lookahead.
    let n = r.available(start, step).unwrap();
    assert_eq!(n as u64, 100 - look - r.kernel().history());
    let mut out = vec![0.0; n + 1];
    assert_eq!(
        r.pull(start, step, &mut out),
        Err(Error::InputShort {
            requested: n + 1,
            available: n
        })
    );
    assert!(r.pull(start, step, &mut out[..n]).is_ok());
    // Any fraction past the last covered index is covered too: its taps end on that index's
    // lookahead.
    let slow = Step::new(1.0 - 1e-3).unwrap();
    assert!(r.available(start, slow).unwrap() >= n);
    assert_eq!(
        r.available(Pos::new(99 - look, 0.999).unwrap(), step),
        Ok(1)
    );
    assert_eq!(r.available(Pos::from_index(100 - look), step), Ok(0));
}

/// `available` and `pull` make one check: a count `available` gives is one `pull` accepts,
/// and a position `pull` refuses for missing history `available` refuses too.
#[test]
fn available_refuses_what_pull_refuses() {
    let mut r = resampler(256);
    r.push(&[0.0; 100]);
    let zero = Pos::from_index(0);
    let refused = Err(Error::BeforeHistory {
        pos: zero,
        oldest: 0,
    });
    assert_eq!(r.available(zero, Step::ONE), refused);
    assert_eq!(
        r.pull(zero, Step::ONE, &mut [0.0; 72]).map(|_| ()),
        refused.map(|_| ())
    );
    // An empty pull goes through the same check.
    assert_eq!(
        r.pull(zero, Step::ONE, &mut []).map(|_| ()),
        refused.map(|_| ())
    );
    let start = Pos::from_index(r.kernel().history());
    assert_eq!(r.pull(start, Step::ONE, &mut []), Ok(start));

    // Stale: the input has moved on past the position's history.
    r.push(&[0.5; 4096]);
    let oldest = r.oldest();
    let stale = Pos::new(oldest + r.kernel().history() - 1, 0.5).unwrap();
    let refused = Error::BeforeHistory { pos: stale, oldest };
    assert_eq!(r.available(stale, Step::ONE), Err(refused));
    assert_eq!(r.pull(stale, Step::ONE, &mut []), Err(refused));
    assert_eq!(r.pull(stale, Step::ONE, &mut [0.0; 1]), Err(refused));
    let ok = Pos::from_index(oldest + r.kernel().history());
    let n = r.available(ok, Step::ONE).unwrap();
    assert!(n > 0);
    let mut out = vec![0.0; n];
    assert!(r.pull(ok, Step::ONE, &mut out).is_ok());
}

#[test]
fn refuses_positions_needing_input_before_index_0() {
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
}

#[test]
fn reset_restarts_input_at_index_0() {
    let mut r = resampler(64);
    r.push(&[1.0; 200]);
    r.reset();
    assert_eq!(r.pushed(), 0);
    let start = Pos::from_index(r.kernel().history());
    assert_eq!(r.available(start, Step::ONE), Ok(0));
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
    let mut out = vec![0.0; r.available(start, step).unwrap()];
    r.pull(start, step, &mut out).unwrap();
    // Every phase has unit DC gain; the f64 sum rounds back to the same f32.
    assert!(out.iter().all(|&y| y == 0.25), "{out:?}");
}

#[test]
fn absurd_retention_is_refused() {
    let kernel = Arc::new(Kernel::measurement());
    for (block, backlog) in [
        (usize::MAX, 0),
        (0, usize::MAX),
        (usize::MAX / 2 + 1, usize::MAX / 2 + 1),
        (Resampler::MAX_RETAINED, 0),
        (Resampler::MAX_RETAINED - kernel.taps(), 1),
    ] {
        assert_eq!(
            Resampler::new(kernel.clone(), block, backlog).unwrap_err(),
            Error::RetentionTooLarge {
                max_block: block,
                max_backlog: backlog
            }
        );
    }
    assert!(Resampler::new(kernel.clone(), Resampler::MAX_RETAINED - kernel.taps(), 0).is_ok());
}
