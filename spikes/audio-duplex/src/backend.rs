//! The small backend trait under evaluation, plus capability and stream types.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::block::{BlockConsumer, BlockFlags};
use crate::output::{OutputControl, OutputMode, OutputTick};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    Fake,
    Cpal,
    Jack,
}

/// How capture and playback relate in time. Decides whether a loopback offset can be stable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockRelation {
    /// One callback services both directions on one clock (JACK, ASIO, fake).
    SingleCallback,
    /// Same physical device, separate callbacks/threads (cpal CoreAudio/ALSA same PCM).
    /// Same clock, but callback phase between directions is not fixed by the API.
    SameDeviceSeparateCallbacks,
    /// Different devices, or identity cannot be proven (WASAPI endpoints differ by
    /// direction). May drift; drift detection must warn.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RateRange {
    pub min: u32,
    pub max: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BufferRange {
    pub min: u32,
    pub max: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DirectionCaps {
    pub max_channels: u16,
    pub rates: Vec<RateRange>,
    /// `None` = the host cannot tell before a stream exists.
    pub buffer_frames: Option<BufferRange>,
    pub sample_formats: Vec<String>,
    pub default_rate: Option<u32>,
    pub default_channels: Option<u16>,
}

/// Latency the backend states without measuring. Plausibility only (Q3); the loopback wins.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum StaticLatency {
    /// JACK port latency ranges of the physical ports, in frames.
    PortRanges {
        capture: (u32, u32),
        playback: (u32, u32),
    },
    /// Only available per callback as timestamps (cpal `capture`/`playback` instants).
    PerCallbackTimestamps,
    None,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeviceCaps {
    pub backend: BackendKind,
    pub host: String,
    pub id: String,
    pub name: String,
    pub input: Option<DirectionCaps>,
    pub output: Option<DirectionCaps>,
    /// Relation if this one device is used for both directions.
    pub duplex_clock: ClockRelation,
    pub latency: StaticLatency,
    /// Enumeration problems that did not stop listing the device.
    pub notes: Vec<String>,
}

/// Which device channels feed the capture block, in block order.
#[derive(Clone, Debug, PartialEq)]
pub struct DuplexRequest {
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    /// Zero-based device input channels; block channel `i` = device channel `input_map[i]`.
    pub input_map: Vec<u16>,
    pub output_channels: u16,
    pub sample_rate: Option<u32>,
    pub buffer_frames: Option<u32>,
    /// Capture ring capacity.
    pub ring_seconds: f64,
    pub output: OutputMode,
    /// JACK: connect our input ports to physical capture ports (and outputs to physical
    /// playback ports only when this is `Both`).
    pub connect: ConnectPolicy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectPolicy {
    None,
    InputsOnly,
    Both,
}

impl Default for DuplexRequest {
    fn default() -> Self {
        Self {
            input_device: None,
            output_device: None,
            input_map: vec![0, 1],
            output_channels: 2,
            sample_rate: None,
            buffer_frames: None,
            ring_seconds: 2.0,
            output: OutputMode::Silence,
            connect: ConnectPolicy::InputsOnly,
        }
    }
}

/// What was actually opened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Negotiated {
    pub backend: BackendKind,
    pub input_device: String,
    pub output_device: String,
    pub sample_rate: u32,
    pub input_channels: u16,
    pub device_input_channels: u16,
    pub output_channels: u16,
    pub buffer_frames: Option<u32>,
    pub input_format: String,
    pub output_format: String,
    pub clock: ClockRelation,
    pub latency: StaticLatency,
}

/// Counters the backend's non-data callbacks (error/xrun notifications) bump.
#[derive(Debug, Default)]
pub struct BackendEvents {
    pub xruns: AtomicU64,
    pub errors: AtomicU64,
    pub config_changes: AtomicU64,
    pub input_callbacks: AtomicU64,
}

impl BackendEvents {
    pub fn snapshot(&self) -> EventSnapshot {
        EventSnapshot {
            xruns: self.xruns.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            config_changes: self.config_changes.load(Ordering::Relaxed),
            input_callbacks: self.input_callbacks.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct EventSnapshot {
    pub xruns: u64,
    pub errors: u64,
    pub config_changes: u64,
    pub input_callbacks: u64,
}

/// Lets the input callback turn asynchronous notifications into flags on its next block,
/// without locks: it remembers the counter values it last saw.
#[derive(Debug)]
pub struct EventLatch {
    events: Arc<BackendEvents>,
    seen_xruns: u64,
    seen_config: u64,
}

impl EventLatch {
    pub fn new(events: Arc<BackendEvents>) -> Self {
        Self {
            events,
            seen_xruns: 0,
            seen_config: 0,
        }
    }

    #[inline]
    pub fn take_flags(&mut self) -> BlockFlags {
        let mut f = BlockFlags::NONE;
        let x = self.events.xruns.load(Ordering::Relaxed);
        if x != self.seen_xruns {
            self.seen_xruns = x;
            f |= BlockFlags::XRUN;
        }
        let c = self.events.config_changes.load(Ordering::Relaxed);
        if c != self.seen_config {
            self.seen_config = c;
            f |= BlockFlags::CONFIG_CHANGE;
        }
        self.events.input_callbacks.fetch_add(1, Ordering::Relaxed);
        f
    }
}

#[derive(Debug)]
pub enum AudioError {
    NoDevice(String),
    Unsupported(String),
    Backend(String),
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDevice(s) => write!(f, "no device: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported: {s}"),
            Self::Backend(s) => write!(f, "backend error: {s}"),
        }
    }
}

impl std::error::Error for AudioError {}

/// Keeps the backend's stream objects alive; dropping it stops the callbacks.
pub trait StreamGuard: Send {}
impl<T: Send> StreamGuard for T {}

/// An open duplex stream.
pub struct DuplexStream {
    pub negotiated: Negotiated,
    pub capture: BlockConsumer,
    pub output_ticks: rtrb::Consumer<OutputTick>,
    pub output: Arc<OutputControl>,
    pub events: Arc<BackendEvents>,
    emits: bool,
    guard: Option<Box<dyn StreamGuard>>,
}

impl std::fmt::Debug for DuplexStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DuplexStream")
            .field("negotiated", &self.negotiated)
            .finish_non_exhaustive()
    }
}

impl DuplexStream {
    pub fn new(
        negotiated: Negotiated,
        capture: BlockConsumer,
        output_ticks: rtrb::Consumer<OutputTick>,
        output: Arc<OutputControl>,
        events: Arc<BackendEvents>,
        output_mode: OutputMode,
        guard: Box<dyn StreamGuard>,
    ) -> Self {
        Self {
            negotiated,
            capture,
            output_ticks,
            output,
            events,
            emits: !matches!(output_mode, OutputMode::Silence),
            guard: Some(guard),
        }
    }

    /// Fade out any emitted signal, then drop the backend stream.
    pub fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        if self.guard.is_none() {
            return;
        }
        self.output.request_stop();
        // Fade is 20 ms; allow several callback periods for it to be rendered and played.
        let deadline = Instant::now() + Duration::from_millis(500);
        while !self.output.is_silent() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if !self.output.is_silent() {
            // Output callback stalled; nothing more can be done than stopping the stream.
            eprintln!("warning: output did not report silence before stop");
        } else if self.emits {
            // Let the faded-out tail drain through the device buffer before tearing down.
            std::thread::sleep(Duration::from_millis(100));
        }
        self.guard = None;
    }
}

impl Drop for DuplexStream {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

/// The trait shape under evaluation.
pub trait AudioBackend {
    fn kind(&self) -> BackendKind;
    fn enumerate(&self) -> Result<Vec<DeviceCaps>, AudioError>;
    fn open_duplex(&self, req: &DuplexRequest) -> Result<DuplexStream, AudioError>;
}
