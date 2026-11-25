//! The crate's integration tests, one binary with a module per area. A single area:
//! `cargo nextest run -p ac2-resampler -E 'test(/^quality::/)'`.

#![allow(clippy::unwrap_used)]

mod chunking;
mod design;
mod exact;
mod positions;
mod pull;
mod quality;
mod retention;
mod servo;
mod servo_loop;
mod servo_sim;
mod servo_stamps;
mod servo_tone;
