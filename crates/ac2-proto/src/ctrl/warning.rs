//! Warnings a reply carries beside its outcome.

use serde::{Deserialize, Serialize};

use crate::units::MeasId;

/// Something the command did that the caller should know about although it succeeded.
/// Wire: `{"type": "<name>", …}` in the reply's `warnings`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Warning {
    /// `session.close` ended the audio the rig SPL meters meter: they read nothing and their
    /// OSC cues report a fault until a session opens again.
    RigMonitorBlind {
        /// The rig-scoped measurements affected.
        meas: Vec<MeasId>,
    },
    /// `session.open` moved to another backend or input device: the rig SPL meters meter the
    /// new device's inputs from now on.
    RigMonitorMoved {
        /// The rig-scoped measurements affected.
        meas: Vec<MeasId>,
    },
}
