//! `push` and `pull` may run beside audio threads: no allocation or free after
//! construction. A counting global allocator watches the test thread, so this test has its
//! process to itself.

#![allow(clippy::unwrap_used)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

use ac2_resampler::{Kernel, Pos, Resampler, Step};

struct Counting;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static EVENTS: Cell<u64> = const { Cell::new(0) };
}

fn note() {
    // `try_with`: the allocator also runs while thread-locals are being torn down.
    let _ = ARMED.try_with(|armed| {
        if armed.get() {
            let _ = EVENTS.try_with(|e| e.set(e.get() + 1));
        }
    });
}

// SAFETY: forwards every call unchanged to the system allocator; only counts.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        // SAFETY: same contract as our caller's.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note();
        // SAFETY: same contract as our caller's.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        // SAFETY: same contract as our caller's.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[test]
fn push_pull_and_reset_do_not_allocate() {
    let mut r = Resampler::new(Arc::new(Kernel::measurement()), 256, 0).unwrap();
    let input: Vec<f32> = (0..256).map(|i| (i as f32 * 0.05).sin()).collect();
    let big = vec![0.1f32; 10_000];
    let mut out = vec![0.0f32; 512];
    let step = Step::new(1.0 + 2.5e-5).unwrap();
    EVENTS.with(|e| e.set(0));
    ARMED.with(|a| a.set(true));
    let mut pos = Pos::new(r.kernel().history(), 0.5).unwrap();
    for _ in 0..50 {
        r.push(&input);
        let n = r.available(pos, step).unwrap().min(out.len());
        pos = r.pull(pos, step, &mut out[..n]).unwrap();
    }
    let _ = r.pull(pos, step, &mut out);
    r.push(&big);
    r.reset();
    ARMED.with(|a| a.set(false));
    assert_eq!(EVENTS.with(Cell::get), 0);
}
