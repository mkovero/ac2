//! Room plans and where to measure in them: plan validation, the mirror axes a room and its
//! speakers share, positions read from FOH, and suggested measurement positions. Geometry
//! only: no I/O, no threads, no DSP. The plan types are `ac2-proto`'s wire types.
#![forbid(unsafe_code)]

mod geom;
pub mod pattern;
pub mod plan;
mod rng;
