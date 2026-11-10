//! The level follower decides on every frame drawn, over windows of minutes of frames: once
//! its ring and scratch have grown, a frame and a decision allocate nothing. A counting
//! global allocator watches the test thread.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use ac2_scene::axis::Range;
use ac2_scene::view::follow::{Follow, Sample};

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

/// Allocations and frees made by `f` on this thread.
fn count(f: impl FnOnce()) -> u64 {
    EVENTS.with(|e| e.set(0));
    ARMED.with(|a| a.set(true));
    f();
    ARMED.with(|a| a.set(false));
    EVENTS.with(Cell::get)
}

/// Steady content under a range it fits: every rule reads its window each tick (the slow
/// ones once a second) and none moves, so the follower runs as on a quiet show.
#[test]
fn a_steady_follower_allocates_nothing_per_frame() {
    const DT: f64 = 0.1;
    let r = Range::new(-80.0, 0.0);
    let s = |i: u32| Sample {
        top: -20.0 - f64::from(i % 7),
        floor: -60.0 - f64::from(i % 5),
    };
    let mut f = Follow::default();
    let mut t = 0.0;
    let mut i = 0;
    // Past the slow window, so the ring is full and drops one frame per frame pushed.
    while t < 600.0 {
        t += DT;
        i += 1;
        f.push(t, s(i));
        assert_eq!(f.decide(t, r), None, "steady content moves nothing");
    }
    let n = count(|| {
        for _ in 0..3000 {
            t += DT;
            i += 1;
            f.push(t, s(i));
            let _ = f.decide(t, r);
        }
    });
    assert_eq!(n, 0, "allocations in 300 s of frames");
}
