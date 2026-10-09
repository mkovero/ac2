//! Pure DSP and acoustics math: MTW transfer function, smoothing, averaging, delay finder,
//! spectrum, RTA, SPL, generator, sweep analysis and room acoustics. No I/O, no threads.
#![forbid(unsafe_code)]

pub mod grid;
pub mod window;

// Filled by phase 2 work; each module owns one concern from PLAN.md §5.
pub mod average;
pub mod band_leq;
pub mod delay;
pub mod generator;
pub mod ir_view;
pub mod leq;
pub mod loopback;
pub mod mic_curve;
pub mod mtw;
pub mod power_average;
pub mod protection;
pub mod room;
pub mod rta;
pub mod smoothing;
pub mod spectrum;
pub mod spl;
pub mod sweep;
pub mod timing;
pub mod weighting;
