//! The callback path must not allocate or free. A counting global allocator watches the
//! test thread while it runs simulated callbacks, which exercise the same transport, clock,
//! event latch, renderer (fades, source swaps, limiting) and history code as the real
//! backends.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use ac2_audio::fake::{FakeFault, FakePath};
use ac2_audio::generator::{Sine, WhiteNoise};
use ac2_audio::{
    DuplexRequest, FakeBackend, FakeConfig, Gain, HistoryRequest, MaxLevel, OutputSource, generator,
};

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

#[test]
fn simulated_callbacks_do_not_allocate() {
    // The guard itself must see allocations, or the zeros below prove nothing.
    assert!(count(|| drop(std::hint::black_box(vec![1u8; 16]))) >= 2);

    let backend = FakeBackend::new(FakeConfig {
        input_noise_rms: 0.001,
        drift_ppm: 50.0,
        paths: vec![
            FakePath::loopback(0, 1, 400),
            FakePath::acoustic(0, 0, 900, vec![0.5, 0.3, -0.1], 0.01),
        ],
        faults: vec![
            FakeFault::XrunFlag { at_block: 5 },
            FakeFault::Xrun {
                at_block: 9,
                lost_frames: 300,
            },
            FakeFault::ConfigChange { at_block: 12 },
            FakeFault::DropOutputFrames {
                at_output_sample: 4000,
                frames: 17,
            },
            FakeFault::RepeatOutputFrames {
                at_output_sample: 8000,
                frames: 64,
            },
        ],
        ..FakeConfig::default()
    })
    .expect("config");
    let (mut g, port) = generator([0, 1]).expect("routes");
    g.set_source(Box::new(WhiteNoise::new(0.5, 1)))
        .expect("queue");
    g.start();
    let mut req = DuplexRequest::new(
        vec![0, 1, 2],
        2,
        MaxLevel::from_peak_db(-12.0).expect("level"),
    );
    req.output = OutputSource::Generator(port);
    req.history = Some(HistoryRequest::channel(0));
    req.ring_seconds = 0.1; // Small, so the overflow path runs too.
    let (stream, mut driver) = backend.open_manual(req).expect("open");

    assert_eq!(
        count(|| driver.run_blocks(30)),
        0,
        "steady state, faults, limiting"
    );

    // A source swap: the box is allocated here, on the control side...
    g.set_source(Box::new(Sine::new(997.0, 48_000, 0.1)))
        .expect("queue");
    g.set_gain(Gain::from_db(-3.0).expect("gain"));
    // ...and the fade, swap and hand-back on the callback side must not allocate or free.
    assert_eq!(count(|| driver.run_blocks(30)), 0, "swap and gain ramp");
    assert_eq!(g.collect_retired(), 1);

    stream.begin_stop();
    assert_eq!(count(|| driver.run_blocks(10)), 0, "fade-out");
    assert!(
        stream.transport_stats().blocks_dropped > 0,
        "overflow path exercised"
    );
    assert!(
        stream.output_stats().limited_samples > 0,
        "limiter exercised"
    );
}
