//! The crate's integration tests, one binary with a module per area: every test binary
//! links the whole dependency graph, so one link replaces one per file. A single area:
//! `cargo nextest run -p ac2-core -E 'test(/^delay::/)'` or `cargo test -p ac2-core --test it delay::`.

mod delay;
mod mtw;
mod mtw_delay;
mod mtw_timing;
mod rta_timing;
mod spl_bench;
