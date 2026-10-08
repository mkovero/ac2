//! The crate's integration tests, one binary with a module per area: every test binary
//! links the whole dependency graph, so one link replaces one per file. A single area:
//! `cargo nextest run -p ac2-audio -E 'test(/^fake::/)'` or `cargo test -p ac2-audio --test it fake::`.

mod fake;
mod jack;
