//! Audio backend trait with explicit capabilities, and sample-indexed multichannel blocks.
//! Backends: JACK (feature `jack`), cpal (the OS's default host) and a simulated device.
//!
//! # Shape
//!
//! A [`Backend`] lists devices ([`DeviceCaps`]) and opens one [`DuplexStream`] from a
//! [`DuplexRequest`]. Everything after that is shared, backend-independent code:
//!
//! - **Capture.** Each callback becomes one block of interleaved `f32` samples with a
//!   [`BlockHeader`] `{start_sample, frames, channels, flags, callback_ns, capture_ns}`,
//!   carried through two wait-free SPSC rings (samples, then header), whole block or
//!   nothing. Lost blocks, xruns, index gaps and configuration changes travel in-band as
//!   [`BlockFlags`]; consumers reset on [`BlockFlags::BREAKS_CONTINUITY`].
//! - **Output.** Silence unless a [`generator()`] is supplied. The output path fades every
//!   change of audibility over [`FADE_SECONDS`], enforces the request's [`MaxLevel`] on
//!   every sample, records one [`OutputTick`] per callback and keeps a generator history
//!   ([`HistoryReader`]) for the loopback timing monitor.
//! - **Stop.** [`DuplexStream::stop`] (and drop) fades out, waits for silence, then tears
//!   the backend down.
//!
//! # Real-time rules
//!
//! Code that runs in a callback never allocates, locks, blocks or makes syscalls: all
//! buffers are allocated at open, notifications cross threads as atomics, and replaced
//! generator sources are handed back to the control thread to be dropped there.
//!
//! # Audio safety
//!
//! Nothing but zeros is written to an output unless the caller builds a generator, gives it
//! a source and starts it. Automated tests use [`fake`] or a JACK dummy server, never real
//! hardware.

pub mod backend;
pub mod block;
pub mod clock;
pub mod error;
pub mod events;
pub mod fake;
pub mod generator;
pub mod history;
pub mod level;
pub mod output;
pub mod stream;

mod cpal_host;
#[cfg(all(feature = "jack", target_os = "linux"))]
mod jack_host;
mod rng;

pub use backend::{
    Backend, BackendKind, ClockRelation, DeviceCaps, DeviceId, DeviceSelector, Direction,
    DirectionCaps, DuplexRequest, FrameRange, HistoryRequest, IndexExactness, Negotiated,
    RateRange, SampleFormat, StaticLatency,
};
pub use block::{BlockConsumer, BlockFlags, BlockHeader, TransportStats};
pub use cpal_host::CpalBackend;
pub use error::{AudioError, Operation, RequestError, Unsupported};
pub use events::EventSnapshot;
pub use fake::{FakeBackend, FakeConfig, FakeDriver};
pub use generator::{GeneratorHandle, GeneratorPort, SignalSource, generator};
pub use history::{HistoryError, HistoryReader};
#[cfg(all(feature = "jack", target_os = "linux"))]
pub use jack_host::{JackBackend, JackConfig};
pub use level::{Gain, LevelError, MaxLevel};
pub use output::{FADE_SECONDS, OutputSource, OutputState, OutputStats, OutputTick};
pub use stream::{DuplexStream, StopOutcome};
