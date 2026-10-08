//! The crate's integration tests, one binary with a module per area: every test binary
//! links the whole dependency graph, so one link replaces one per file. A single area:
//! `cargo nextest run -p ac2-proto -E 'test(/^doc_parity::/)'` or `cargo test -p ac2-proto --test it doc_parity::`.

mod doc_parity;
mod fixtures;
mod fuzz;
mod leq_presets;
mod roundtrip;
