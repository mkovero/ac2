//! Lock-free control of what the output callback emits.
//!
//! [`generator`] creates a pair: a [`GeneratorHandle`] kept by the control side, and a
//! [`GeneratorPort`] that goes into the [`DuplexRequest`](crate::DuplexRequest) as
//! [`OutputSource::Generator`](crate::OutputSource::Generator). Signal sources are handed to
//! the callback through a wait-free ring; replaced sources come back through a second ring
//! so that they are dropped (deallocated) on the control thread, never on the audio thread.
//! Run state and gain are atomics. The callback fades in, fades out and swaps sources itself,
//! so no control action can produce a hard cut.

use std::f64::consts::TAU;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use thiserror::Error;

use crate::level::Gain;
use crate::output::{OutputShared, OutputState};
use crate::rng::Rng;

/// A mono signal rendered on the audio thread.
///
/// Full scale is ±1.0. The output path applies fades, gain and the global
/// [`MaxLevel`](crate::MaxLevel) after this, and routes the result to the generator's
/// output channels.
pub trait SignalSource: Send {
    /// Writes the next `out.len()` samples.
    ///
    /// Runs on the audio callback: it must not allocate, lock, block or make syscalls, and
    /// its cost must be bounded by `out.len()`.
    fn fill(&mut self, out: &mut [f32]);
}

/// [`GeneratorHandle::set_source`] could not queue the source: two replacements are already
/// waiting for the callback. The source is handed back.
pub struct SourceQueueFull(pub Box<dyn SignalSource>);

impl fmt::Debug for SourceQueueFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SourceQueueFull(..)")
    }
}

impl fmt::Display for SourceQueueFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("generator source queue is full; the callback has not caught up")
    }
}

impl std::error::Error for SourceQueueFull {}

/// A generator routing that cannot work.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RouteError {
    /// The same output channel is listed twice.
    #[error("output channel {0} is routed twice")]
    Duplicate(u16),
}

/// Pending replacements the control side may queue before the callback takes them.
const SOURCE_QUEUE: usize = 2;
/// Replaced sources waiting to be dropped on the control side. Larger than the inbound
/// queue so the callback always has room to hand back what it replaces.
const RETIRED_QUEUE: usize = 8;

/// Creates a generator routed to `routes` (zero-based output channels). Every routed channel
/// carries the same signal: stimulus and reference leave through the same converter.
pub fn generator(
    routes: impl Into<Vec<u16>>,
) -> Result<(GeneratorHandle, GeneratorPort), RouteError> {
    let routes = routes.into();
    for (i, r) in routes.iter().enumerate() {
        if routes[..i].contains(r) {
            return Err(RouteError::Duplicate(*r));
        }
    }
    let (sp, sc) = rtrb::RingBuffer::new(SOURCE_QUEUE);
    let (rp, rc) = rtrb::RingBuffer::new(RETIRED_QUEUE);
    let shared = Arc::new(OutputShared::default());
    Ok((
        GeneratorHandle {
            sources: sp,
            retired: rc,
            shared: Arc::clone(&shared),
        },
        GeneratorPort {
            routes,
            sources: sc,
            retired: rp,
            shared,
        },
    ))
}

/// Control side of a generator.
pub struct GeneratorHandle {
    sources: rtrb::Producer<Box<dyn SignalSource>>,
    retired: rtrb::Consumer<Box<dyn SignalSource>>,
    shared: Arc<OutputShared>,
}

impl fmt::Debug for GeneratorHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GeneratorHandle")
            .field("state", &self.state())
            .finish_non_exhaustive()
    }
}

impl GeneratorHandle {
    /// Queues `source` to replace the current one. If the generator is audible, the callback
    /// fades out, swaps, and fades back in.
    pub fn set_source(&mut self, source: Box<dyn SignalSource>) -> Result<(), SourceQueueFull> {
        self.collect_retired();
        self.sources
            .push(source)
            .map_err(|rtrb::PushError::Full(s)| SourceQueueFull(s))
    }

    /// Requests output: fades in over [`FADE_SECONDS`](crate::FADE_SECONDS) once a source is
    /// set. Has no effect after the stream was stopped.
    pub fn start(&self) {
        self.shared.run.store(true, Ordering::Release);
    }

    /// Fades out over [`FADE_SECONDS`](crate::FADE_SECONDS), then emits silence.
    pub fn stop(&self) {
        self.shared.run.store(false, Ordering::Release);
    }

    /// Sets the gain; the callback ramps to it rather than stepping.
    pub fn set_gain(&self, gain: Gain) {
        self.shared
            .gain_bits
            .store(gain.to_bits(), Ordering::Release);
    }

    /// What the callback reported for its most recent block.
    pub fn state(&self) -> OutputState {
        self.shared.state()
    }

    /// Drops sources the callback has replaced; returns how many. Called by
    /// [`set_source`](Self::set_source) too.
    pub fn collect_retired(&mut self) -> usize {
        let mut n = 0;
        while self.retired.pop().is_ok() {
            n += 1;
        }
        n
    }
}

/// Audio side of a generator; moved into the backend through the request.
pub struct GeneratorPort {
    pub(crate) routes: Vec<u16>,
    pub(crate) sources: rtrb::Consumer<Box<dyn SignalSource>>,
    pub(crate) retired: rtrb::Producer<Box<dyn SignalSource>>,
    pub(crate) shared: Arc<OutputShared>,
}

impl fmt::Debug for GeneratorPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GeneratorPort")
            .field("routes", &self.routes)
            .finish_non_exhaustive()
    }
}

impl GeneratorPort {
    /// Output channels that carry the generator.
    pub fn routes(&self) -> &[u16] {
        &self.routes
    }
}

/// A sine at a fixed frequency and peak amplitude. Mainly for tests and checks.
#[derive(Debug, Clone)]
pub struct Sine {
    phase: f64,
    step: f64,
    peak: f64,
}

impl Sine {
    /// Sine of `freq_hz` at `sample_rate`, peak amplitude `peak` (full scale = 1.0).
    pub fn new(freq_hz: f64, sample_rate: u32, peak: f64) -> Self {
        Self {
            phase: 0.0,
            step: TAU * freq_hz / f64::from(sample_rate.max(1)),
            peak,
        }
    }
}

impl SignalSource for Sine {
    fn fill(&mut self, out: &mut [f32]) {
        for v in out {
            *v = (self.peak * self.phase.sin()) as f32;
            self.phase += self.step;
            if self.phase >= TAU {
                self.phase -= TAU;
            }
        }
    }
}

/// Seeded Gaussian white noise at a given RMS. Mainly for tests and checks: broadband, so
/// a loopback correlation has one sharp peak.
#[derive(Debug, Clone)]
pub struct WhiteNoise {
    rng: Rng,
    rms: f64,
}

impl WhiteNoise {
    /// White noise with RMS `rms` (full scale = 1.0) from `seed`.
    pub fn new(rms: f64, seed: u64) -> Self {
        Self {
            rng: Rng::new(seed),
            rms,
        }
    }
}

impl SignalSource for WhiteNoise {
    fn fill(&mut self, out: &mut [f32]) {
        for v in out {
            *v = (self.rms * self.rng.gaussian()) as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_routes_are_rejected() {
        assert_eq!(generator([0, 1, 0]).err(), Some(RouteError::Duplicate(0)));
        assert!(generator([0, 1]).is_ok());
    }

    #[test]
    fn source_queue_is_bounded_and_hands_back() {
        let (mut h, _port) = generator([0]).expect("routes");
        for _ in 0..SOURCE_QUEUE {
            h.set_source(Box::new(Sine::new(1000.0, 48_000, 0.1)))
                .expect("room");
        }
        assert!(h.set_source(Box::new(Sine::new(1.0, 48_000, 0.1))).is_err());
    }
}
