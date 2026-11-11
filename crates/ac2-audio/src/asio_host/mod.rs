//! ASIO backend (Windows), on the `azo` binding: ASIO drivers' COM objects
//! directly, no Steinberg SDK.
//!
//! One `bufferSwitch` serves capture and playback on the driver's clock, so both directions
//! of a switch share one exact index taken from the driver's sample position, as on JACK.
//! The driver-independent parts (sample conversion, the position clock, the callback body,
//! message handling, negotiation) are plain Rust and run in the Linux tests against a
//! scripted driver; only `driver.rs` touches COM.

mod backend;
// The callback body reads and writes the driver's buffers through the raw pointers ASIO
// hands out, and the engine crosses to the driver's thread holding them.
#[allow(unsafe_code)]
mod engine;
// Sample conversion at a raw pointer into a driver buffer, whose layout only the driver's
// reported sample type describes.
#[allow(unsafe_code)]
mod format;
mod open;
mod position;

#[cfg(windows)]
mod driver;
#[cfg(windows)]
pub use driver::AsioBackend;

// The scripted driver plays the driver's part: it owns the buffers and fills them by pointer.
#[cfg(test)]
#[allow(unsafe_code)]
mod tests;
