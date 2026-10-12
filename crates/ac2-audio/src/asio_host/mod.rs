//! ASIO backend (Windows), on the `azo` binding: ASIO drivers' COM objects
//! directly, no Steinberg SDK.
//!
//! One `bufferSwitch` serves capture and playback on the driver's clock, so both directions
//! of a switch share one exact index taken from the driver's sample position, as on JACK.
//! The driver-independent parts (sample conversion, the position clock, the callback body,
//! message handling, negotiation) are plain Rust and run in the Linux tests against a
//! scripted driver; only `driver.rs` touches COM.

mod backend;
mod engine;
mod format;
mod open;
mod position;

#[cfg(windows)]
mod driver;
#[cfg(windows)]
pub use driver::AsioBackend;

#[cfg(test)]
mod tests;
