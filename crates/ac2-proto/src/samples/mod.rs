//! One deterministic example of every message variant.
//!
//! Used by round-trip tests, the documentation parity test and the cross-language
//! fixtures (`tools/protocol/fixtures.py` builds the same frames in Python).

mod builders;
mod commands;
mod events;
mod frames;
mod replies;
mod state;
mod traces;

pub use builders::{grids, log_grid, math_measurement, pane_layout, sweep_measurement, working};
pub use commands::commands;
pub use events::events;
pub use frames::{frames, stamp, tf_frame};
pub use replies::replies;
pub use state::{band_leq_config, recording_run, server_info, spl_config, spl_measurement, state};
