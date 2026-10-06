//! An open duplex stream: the consumer ends of the transport plus lifetime control.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::backend::{DuplexRequest, Negotiated};
use crate::block::{BlockConsumer, BlockProducer, TransportStats, transport};
use crate::error::AudioError;
use crate::events::{BackendEvents, EventSnapshot};
use crate::history::HistoryReader;
use crate::output::{
    OutputRenderer, OutputShared, OutputState, OutputStats, OutputTick, RendererConfig,
    RendererParts,
};

/// Keeps a backend's stream objects alive; dropping it stops the callbacks.
pub(crate) trait StreamGuard: Send {}
impl<T: Send> StreamGuard for T {}

/// The backend-independent plumbing of one stream: capture transport, output renderer and
/// event counters. Every backend builds it the same way, then moves the callback halves
/// into its callbacks.
pub(crate) struct Plumbing {
    pub(crate) producer: BlockProducer,
    pub(crate) consumer: BlockConsumer,
    pub(crate) renderer: OutputRenderer,
    pub(crate) output: RendererParts,
    pub(crate) events: Arc<BackendEvents>,
}

impl Plumbing {
    /// Builds the plumbing for `req` at `rate`. `min_block_frames` is the smallest callback
    /// the backend can deliver (sizes the header ring). Takes the output source out of the
    /// request.
    pub(crate) fn new(req: &mut DuplexRequest, rate: u32, min_block_frames: usize) -> Self {
        let channels = req.input_map.len() as u16;
        let (producer, consumer) = transport(channels, req.ring_frames(rate), min_block_frames);
        let (renderer, output) = OutputRenderer::new(RendererConfig {
            source: std::mem::take(&mut req.output),
            output_channels: req.output_channels,
            sample_rate: rate,
            max_level: req.max_level,
            history: req.history_frames(rate),
        });
        Self {
            producer,
            consumer,
            renderer,
            output,
            events: Arc::new(BackendEvents::default()),
        }
    }
}

/// How [`DuplexStream::stop`] ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopOutcome {
    /// Nothing but zeros was ever written; torn down at once.
    NeverEmitted,
    /// The fade-out completed and the faded tail had time to leave the device buffers.
    FadedOut,
    /// The output callback did not report silence in time (stalled or disconnected
    /// device). The stream was torn down anyway; nothing better is possible.
    TimedOut,
}

/// One connection an [`OutputPatch`] holds: a stream output and the physical port it feeds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchLink {
    /// Stream output channel, zero-based.
    pub output: u16,
    /// The stream's port (`ac2:out_1`).
    pub from: String,
    /// The physical playback port it is connected to (`system:playback_1`).
    pub to: String,
}

/// What [`OutputPatch::connect_only`] left in place.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchState {
    /// Connections the patch holds now.
    pub links: Vec<PatchLink>,
    /// Outputs asked for that have no physical playback port of the same number.
    pub unmatched: Vec<u16>,
}

/// Connections from a stream's outputs to the device's physical outputs, on a backend whose
/// stream outputs are ports in a graph (JACK) rather than the device's channels themselves.
///
/// Stream output `k` goes to the `k`-th physical playback port, in the order the device
/// listing names them. Only connections the patch made itself are ever removed; a
/// connection someone else made is left alone.
pub trait OutputPatch: Send + Sync {
    /// Connects exactly the outputs in `outputs` (zero-based) to their physical playback
    /// ports, and removes the patch's own connections of every other output.
    fn connect_only(&self, outputs: &[u16]) -> Result<PatchState, AudioError>;
}

/// Pieces a backend hands to [`DuplexStream::new`].
pub(crate) struct StreamParts {
    pub(crate) negotiated: Negotiated,
    pub(crate) capture: BlockConsumer,
    pub(crate) output: RendererParts,
    pub(crate) events: Arc<BackendEvents>,
    /// Time for a rendered block to leave the device after the callback produced it.
    pub(crate) drain: Duration,
    pub(crate) guard: Box<dyn StreamGuard>,
    /// How the stream's outputs reach the device, when that takes graph connections.
    pub(crate) patch: Option<Arc<dyn OutputPatch>>,
}

/// An open duplex stream.
///
/// Concrete rather than a trait object: every backend produces the same consumer side.
/// Dropping it performs [`stop`](Self::stop) with [`DuplexStream::DEFAULT_STOP_TIMEOUT`].
pub struct DuplexStream {
    negotiated: Negotiated,
    capture: BlockConsumer,
    ticks: rtrb::Consumer<OutputTick>,
    history: Option<HistoryReader>,
    output: Arc<OutputShared>,
    events: Arc<BackendEvents>,
    drain: Duration,
    guard: Option<Box<dyn StreamGuard>>,
    patch: Option<Arc<dyn OutputPatch>>,
}

impl fmt::Debug for DuplexStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DuplexStream")
            .field("negotiated", &self.negotiated)
            .field("output", &self.output.state())
            .finish_non_exhaustive()
    }
}

impl DuplexStream {
    /// Stop timeout used on drop: the fade is 20 ms, this leaves room for several slow
    /// callback periods.
    pub const DEFAULT_STOP_TIMEOUT: Duration = Duration::from_millis(500);

    pub(crate) fn new(parts: StreamParts) -> Self {
        Self {
            negotiated: parts.negotiated,
            capture: parts.capture,
            ticks: parts.output.ticks,
            history: parts.output.history,
            output: parts.output.shared,
            events: parts.events,
            drain: parts.drain,
            guard: Some(parts.guard),
            patch: parts.patch,
        }
    }

    /// Replaces the stream guard (a backend that starts its driver after building the
    /// stream).
    pub(crate) fn set_guard(&mut self, guard: Box<dyn StreamGuard>, drain: Duration) {
        self.guard = Some(guard);
        self.drain = drain;
    }

    /// Records what paces the blocks (a backend that starts its driver after building the
    /// stream).
    pub(crate) fn set_delivery(&mut self, delivery: crate::backend::Delivery) {
        self.negotiated.delivery = delivery;
    }

    /// What was opened.
    pub fn negotiated(&self) -> &Negotiated {
        &self.negotiated
    }

    /// The connections from this stream's outputs to the device's physical outputs, on a
    /// backend where those are graph connections (JACK). `None` where the stream's outputs
    /// are the device's channels themselves (cpal, the fake device). The patch outlives
    /// the stream harmlessly: once the stream is stopped, it refuses to connect.
    pub fn output_patch(&self) -> Option<Arc<dyn OutputPatch>> {
        self.patch.clone()
    }

    /// Capture blocks.
    pub fn capture(&mut self) -> &mut BlockConsumer {
        &mut self.capture
    }

    /// Next output timing record, oldest first.
    pub fn pop_output_tick(&mut self) -> Option<OutputTick> {
        self.ticks.pop().ok()
    }

    /// Generator history, if requested.
    pub fn history(&self) -> Option<&HistoryReader> {
        self.history.as_ref()
    }

    /// Host notifications so far.
    pub fn events(&self) -> EventSnapshot {
        self.events.snapshot()
    }

    /// Capture transport counters.
    pub fn transport_stats(&self) -> TransportStats {
        self.capture.stats()
    }

    /// Output counters and audibility.
    pub fn output_stats(&self) -> OutputStats {
        self.output.stats()
    }

    /// Starts the final fade-out without waiting. Irreversible: the generator cannot be
    /// restarted on this stream.
    pub fn begin_stop(&self) {
        self.output.begin_shutdown();
    }

    /// Fades out (decision 6b), waits up to `timeout` for the callback to report silence,
    /// lets the faded tail leave the device, then stops the backend.
    pub fn stop(mut self, timeout: Duration) -> StopOutcome {
        self.stop_inner(timeout)
    }

    fn stop_inner(&mut self, timeout: Duration) -> StopOutcome {
        self.output.begin_shutdown();
        let outcome = if !self.output.has_emitted() {
            StopOutcome::NeverEmitted
        } else {
            let deadline = Instant::now() + timeout;
            while self.output.state() != OutputState::Silent && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(2));
            }
            if self.output.state() == OutputState::Silent {
                std::thread::sleep(self.drain);
                StopOutcome::FadedOut
            } else {
                StopOutcome::TimedOut
            }
        };
        self.guard = None;
        outcome
    }
}

impl Drop for DuplexStream {
    fn drop(&mut self) {
        if self.guard.is_some() {
            self.stop_inner(Self::DEFAULT_STOP_TIMEOUT);
        }
    }
}
