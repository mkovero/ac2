//! Warnings a reply carries beside its outcome.

use serde::{Deserialize, Serialize};

use crate::model::{ArtnetBindFailure, DmxOutput, RecordingRef};
use crate::units::{Dbu, MeasId};

/// Something the command did that the caller should know about, beside its outcome.
/// Wire: `{"type": "<name>", …}` in the reply's `warnings`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Warning {
    /// `stream.close` ended the audio the rig SPL meters meter: they read nothing and their
    /// OSC cues report a fault until a stream opens again.
    RigMonitorBlind {
        /// The rig-scoped measurements affected.
        meas: Vec<MeasId>,
    },
    /// `stream.open` moved to another backend or input device: the rig SPL meters meter the
    /// new device's inputs from now on.
    RigMonitorMoved {
        /// The rig-scoped measurements affected.
        meas: Vec<MeasId>,
    },
    /// The command closed or reopened the stream a recording was recording: the recording
    /// ended there, its file finalised (`file.load`, `file.new`, `stream.open`,
    /// `stream.close`, `stream.replay`, …; `rec.stop` ends one on purpose and says nothing).
    RecordingEnded {
        /// The recording that ended.
        recording: RecordingRef,
    },
    /// `artnet.target` opened the Art-Net socket, but not on Art-Net's port 6454: frames go
    /// out from `from_port`, which nodes that check the source port drop, and no console
    /// lists the rig (`rig.artnet_socket` holds it while it lasts).
    ArtnetNotConforming {
        /// The port frames go out from.
        from_port: u16,
        /// Why 6454 could not be bound.
        reason: ArtnetBindFailure,
    },
    /// `artnet.target` set universe 0 (Port-Address 0): kept, but Art-Net 4 deprecates it
    /// (sACN has no universe 0), so a node may not take it.
    ArtnetUniverseZero,
    /// `artnet.test` sent its frame to the other DMX output but not to `output`.
    DmxSendFailed {
        /// The output that did not send.
        output: DmxOutput,
        /// Why, in the system's words.
        error: String,
    },
    /// `cal.line` stored a full scale outside the usual +4 … +30 dBu of line interfaces:
    /// within what an input can be (−40 … +50 dBu), but worth a second look at the volts
    /// typed (mV for V) and at the level measured.
    LineLevelUnusual {
        /// The line full scale stored.
        full_scale: Dbu,
    },
}
