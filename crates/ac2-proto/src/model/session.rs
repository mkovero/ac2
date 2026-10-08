//! Devices, backends and the audio session: selection, opening, stops, recovery, loopback detection (mirrors of ac2-audio).

use serde::{Deserialize, Serialize};

use super::ReplayInfo;
use crate::units::{Db, Dbfs, Samples, Seconds, SessionEpoch, WallNs};

/// Audio backend implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// JACK.
    Jack,
    /// cpal's OS host.
    Cpal,
    /// Simulated device.
    Fake,
    /// A raw capture file played back (`session.replay`); never listed by
    /// `session.devices`.
    Replay,
}

/// How capture and playback of one device relate in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockRelation {
    /// One callback, one clock.
    SingleCallback,
    /// One device, separate callbacks.
    SameDeviceSeparateCallbacks,
    /// Unknown; may drift.
    Unknown,
}

/// How the capture index is obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexExactness {
    /// Exact hardware/server counter.
    Exact,
    /// Running count; gaps estimated.
    Estimated,
}

/// Opaque device identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(pub String);

/// Inclusive range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RangeU32 {
    /// Smallest.
    pub min: u32,
    /// Largest.
    pub max: u32,
}

/// One direction of a device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectionInfo {
    /// Most channels.
    pub max_channels: u16,
    /// Supported sample-rate ranges, Hz.
    pub rates_hz: Vec<RangeU32>,
    /// Buffer sizes in frames, when the host can say.
    pub buffer_frames: Option<RangeU32>,
    /// Default rate, Hz.
    pub default_rate_hz: Option<u32>,
    /// Callback size the device runs at unless asked otherwise, when the host states one.
    pub default_buffer_frames: Option<u32>,
    /// One name per channel (`max_channels` of them) where the backend names its channels
    /// (JACK ports: alias or short name); `None` where it does not (cpal).
    pub channel_names: Option<Vec<String>>,
    /// The host's default device for this direction (where the system plays or records
    /// unless told otherwise).
    pub system_default: bool,
}

/// A device as listed by `session.devices`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceInfo {
    /// Backend.
    pub backend: BackendKind,
    /// Host name (`jack`, `alsa`, …).
    pub host: String,
    /// Identifier.
    pub id: DeviceId,
    /// Human-readable name.
    pub name: String,
    /// Capture side.
    pub input: Option<DirectionInfo>,
    /// Playback side.
    pub output: Option<DirectionInfo>,
    /// Duplex clock relation.
    pub duplex_clock: ClockRelation,
    /// Capture index exactness.
    pub index: IndexExactness,
    /// Non-fatal problems.
    pub notes: Vec<String>,
}

/// Whether a backend can be used now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Availability {
    /// Devices listed.
    Available,
    /// Nothing can be opened on it now.
    Unavailable {
        /// Why, for the operator (`JACK server not running`).
        reason: String,
    },
}

/// One backend the daemon offers, as listed by `session.devices`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendInfo {
    /// Backend.
    pub kind: BackendKind,
    /// What it is, for the operator.
    pub description: String,
    /// Usable now, or why not.
    pub availability: Availability,
    /// Its devices; empty while unavailable.
    pub devices: Vec<DeviceInfo>,
}

/// Which device to open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DeviceSelector {
    /// The host default.
    Default,
    /// A device by id.
    Id {
        /// Device.
        id: DeviceId,
    },
}

/// Generator output and its loopback: stimulus and reference leave the same converter and
/// the reference returns on an input, so all chain latency cancels (§4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopbackRoute {
    /// Output channel carrying the reference copy (zero-based).
    pub output: u16,
    /// Input channel it returns on (zero-based device channel).
    pub input: u16,
}

/// Arguments of `session.open`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionConfig {
    /// Backend the devices belong to; `None` = the daemon's default backend (the one it was
    /// started on).
    pub backend: Option<BackendKind>,
    /// Capture device.
    pub input_device: DeviceSelector,
    /// Playback device.
    pub output_device: DeviceSelector,
    /// Device input channels captured (zero-based).
    pub input_channels: Vec<u16>,
    /// Output channels of the stream.
    pub output_channels: u16,
    /// Requested sample rate; `None` = device default.
    pub sample_rate_hz: Option<u32>,
    /// Requested buffer size; `None` = device default.
    pub buffer_frames: Option<u32>,
    /// Reference loopback; `None` = no internal reference (decision 3b).
    pub loopback: Option<LoopbackRoute>,
}

/// An open session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenSession {
    /// Requested configuration.
    pub config: SessionConfig,
    /// Backend actually used.
    pub backend: BackendKind,
    /// Device actually opened for capture.
    pub input_device: DeviceId,
    /// Device actually opened for playback.
    pub output_device: DeviceId,
    /// Actual rate.
    pub sample_rate_hz: u32,
    /// Actual buffer size.
    pub buffer_frames: u32,
    /// Clock relation of the opened pair.
    pub clock: ClockRelation,
    /// When it opened.
    pub opened_at: WallNs,
    /// The recording it plays, for a replay session (`backend` is then `replay`, the
    /// devices are the ones that recorded it); `None` for a live device.
    pub replay: Option<ReplayInfo>,
}

/// Session entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    /// Current epoch.
    pub epoch: SessionEpoch,
    /// `None` while closed. Kept through an audio outage: the session stays open, with its
    /// configuration, until a client closes it.
    pub open: Option<OpenSession>,
    /// The open session's audio has stopped and the daemon is reopening it
    /// (`docs/design/audio-recovery.md`); `None` while audio runs or no session is open.
    pub stopped: Option<AudioStopped>,
}

/// An open session whose audio stopped: since when, why, and how reopening it goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioStopped {
    /// Wall time of the last audio received (the open, if none ever came).
    pub since: WallNs,
    /// Why the stream is considered stopped.
    pub cause: StopCause,
    /// Where reopening the same configuration stands.
    pub recovery: Recovery,
}

/// Why a session's audio stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum StopCause {
    /// No audio for `after` while the backend reported nothing (a hung audio server, a
    /// device reset under it).
    NotDelivering {
        /// The silence that counts as stopped for this stream.
        after_ms: u32,
    },
    /// The audio host ended the stream (its server shut down, the device was removed).
    HostEnded,
    /// The device or its configuration changed, and opening it again failed.
    DeviceChanged,
}

/// Reopening a stopped session's configuration, attempt by attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Recovery {
    /// Attempt `attempt` (1-based) is opening the stream since `started`; an audio server
    /// that hangs may keep it there.
    Opening {
        /// Attempt number.
        attempt: u32,
        /// When it began.
        started: WallNs,
    },
    /// Attempt `attempt` failed with `error`; the next begins at `next_at`.
    Waiting {
        /// Attempt number that failed.
        attempt: u32,
        /// What the backend said, which names what it waits for (a server to start, a
        /// device to return).
        error: String,
        /// When the next attempt begins.
        next_at: WallNs,
    },
}

/// Reply of `session.preview`: capture-only meters of a device before a session opens on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preview {
    /// Backend.
    pub backend: BackendKind,
    /// Device metered.
    pub device: DeviceId,
    /// Inputs metered: every input of the device, `0 .. channels`.
    pub channels: u16,
    /// Rate it runs at.
    pub sample_rate_hz: u32,
    /// The preview closes unless `session.preview` names this device again within this.
    pub expires_in_ms: u32,
}

/// How well one input matched the loopback-detection burst.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopbackCandidate {
    /// Zero-based device input.
    pub input: u16,
    /// Arrival after the burst left the output (exact on a single-callback clock; with
    /// separate callbacks it includes an offset common to every input).
    pub delay: Seconds,
    /// The same in samples.
    pub delay_samples: Samples,
    /// Normalised cross-correlation at that delay, −1 … 1 (negative: polarity inverted).
    pub correlation: f64,
    /// Level of the return relative to the burst; `None` for a silent input.
    pub gain: Option<Db>,
}

/// Reply of `session.detect_loopback`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopbackDetection {
    /// Backend.
    pub backend: BackendKind,
    /// Device the inputs were captured from.
    pub input_device: DeviceId,
    /// Device the burst played on.
    pub output_device: DeviceId,
    /// Output the burst played on (zero-based).
    pub output: u16,
    /// RMS level of the burst.
    pub level: Dbfs,
    /// Every input, best match first.
    pub ranked: Vec<LoopbackCandidate>,
    /// The best-ranked input when it correlates as a loopback cable does; `None` when no
    /// input does.
    pub loopback: Option<u16>,
    /// Clock relation of the stream the burst played on.
    pub clock: ClockRelation,
}
