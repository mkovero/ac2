//! A simulated duplex device for hardware-free tests.
//!
//! The fake runs the same plumbing as the real backends (transport, frame-counter clock,
//! event latch, output renderer, generator history); only the "device" is simulated. One
//! simulated callback renders the output block, hands it to a simulated DAC, simulates the
//! analog world for one block, and captures it through a simulated ADC:
//!
//! ```text
//! renderer ──► output faults (drop / repeat frames) ──► DAC ring (per output channel)
//!                                                           │  clock drift (resampling)
//!                                                           ▼
//! ADC input i = base content + noise + Σ paths (delay, nonlinearity, FIR, noise) from DAC channels
//! ```
//!
//! Time is simulated: header and tick timestamps are device time derived from the frame
//! count, so results are bit-for-bit deterministic for a given [`FakeConfig::seed`]. In
//! [`FakeDrive::Manual`] a test advances the device with [`FakeDriver::step`]; nothing
//! depends on wall-clock time.
//!
//! # Index relations
//!
//! The fake is a single-callback device: the output index of a block equals its capture
//! index. Without faults, DAC frame `d` is output frame `d`, and a path with delay `D`
//! makes capture index `n` hear output index `n − D`. Dropping `k` output frames shifts the
//! loopback offset by −k, repeating `k` frames by +k. A drift of `ppm` makes capture index
//! `n` hear DAC position `n · (1 + ppm·1e-6) − D` (positive = DAC clock fast), so the
//! offset changes by `−ppm·1e-6` samples per sample.

/// Device id the fake lists ([`FakeEndpoints::Duplex`]).
pub const FAKE_DEVICE_ID: &str = "fake";
/// Capture device id of [`FakeEndpoints::Split`].
pub const FAKE_INPUT_ID: &str = "fake:in";
/// Playback device id of [`FakeEndpoints::Split`].
pub const FAKE_OUTPUT_ID: &str = "fake:out";

mod analog;
mod config;
mod device;
mod outage;
mod sim;
#[cfg(test)]
mod tests;

pub use config::{
    FakeConfig, FakeConfigError, FakeDrive, FakeEndpoints, FakeFault, FakePath, FakeReverb,
    InputContent, Pace, signature,
};
pub use device::FakeBackend;
pub use outage::OutageKind;
pub use sim::{FakeDriver, FakeStats};
