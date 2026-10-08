//! The crate's integration tests, one binary with a module per area: every test binary
//! links the whole dependency graph, so one link replaces one per file. A single area:
//! `cargo nextest run -p ac2d -E 'test(/^autosave::/)'` or `cargo test -p ac2d --test it autosave::`.

mod autosave;
mod band_leq;
mod calibration;
mod ceiling;
mod client;
mod common;
mod devices;
mod discovery;
mod drift;
mod jack;
mod leq;
mod math_channels;
mod network;
mod publish;
mod recording;
mod recovery;
mod remote_stimulus;
mod server;
mod stimulus;
mod sweep;
mod sync;
mod traces;
mod transfer;
