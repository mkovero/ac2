//! Test support: golden-vector loading from tools/refgen and tolerance comparison.
//!
//! Golden vectors are produced by `tools/refgen/generate.py` into `fixtures/golden/` as
//! `<name>.json` (metadata) + `<name>.bin` (little-endian f64 blob). The format is
//! documented in `tools/refgen/README.md`.
#![forbid(unsafe_code)]

pub mod compare;
pub mod golden;

pub use compare::{Comparison, DbTolerance, Failure, Mismatch, Tolerance, assert_ok};
pub use golden::{
    ArrayMeta, Dtype, GoldenError, GoldenSet, Metadata, SuggestedTolerance, fixtures_dir,
};
pub use num_complex::Complex64;
