//! The crate's integration tests, one binary with a module per area. A single area:
//! `cargo nextest run -p ac2-resampler -E 'test(/^quality::/)'`.

#![allow(clippy::unwrap_used)]

mod chunking;
mod exact;
mod pull;
mod quality;
