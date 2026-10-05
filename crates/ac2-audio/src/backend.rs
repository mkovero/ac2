//! The backend trait, capability descriptions and the duplex request.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{AudioError, RequestError};
use crate::level::MaxLevel;
use crate::output::OutputSource;
use crate::stream::DuplexStream;

/// Which implementation a backend is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// The `jack` crate (Linux only: JACK2, or PipeWire through pipewire-jack).
    Jack,
    /// cpal on the OS host (Core Audio, WASAPI); macOS and Windows only.
    Cpal,
    /// The simulated device of [`crate::fake`].
    Fake,
    /// A recording played back by [`crate::replay`].
    Replay,
}

/// Capture or playback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Capture.
    Input,
    /// Playback.
    Output,
}

/// How capture and playback relate in time. Decides whether a generator→loopback offset
/// can be stable, and whether output and capture sample indices are the same counter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockRelation {
    /// One callback services both directions on one clock (JACK, ASIO, fake). Output and
    /// capture indices of one cycle are equal.
    SingleCallback,
    /// One physical device, separate callbacks (cpal on Core Audio). One
    /// clock, but the phase between directions is new on every start.
    SameDeviceSeparateCallbacks,
    /// Different devices, or identity cannot be proven (WASAPI endpoints differ by
    /// direction). May drift; drift detection must warn.
    Unknown,
}

/// How the capture sample index is obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexExactness {
    /// From an exact hardware or server frame counter; gap sizes are exact.
    Exact,
    /// A running frame count; gaps are estimated from host timestamps and flagged
    /// [`GAP_ESTIMATED`](crate::BlockFlags::GAP_ESTIMATED).
    Estimated,
}

/// An inclusive range of frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameRange {
    /// Smallest value.
    pub min: u32,
    /// Largest value.
    pub max: u32,
}

/// Frames per callback a backend asks for when the request names no buffer: 1024 at
/// 48 kHz (21 ms), scaled with the rate.
///
/// A host's own default can be far larger and is delivered in bursts: PipeWire's ALSA
/// plugin, left to choose, ran 65,536-frame periods, 1.37 s of audio arriving at once, and
/// every published frame outlived the one-second staleness bound between bursts. A period
/// this short keeps the latest frame well inside that bound and costs nothing in CPU.
pub const SHORT_BUFFER_AT_48K: u32 = 1024;

/// [`SHORT_BUFFER_AT_48K`] scaled to `rate`, clamped to what the device supports.
pub fn short_buffer_frames(rate: u32, supported: Option<FrameRange>) -> u32 {
    let want = (u64::from(rate) * u64::from(SHORT_BUFFER_AT_48K)).div_ceil(48_000);
    let want = u32::try_from(want).unwrap_or(u32::MAX).max(1);
    match supported {
        Some(r) => want.clamp(r.min.min(r.max), r.max.max(r.min)),
        None => want,
    }
}

/// An inclusive range of sample rates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateRange {
    /// Lowest rate, Hz.
    pub min: u32,
    /// Highest rate, Hz.
    pub max: u32,
}

impl RateRange {
    /// True when `rate` is inside the range.
    pub fn contains(&self, rate: u32) -> bool {
        (self.min..=self.max).contains(&rate)
    }
}

/// Latency a backend states without measuring. Plausibility data only: the loopback
/// correlation is the truth (decision 3a).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum StaticLatency {
    /// Port latency ranges of the physical ports used, in frames (JACK).
    PortRanges {
        /// Capture side.
        capture: FrameRange,
        /// Playback side.
        playback: FrameRange,
    },
    /// Only available per callback, as the capture / playback times in block headers and
    /// output ticks (cpal).
    PerCallbackTimestamps,
    /// The backend states nothing.
    Unknown,
}

/// Device sample formats this crate converts from and to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleFormat {
    /// 32-bit float.
    F32,
    /// 32-bit integer.
    I32,
    /// 24-bit integer.
    I24,
    /// 16-bit integer.
    I16,
}

/// What one direction of a device offers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectionCaps {
    /// Most channels any configuration offers.
    pub max_channels: u16,
    /// Supported rates.
    pub rates: Vec<RateRange>,
    /// Buffer sizes; `None` when the host cannot say before a stream exists.
    pub buffer_frames: Option<FrameRange>,
    /// Formats this crate can use, in preference order.
    pub formats: Vec<SampleFormat>,
    /// The host's default rate, if any.
    pub default_rate: Option<u32>,
    /// The callback size the device runs at unless asked otherwise, when the host states
    /// one.
    pub default_buffer: Option<u32>,
    /// One name per channel (`max_channels` of them) where the host names its channels
    /// (JACK ports); `None` where it does not (cpal).
    pub channel_names: Option<Vec<String>>,
}

/// Opaque device identifier, stable for as long as the host keeps it stable.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(pub String);

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Everything a backend states about one device, before opening it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCaps {
    /// Backend that lists it.
    pub backend: BackendKind,
    /// Host name (`jack`, `alsa`, `coreaudio`, `wasapi`, `fake`).
    pub host: String,
    /// Identifier for [`DeviceSelector::Id`].
    pub id: DeviceId,
    /// Human-readable name.
    pub name: String,
    /// Capture capabilities, if the device captures.
    pub input: Option<DirectionCaps>,
    /// Playback capabilities, if the device plays.
    pub output: Option<DirectionCaps>,
    /// Relation when this one device is used for both directions.
    pub duplex_clock: ClockRelation,
    /// How the capture index is obtained.
    pub index: IndexExactness,
    /// Stated latency.
    pub latency: StaticLatency,
    /// Problems that did not stop the device from being listed.
    pub notes: Vec<String>,
}

/// Which device to open for one direction.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceSelector {
    /// The host's default device.
    #[default]
    Default,
    /// A device by [`DeviceCaps::id`].
    Id(DeviceId),
}

impl fmt::Display for DeviceSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Default => f.write_str("default"),
            Self::Id(id) => write!(f, "{id}"),
        }
    }
}

/// Generator history to keep for the loopback timing monitor (Q3).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryRequest {
    /// Output channel to record.
    pub channel: u16,
    /// Seconds to keep (rounded up to a power-of-two frame count).
    pub seconds: f64,
}

impl HistoryRequest {
    /// Seconds kept by default: enough for a 2^15-sample correlation window plus a 1 s
    /// output→input search range, with margin for the consumer to run late.
    pub const DEFAULT_SECONDS: f64 = 4.0;

    /// Records `channel` for [`Self::DEFAULT_SECONDS`].
    pub fn channel(channel: u16) -> Self {
        Self {
            channel,
            seconds: Self::DEFAULT_SECONDS,
        }
    }
}

/// Everything needed to open a duplex stream.
#[derive(Debug)]
pub struct DuplexRequest {
    /// Capture device.
    pub input_device: DeviceSelector,
    /// Playback device. With [`DeviceSelector::Default`] backends prefer the capture device
    /// itself when it also plays, which keeps one clock where the host allows it.
    pub output_device: DeviceSelector,
    /// Zero-based device input channels; block channel `i` carries device channel
    /// `input_map[i]`.
    pub input_map: Vec<u16>,
    /// Output channels of the stream (channel `i` is device output `i`).
    pub output_channels: u16,
    /// Sample rate; `None` takes the device's default.
    pub sample_rate: Option<u32>,
    /// Callback size; `None` takes the device's default.
    pub buffer_frames: Option<u32>,
    /// Capture ring capacity in seconds.
    pub ring_seconds: f64,
    /// What the output emits. Silence unless a generator is given explicitly.
    pub output: OutputSource,
    /// Global maximum for every output sample.
    pub max_level: MaxLevel,
    /// Generator history for the loopback monitor.
    pub history: Option<HistoryRequest>,
}

impl DuplexRequest {
    /// Capture ring capacity used by [`DuplexRequest::new`].
    pub const DEFAULT_RING_SECONDS: f64 = 2.0;

    /// A request on default devices, silent output, default rate and buffer size.
    pub fn new(input_map: Vec<u16>, output_channels: u16, max_level: MaxLevel) -> Self {
        Self {
            input_device: DeviceSelector::Default,
            output_device: DeviceSelector::Default,
            input_map,
            output_channels,
            sample_rate: None,
            buffer_frames: None,
            ring_seconds: Self::DEFAULT_RING_SECONDS,
            output: OutputSource::Silence,
            max_level,
            history: None,
        }
    }

    /// Checks what can be checked without a device.
    pub fn validate(&self) -> Result<(), RequestError> {
        if self.input_map.is_empty() {
            return Err(RequestError::EmptyInputMap);
        }
        if !(self.ring_seconds.is_finite() && self.ring_seconds > 0.0) {
            return Err(RequestError::BadDuration {
                what: "ring_seconds",
            });
        }
        if self.sample_rate == Some(0) {
            return Err(RequestError::Zero {
                what: "sample_rate",
            });
        }
        if self.buffer_frames == Some(0) {
            return Err(RequestError::Zero {
                what: "buffer_frames",
            });
        }
        if let OutputSource::Generator(port) = &self.output
            && let Some(&channel) = port.routes().iter().find(|&&r| r >= self.output_channels)
        {
            return Err(RequestError::RouteOutOfRange {
                channel,
                output_channels: self.output_channels,
            });
        }
        if let Some(h) = &self.history {
            if h.channel >= self.output_channels {
                return Err(RequestError::HistoryChannelOutOfRange {
                    channel: h.channel,
                    output_channels: self.output_channels,
                });
            }
            if !(h.seconds.is_finite() && h.seconds > 0.0) {
                return Err(RequestError::BadDuration {
                    what: "history.seconds",
                });
            }
        }
        Ok(())
    }

    /// Capture ring capacity in frames at `rate`.
    pub(crate) fn ring_frames(&self, rate: u32) -> usize {
        (self.ring_seconds * f64::from(rate)).ceil() as usize
    }

    /// History channel and frame count at `rate`.
    pub(crate) fn history_frames(&self, rate: u32) -> Option<(u16, usize)> {
        self.history
            .map(|h| (h.channel, (h.seconds * f64::from(rate)).ceil() as usize))
    }
}

/// What was actually opened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Negotiated {
    /// Backend.
    pub backend: BackendKind,
    /// Capture device.
    pub input_device: DeviceId,
    /// Playback device.
    pub output_device: DeviceId,
    /// Stream rate, Hz.
    pub sample_rate: u32,
    /// Capture block channels (`input_map.len()`).
    pub input_channels: u16,
    /// Channels the capture device delivers.
    pub device_input_channels: u16,
    /// Output channels of the stream.
    pub output_channels: u16,
    /// Channels the playback device was opened with (≥ `output_channels`; the extra ones
    /// get zeros).
    pub device_output_channels: u16,
    /// Callback size when the host fixes one; `None` when it varies.
    pub buffer_frames: Option<u32>,
    /// Device format, capture side.
    pub input_format: SampleFormat,
    /// Device format, playback side; `None` when no output stream was opened.
    pub output_format: Option<SampleFormat>,
    /// Relation between the two directions as opened.
    pub clock: ClockRelation,
    /// How the capture index is obtained.
    pub index: IndexExactness,
    /// Stated latency.
    pub latency: StaticLatency,
}

/// An audio backend.
///
/// Deliberately small: the transport, clocks, event latches and the output path are shared
/// code that every implementation drives the same way. There are no default methods; each
/// backend states every capability explicitly.
pub trait Backend: Send + Sync + fmt::Debug {
    /// Which implementation this is.
    fn kind(&self) -> BackendKind;

    /// Lists devices and their capabilities. Read-only: opens no stream and emits nothing.
    fn enumerate(&self) -> Result<Vec<DeviceCaps>, AudioError>;

    /// Opens one duplex stream. Capture starts immediately; the output emits silence until
    /// a generator in the request is started.
    fn open(&self, request: DuplexRequest) -> Result<DuplexStream, AudioError>;
}

#[cfg(test)]
mod short_buffer_tests {
    use super::*;

    #[test]
    fn about_twenty_milliseconds_within_the_device_range() {
        assert_eq!(short_buffer_frames(48_000, None), 1024);
        assert_eq!(short_buffer_frames(96_000, None), 2048);
        assert_eq!(short_buffer_frames(44_100, None), 941);
        let r = |min, max| Some(FrameRange { min, max });
        assert_eq!(short_buffer_frames(48_000, r(64, 4096)), 1024);
        assert_eq!(short_buffer_frames(48_000, r(2048, 8192)), 2048);
        assert_eq!(short_buffer_frames(192_000, r(15, 512)), 512);
        for rate in [44_100, 48_000, 88_200, 96_000, 192_000] {
            let s = f64::from(short_buffer_frames(rate, None)) / f64::from(rate);
            assert!((0.020..0.022).contains(&s), "{rate}: {s}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::generator;

    fn level() -> MaxLevel {
        MaxLevel::from_peak_db(-20.0).expect("level")
    }

    #[test]
    fn request_validation() {
        assert_eq!(
            DuplexRequest::new(vec![], 2, level()).validate(),
            Err(RequestError::EmptyInputMap)
        );
        let mut r = DuplexRequest::new(vec![0], 2, level());
        assert_eq!(r.validate(), Ok(()));
        r.ring_seconds = f64::NAN;
        assert!(matches!(
            r.validate(),
            Err(RequestError::BadDuration { .. })
        ));
        r.ring_seconds = 1.0;
        r.history = Some(HistoryRequest::channel(2));
        assert!(matches!(
            r.validate(),
            Err(RequestError::HistoryChannelOutOfRange { channel: 2, .. })
        ));
        r.history = Some(HistoryRequest::channel(1));
        let (_h, port) = generator([0, 2]).expect("routes");
        r.output = OutputSource::Generator(port);
        assert!(matches!(
            r.validate(),
            Err(RequestError::RouteOutOfRange { channel: 2, .. })
        ));
        r.output = OutputSource::Silence;
        r.buffer_frames = Some(0);
        assert!(matches!(r.validate(), Err(RequestError::Zero { .. })));
    }
}
