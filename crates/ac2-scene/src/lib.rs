//! Pure display layer: turns frames and view state into plain-data scenes (geometry, axes,
//! readouts, banners).
//!
//! If a displayed value can be wrong, it is computed here, where a test can assert it
//! without a window: tick values and labels, readout strings, banner choice and order,
//! frame age, the phase-comparison rotation, coherence alpha, gaps for invalid columns.
//! `ac2-plot` only places pixels for the [`primitives`] produced here.
//!
//! Every builder is a pure function of its inputs (data, [`ViewState`], [`Theme`], size).
//! Nothing here reads a clock, opens a socket or touches the GPU; the caller passes "now"
//! and the clock offset ([`time`]).
//!
//! Builders: [`tf::transfer_scene`], [`spectrum::spectrum_scene`], [`ir::ir_scene`],
//! [`spl::spl_scene`], [`distortion::distortion_scene`]. Strings without geometry: [`readout`], [`banner::banners`],
//! [`spl::spl_readout`].
#![forbid(unsafe_code)]

pub mod autosave;
pub mod axis;
pub mod banner;
pub mod cal;
mod canvas;
pub mod distortion;
pub mod finding;
pub mod format;
pub mod grid;
pub mod ir;
pub mod leq;
pub mod meter;
pub mod primitives;
pub mod progress;
pub mod readout;
pub mod spectrum;
pub mod spl;
pub mod tf;
pub mod theme;
pub mod time;
pub mod trace;
pub mod view;

pub use primitives::*;
pub use theme::Theme;
pub use view::ViewState;
