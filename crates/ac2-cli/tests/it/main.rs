//! The crate's integration tests, one binary with a module per area: every test binary
//! links the whole dependency graph, so one link replaces one per file. A single area:
//! `cargo nextest run -p ac2-cli -E 'test(/^band_rig::/)'` or `cargo test -p ac2-cli --test it band_rig::`.

mod artnet_rig;
mod band_rig;
mod discover;
mod drift_rig;
mod fake_parity;
mod json;
mod leq_rig;
mod math_rig;
mod meas_rig;
mod osc_rig;
mod out_device_rig;
mod pairing;
mod parse;
mod rec_rig;
mod recovery_rig;
mod salvage_rig;
mod spl_rig;
mod tree_rig;
