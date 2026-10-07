//! State entities mirrored by clients, and the configuration enums they are made of.
//!
//! These mirror `ac2-core` / `ac2-audio` types on the wire. They are separate types (not
//! re-exports) so the protocol has no dependency on the DSP or audio crates and so a change
//! there cannot silently change the wire; the daemon converts at its boundary.

mod calibration;
mod defaults;
mod delay;
mod dsp;
mod generator;
mod measurement;
mod recording;
mod session;
mod spl;
mod state;
mod sweep;
mod trace;

pub use calibration::*;
pub use delay::*;
pub use dsp::*;
pub use generator::*;
pub use measurement::*;
pub use recording::*;
pub use session::*;
pub use spl::*;
pub use state::*;
pub use sweep::*;
pub use trace::*;
