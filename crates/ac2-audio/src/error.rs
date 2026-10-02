//! Error types.

use thiserror::Error;

use crate::backend::{BackendKind, Direction};

/// Anything a backend can refuse or fail at.
#[derive(Debug, Error)]
pub enum AudioError {
    /// The backend cannot be used at all here (no JACK server or library, no audio host).
    #[error("{backend:?} backend unavailable: {reason}")]
    Unavailable {
        /// Which backend.
        backend: BackendKind,
        /// Why, and what to do about it.
        reason: Unavailability,
    },
    /// No device matches the selector.
    #[error("no {direction:?} device {selector}")]
    DeviceNotFound {
        /// Direction that was searched.
        direction: Direction,
        /// What was asked for (device id or "default").
        selector: String,
    },
    /// The device exists but cannot do what was asked.
    #[error("unsupported: {0}")]
    Unsupported(#[from] Unsupported),
    /// The request is inconsistent in itself.
    #[error("invalid request: {0}")]
    InvalidRequest(#[from] RequestError),
    /// The host reported an error.
    #[error("{backend:?} backend failed to {operation:?}: {detail}")]
    Backend {
        /// Which backend.
        backend: BackendKind,
        /// What was being done.
        operation: Operation,
        /// Host-supplied detail.
        detail: String,
    },
}

/// Why a backend cannot be used at all, in the operator's words, each with its remedy.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Unavailability {
    /// PipeWire serves audio, but the libjack that was loaded is JACK2's, which finds no
    /// server: PipeWire's own libjack (pipewire-jack) is what talks to it.
    #[error(
        "PipeWire is running but its JACK library isn't in use: install pipewire-jack \
         (e.g. `sudo apt install pipewire-jack`, `sudo pacman -S pipewire-jack`) or start the \
         daemon with `pw-jack ac2d`"
    )]
    PipeWireWithoutJack,
    /// Neither a JACK server nor PipeWire.
    #[error("No JACK server: start JACK (e.g. `jackd -d alsa`) or use PipeWire")]
    NoJackServer,
    /// No libjack could be loaded at all.
    #[error(
        "No JACK library (libjack) is installed: install pipewire-jack where PipeWire runs \
         (e.g. `sudo apt install pipewire-jack`), else JACK2 (e.g. `sudo apt install jackd2`)"
    )]
    NoJackLibrary,
    /// Anything else the host reported.
    #[error("{0}")]
    Host(String),
}

/// What a backend was doing when it failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// Listing devices or capabilities.
    Enumerate,
    /// Creating streams, clients or ports.
    Open,
    /// Starting streams or activating a client.
    Start,
    /// Connecting ports.
    Connect,
}

/// A capability the device does not have.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Unsupported {
    /// The rate is not available (JACK and the fake device run at one fixed rate).
    #[error("sample rate {requested} Hz (device offers {offered})")]
    SampleRate {
        /// Requested rate.
        requested: u32,
        /// Human-readable list of what the device offers.
        offered: String,
    },
    /// The buffer size is not available.
    #[error("buffer of {requested} frames (fixed at {fixed:?})")]
    BufferFrames {
        /// Requested size.
        requested: u32,
        /// The only size the backend runs at, if it is fixed.
        fixed: Option<u32>,
    },
    /// An input channel index beyond the device.
    #[error("input channel {channel} (device has {available})")]
    InputChannel {
        /// Zero-based device channel asked for.
        channel: u16,
        /// Channels the device has.
        available: u16,
    },
    /// More output channels than the device has.
    #[error("{requested} output channels (device has {available})")]
    OutputChannels {
        /// Requested count.
        requested: u16,
        /// Channels the device has.
        available: u16,
    },
    /// More ports than the backend's fixed per-callback limit.
    #[error("{requested} channels in one direction (limit {max})")]
    TooManyChannels {
        /// Requested count.
        requested: usize,
        /// Backend limit.
        max: usize,
    },
    /// No sample format this crate converts is offered in this direction.
    #[error("no supported sample format for {direction:?} at the requested rate and channels")]
    SampleFormat {
        /// Direction that failed.
        direction: Direction,
    },
}

/// A request that cannot be served by any backend.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RequestError {
    /// No capture channels requested; ac2 always measures.
    #[error("input map is empty")]
    EmptyInputMap,
    /// A generator route points past the stream's output channels.
    #[error("generator routed to output {channel}, stream has {output_channels}")]
    RouteOutOfRange {
        /// Routed channel.
        channel: u16,
        /// Output channels requested.
        output_channels: u16,
    },
    /// The history channel is not one of the stream's output channels.
    #[error("history channel {channel}, stream has {output_channels} outputs")]
    HistoryChannelOutOfRange {
        /// Requested channel.
        channel: u16,
        /// Output channels requested.
        output_channels: u16,
    },
    /// A duration that must be positive and finite is not (ring or history length).
    #[error("{what} must be a positive, finite number of seconds")]
    BadDuration {
        /// Which field.
        what: &'static str,
    },
    /// A sample rate or buffer size of zero.
    #[error("{what} must be non-zero")]
    Zero {
        /// Which field.
        what: &'static str,
    },
}
