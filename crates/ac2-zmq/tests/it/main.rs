//! The crate's integration tests, one binary with a module per area: every test binary
//! links the whole dependency graph, so one link replaces one per file. A single area:
//! `cargo nextest run -p ac2-zmq -E 'test(/^ctrl::/)'` or `cargo test -p ac2-zmq --test it ctrl::`.

mod common;
mod ctrl;
mod curve;
mod latest;
mod liveness;
mod parts;
mod pubsub;
mod transports;
