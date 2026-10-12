//! The IOProc closure: capture into the ring, render the generator, latch events. Nothing
//! here allocates, locks or makes a system call.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use cahal::{Cycle, Gap};

use crate::block::{BlockFlags, BlockProducer, BlockStamp};
use crate::events::{BackendEvents, EventLatch};
use crate::output::{OutputRenderer, OutputStamp};

/// What the closure shares with the control side.
#[derive(Debug, Default)]
pub(crate) struct Shared {
    /// The index fell back to a frame count (a sample time was invalid).
    pub(crate) estimated: AtomicBool,
    /// `output_sample_time − input_sample_time` of the latest cycle, `f64` bits; `NO_DIFF`
    /// until a duplex cycle has run.
    pub(crate) out_minus_in: AtomicU64,
}

/// No duplex cycle yet.
pub(crate) const NO_DIFF: u64 = u64::MAX;

impl Shared {
    pub(crate) fn new() -> Self {
        Self {
            estimated: AtomicBool::new(false),
            out_minus_in: AtomicU64::new(NO_DIFF),
        }
    }

    /// The latest cycle's output − input sample time, in whole frames.
    pub(crate) fn out_minus_in(&self) -> Option<u32> {
        match self.out_minus_in.load(Ordering::Acquire) {
            NO_DIFF => None,
            bits => {
                let d = f64::from_bits(bits);
                (d.is_finite() && d >= 0.0).then(|| d.round() as u32)
            }
        }
    }
}

/// The capture half: device input channels into the ring.
pub(crate) struct Capture {
    pub(crate) producer: BlockProducer,
    /// Device input channel of each block channel.
    pub(crate) map: Box<[usize]>,
}

/// The playback half.
pub(crate) struct Render {
    pub(crate) renderer: OutputRenderer,
}

/// One IOProc's closure state.
pub(crate) struct Engine {
    pub(crate) capture: Option<Capture>,
    pub(crate) render: Option<Render>,
    pub(crate) latch: EventLatch,
    pub(crate) events: Arc<BackendEvents>,
    pub(crate) shared: Arc<Shared>,
}

/// A cycle's gap as block flags.
pub(crate) fn gap_flags(gap: Gap) -> BlockFlags {
    match gap {
        Gap::First => BlockFlags::FIRST,
        Gap::None => BlockFlags::NONE,
        Gap::Forward(0) => BlockFlags::DISCONTINUITY | BlockFlags::GAP_ESTIMATED,
        Gap::Forward(_) | Gap::Backward => BlockFlags::DISCONTINUITY,
    }
}

impl Engine {
    /// One IOProc cycle.
    pub(crate) fn cycle(&mut self, mut c: Cycle<'_, '_>) {
        let t = c.time;
        for _ in 0..t.overloads {
            self.events.xrun();
        }
        let mut flags = gap_flags(t.gap) | self.latch.take();
        if t.estimated {
            self.shared.estimated.store(true, Ordering::Relaxed);
            if t.gap == Gap::First {
                flags |= BlockFlags::GAP_ESTIMATED;
            }
        }
        if self.capture.is_some() && self.render.is_some() {
            self.shared.out_minus_in.store(
                (t.output_sample_time - t.input_sample_time).to_bits(),
                Ordering::Release,
            );
        }
        if let Some(cap) = &mut self.capture {
            let input = &c.input;
            let map = &cap.map;
            cap.producer.push_with(
                BlockStamp {
                    start_sample: t.index,
                    frames: t.frames,
                    flags,
                    callback_ns: t.now_ns,
                    capture_ns: t.input_ns,
                },
                |f, ch| input.channel(map[ch]).get(f),
            );
        }
        if let Some(r) = &mut self.render {
            let channels = c.output.channels();
            let out = &mut c.output;
            r.renderer.render(
                OutputStamp {
                    start_sample: t.index,
                    frames: t.frames,
                    flags,
                    callback_ns: t.now_ns,
                    playback_ns: t.output_ns,
                },
                channels,
                |f, ch, v| out.channel_mut(ch).set(f, v),
            );
        }
    }
}
