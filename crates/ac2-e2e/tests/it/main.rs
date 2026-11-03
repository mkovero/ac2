//! The app's end-to-end tests, one binary with a module per area. Without a window or GPU
//! (but `connect`): the reducer and the link thread driven exactly as the app drives them,
//! against real daemons on the simulated rig (never real audio).
//!
//! - The simulated rig starts measuring: session open, "demo" running, frames that show the
//!   rig's acoustic path once the stimulus plays.
//! - An empty daemon (the simulated rig with no setup, as a daemon on real audio starts; or
//!   a stand-alone local daemon) is made to measure from the app alone: the session dialog
//!   opens a session, the new-measurement dialog creates and starts a measurement, frames
//!   arrive.

// The fake rig and pane helpers of ac2-ui's own tests, shared rather than copied.
#[path = "../../../ac2-ui/tests/it/common/mod.rs"]
mod common;
mod connect;

mod advice;
mod align;
mod calibration;
mod cursor;
mod drop_math;
mod eq;
mod harness;
mod keep;
mod math;
mod panes;
mod session;
mod spatial;
mod spl;
mod stimulus;
mod sweep;
mod transfer;
mod walk;
