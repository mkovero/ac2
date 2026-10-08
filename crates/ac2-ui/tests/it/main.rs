//! The crate's integration tests, one binary with a module per area: every test binary
//! links the whole dependency graph, so one link replaces one per file. A single area:
//! `cargo nextest run -p ac2-ui -E 'test(/^conn::/)'` or `cargo test -p ac2-ui --test it conn::`.

mod common;
mod conn;
mod connect;
mod embedded;
mod keymap_doc;
mod no_dsp;
mod ui;
