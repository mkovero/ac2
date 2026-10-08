//! The crate's integration tests, one binary with a module per area: every test binary
//! links the whole dependency graph, so one link replaces one per file. A single area:
//! `cargo nextest run -p ac2-plot -E 'test(/^golden::/)'` or `cargo test -p ac2-plot --test it golden::`.

mod common;
mod golden;
mod prepare_bench;
mod render;
mod scene_e2e;
