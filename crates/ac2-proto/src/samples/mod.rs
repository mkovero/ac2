//! One deterministic example of every message variant.
//!
//! Used by round-trip tests, the documentation parity test and the cross-language
//! fixtures (`tools/protocol/fixtures.py` builds the same frames in Python).

mod builders;
mod commands;
mod eq;
mod events;
mod frames;
mod replies;
mod sim;
mod state;
mod traces;

pub use builders::{grids, log_grid, math_measurement, pane_layout, sweep_measurement, working};
pub use commands::commands;
pub use eq::{eq_edits, eq_measurement};
pub use events::events;
pub use frames::{frames, stamp, tf_frame};
pub use replies::{replies, reply, warnings};
pub use sim::{sim_fit_request, sim_request, sim_running, sim_state};
pub use state::{
    band_leq_config, plan, recording_run, server_info, spl_config, spl_measurement, state, walk,
};
