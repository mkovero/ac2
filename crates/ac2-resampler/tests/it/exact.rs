//! Outputs on input samples reproduce them bit for bit.

use std::sync::{Arc, LazyLock};

use ac2_resampler::{Kernel, Pos, Resampler, Step};
use proptest::prelude::*;

fn special(bits: u32) -> f32 {
    f32::from_bits(bits)
}

#[test]
fn integer_positions_at_step_one_copy_bit_for_bit() {
    let kernel = Arc::new(Kernel::measurement());
    let mut input: Vec<f32> = (0..1000)
        .map(|i| ((i as f32) * 0.123).sin() * 0.9)
        .collect();
    // Signed zero, subnormals, extremes: a summing kernel would alter some of these.
    input[100..106].copy_from_slice(&[
        -0.0,
        special(1),
        special(0x8000_0001),
        f32::MAX,
        f32::MIN_POSITIVE,
        -1.0,
    ]);
    let mut r = Resampler::new(kernel.clone(), input.len());
    r.push(&input);
    let start = kernel.history();
    let n = r.available(Pos::from_index(start), Step::ONE);
    assert_eq!(n as u64, input.len() as u64 - kernel.taps() as u64 + 1);
    let mut out = vec![0.0f32; n];
    let next = r.pull(Pos::from_index(start), Step::ONE, &mut out).unwrap();
    assert_eq!(next, Pos::from_index(start + n as u64));
    for (i, (&y, &x)) in out.iter().zip(&input[start as usize..]).enumerate() {
        assert_eq!(y.to_bits(), x.to_bits(), "output {i}");
    }
}

/// Built once: building the 0.9 MB table per proptest case dominates a debug run.
static KERNEL: LazyLock<Arc<Kernel>> = LazyLock::new(|| Arc::new(Kernel::measurement()));

proptest! {
    #[test]
    fn any_integer_position_copies_its_sample(bits in prop::collection::vec(any::<u32>(), 64..200), at in 27u64..36) {
        let input: Vec<f32> = bits.into_iter().map(f32::from_bits).collect();
        let mut r = Resampler::new(KERNEL.clone(), input.len());
        r.push(&input);
        let mut out = [0.0f32; 1];
        r.pull(Pos::from_index(at), Step::ONE, &mut out).unwrap();
        prop_assert_eq!(out[0].to_bits(), input[at as usize].to_bits());
    }
}
