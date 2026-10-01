//! Pure DSP and acoustics math: MTW transfer function, smoothing, averaging, delay finder,
//! spectrum, RTA, SPL, generator. No I/O, no threads.
#![forbid(unsafe_code)]

pub mod grid;
pub mod window;

// Filled by phase 2 work; each module owns one concern from PLAN.md §5.
pub mod average;
pub mod delay;
pub mod generator;
pub mod ir_view;
pub mod mtw;
pub mod protection;
pub mod rta;
pub mod smoothing;
pub mod spectrum;
pub mod spl;
pub mod timing;
pub mod weighting;
