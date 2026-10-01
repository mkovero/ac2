//! Test support: golden-vector loading from tools/refgen and tolerance comparison.
//!
//! Golden vectors are produced by `tools/refgen/generate.py` into `fixtures/golden/` as
//! `<name>.json` (metadata) + `<name>.bin` (little-endian f64 blob). The format is
//! documented in `tools/refgen/README.md`.
//!
//! With the `image` feature, [`image`] adds RGBA8 PNG I/O, per-pixel tolerance comparison
//! and a bless/compare helper for renderer golden images.
#![forbid(unsafe_code)]

pub mod compare;
pub mod golden;
#[cfg(feature = "image")]
pub mod image;

pub use compare::{Comparison, DbTolerance, Failure, Mismatch, Tolerance, assert_ok};
pub use golden::{
    ArrayMeta, Dtype, GoldenError, GoldenSet, Metadata, SuggestedTolerance, fixtures_dir,
};
pub use num_complex::Complex64;
