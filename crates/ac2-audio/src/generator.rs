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

use crate::level::{Gain, MaxLevel};
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
    /// The channel is beyond the largest channel a generator can carry.
    #[error("output channel {0} is above the routable maximum {max}", max = MAX_ROUTED_CHANNELS - 1)]
    OutOfRange(u16),
    /// Earlier routing changes are still waiting for the callback.
    #[error("generator routing queue is full; the callback has not caught up")]
    QueueFull,
}

/// Output channels a generator can be routed to (zero-based channels below this).
pub const MAX_ROUTED_CHANNELS: u16 = 256;

/// The set of output channels carrying the generator: a fixed-size bit set, so a routing
/// change crosses to the callback without allocation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RouteSet([u64; (MAX_ROUTED_CHANNELS as usize) / 64]);

impl RouteSet {
    /// Checks `routes` (no duplicates, all routable) and builds the set.
    pub(crate) fn new(routes: &[u16]) -> Result<Self, RouteError> {
        let mut set = Self::default();
        for &r in routes {
            if r >= MAX_ROUTED_CHANNELS {
                return Err(RouteError::OutOfRange(r));
            }
            if set.contains(usize::from(r)) {
                return Err(RouteError::Duplicate(r));
            }
            set.0[usize::from(r) / 64] |= 1 << (r % 64);
        }
        Ok(set)
    }

    pub(crate) fn contains(&self, channel: usize) -> bool {
        self.0
            .get(channel / 64)
            .is_some_and(|w| w & (1 << (channel % 64)) != 0)
    }
}

/// Pending replacements the control side may queue before the callback takes them.
const SOURCE_QUEUE: usize = 2;
/// Replaced sources waiting to be dropped on the control side. Larger than the inbound
/// queue so the callback always has room to hand back what it replaces.
const RETIRED_QUEUE: usize = 8;
/// Pending routing changes the control side may queue before the callback takes them.
const ROUTE_QUEUE: usize = 4;

/// Creates a generator routed to `routes` (zero-based output channels). Every routed channel
/// carries the same signal: stimulus and reference leave through the same converter. The
/// routing can change later without reopening the stream
/// ([`GeneratorHandle::set_routes`]).
pub fn generator(
    routes: impl Into<Vec<u16>>,
) -> Result<(GeneratorHandle, GeneratorPort), RouteError> {
    let routes = routes.into();
    let set = RouteSet::new(&routes)?;
    let (sp, sc) = rtrb::RingBuffer::new(SOURCE_QUEUE);
    let (rp, rc) = rtrb::RingBuffer::new(RETIRED_QUEUE);
    let (tp, tc) = rtrb::RingBuffer::new(ROUTE_QUEUE);
    let shared = Arc::new(OutputShared::default());
    Ok((
        GeneratorHandle {
            sources: sp,
            retired: rc,
            route_changes: tp,
            routes: routes.clone(),
            shared: Arc::clone(&shared),
        },
        GeneratorPort {
            routes,
            set,
            sources: sc,
            retired: rp,
            route_changes: tc,
            shared,
        },
    ))
}

/// Control side of a generator.
pub struct GeneratorHandle {
    sources: rtrb::Producer<Box<dyn SignalSource>>,
    retired: rtrb::Consumer<Box<dyn SignalSource>>,
    route_changes: rtrb::Producer<RouteSet>,
    routes: Vec<u16>,
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

    /// Routes the generator to `routes` (zero-based output channels) from now on. If the
    /// generator is audible, the callback fades out, switches the routing, and fades back
    /// in, so no channel starts or stops with a hard cut. Channels the stream does not
    /// have are ignored by the callback; the caller checks them against the stream.
    pub fn set_routes(&mut self, routes: &[u16]) -> Result<(), RouteError> {
        let set = RouteSet::new(routes)?;
        self.route_changes
            .push(set)
            .map_err(|_| RouteError::QueueFull)?;
        self.routes = routes.to_vec();
        Ok(())
    }

    /// The routing last set ([`generator`] or [`set_routes`](Self::set_routes)).
    pub fn routes(&self) -> &[u16] {
        &self.routes
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

    /// Sets the sample-peak limit from the next block on. The callback enforces the lower of
    /// this and the limit the stream was opened with ([`crate::DuplexRequest::max_level`]):
    /// a running stream's limit can be tightened at once, and never raised above the one it
    /// was opened with.
    pub fn set_max_level(&self, max: MaxLevel) {
        self.shared
            .limit_bits
            .store(max.linear().to_bits(), Ordering::Release);
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
    pub(crate) set: RouteSet,
    pub(crate) sources: rtrb::Consumer<Box<dyn SignalSource>>,
    pub(crate) retired: rtrb::Producer<Box<dyn SignalSource>>,
    pub(crate) route_changes: rtrb::Consumer<RouteSet>,
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
    /// Output channels that carry the generator when the stream opens.
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
        assert_eq!(
            generator([MAX_ROUTED_CHANNELS]).err(),
            Some(RouteError::OutOfRange(MAX_ROUTED_CHANNELS))
        );
        let (mut h, _port) = generator([0, 1]).expect("routes");
        assert_eq!(h.set_routes(&[3, 3]), Err(RouteError::Duplicate(3)));
        assert_eq!(h.routes(), [0, 1], "a refused routing changes nothing");
        h.set_routes(&[2, 200]).expect("routes");
        assert_eq!(h.routes(), [2, 200]);
        let set = RouteSet::new(&[2, 200]).expect("set");
        assert!(set.contains(2) && set.contains(200) && !set.contains(0));
        assert!(!set.contains(10_000));
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
