//! End to end without a window or GPU: the reducer and the link thread driven exactly as the
//! app drives them, against real daemons on the simulated rig (never real audio).
//!
//! - The embedded simulated rig starts measuring: session open, "demo" running, frames that
//!   show the rig's acoustic path once the stimulus plays.
//! - An empty daemon (embedded with no setup, as on real audio; or a stand-alone local
//!   daemon) is made to measure from the app alone: the session dialog opens a session, the
//!   new-measurement dialog creates and starts a measurement, frames arrive.
#![cfg(feature = "embedded")]

mod calibration;
mod harness;
mod math;
mod panes;
mod session;
mod spl;
mod stimulus;
mod sweep;
mod transfer;
