//! Phase 0 spike: audio-duplex. Throwaway code; findings live in docs/design/spike-audio-duplex.md.
//!
//! Question: can one small trait give RT-safe, multichannel, sample-indexed duplex capture
//! over cpal (CoreAudio/WASAPI/ALSA) and JACK?

pub mod backend;
pub mod block;
pub mod clock;
pub mod cpal_backend;
pub mod fake;
#[cfg(all(feature = "jack", target_os = "linux"))]
pub mod jack_backend;
pub mod output;
pub mod stats;

#[cfg(test)]
mod tests;
