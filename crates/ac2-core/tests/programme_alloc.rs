//! The dynamics programme's `fill` runs on the audio callback: no allocation or free,
//! through every segment, released rests and the closing fade. A counting global allocator
//! watches the test thread, so this test has its process to itself.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

use ac2_core::dynamics::{Onset, Programme, ProgrammeConfig, Release};
use ac2_core::generator::{BandLimit, Generator, GeneratorConfig, Signal};

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

fn count(f: impl FnOnce()) -> u64 {
    EVENTS.with(|e| e.set(0));
    ARMED.with(|a| a.set(true));
    f();
    ARMED.with(|a| a.set(false));
    EVENTS.with(Cell::get)
}

#[test]
fn the_programme_fills_without_allocating() {
    assert!(count(|| drop(std::hint::black_box(vec![1u8; 16]))) >= 2);
    let fs = 48_000.0;
    let programme = Arc::new(
        Programme::new(
            &ProgrammeConfig {
                carrier_hz: 1000.0,
                onset: Onset::Peak,
                pilot_dbfs: -40.0,
                bursts_dbfs: vec![-20.0, -10.0],
                durations_s: vec![0.003, 0.01],
                rest_max_s: 3.0,
                replay_rests_s: None,
            },
            fs,
        )
        .expect("programme"),
    );
    let release = Release::new();
    let mut g = Generator::new(&GeneratorConfig {
        signal: Signal::Programme {
            programme: Arc::clone(&programme),
            release: release.clone(),
        },
        sample_rate: fs,
        seed: 0,
        band: BandLimit::NONE,
        level_dbfs: programme.highest_dbfs(),
        ceiling_dbfs: 0.0,
    })
    .expect("generator");
    let mut block = vec![0.0f32; 480];
    let blocks = (programme.longest_seconds() * fs / 480.0) as usize + 10;
    let n = count(|| {
        for b in 0..blocks {
            // Releases mid-run: every rest after its minimum.
            if b % 500 == 0 {
                release.publish(u32::try_from(b / 500).unwrap_or(u32::MAX));
            }
            g.fill(&mut block);
        }
    });
    assert!(g.is_finished());
    assert_eq!(n, 0, "fill allocated or freed {n} times");
}
