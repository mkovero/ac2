//! Rig settings files of format version 9, read into the current shape.
//!
//! Its meters' panes carried no 3D spectrograph camera: each gets the default
//! ([`ac2_traces::old_layout::panes_spectrum_camera`]). Every other section is the same.

use ac2_traces::old_layout::panes_spectrum_camera;
use serde_json::{Map, Value};

/// `obj` (a version 9 file without its `format` and `version`) as version 10 sections.
pub(super) fn upgrade(obj: &mut Map<String, Value>) {
    if let Some(Value::Array(meters)) = obj.get_mut("meters") {
        for m in meters {
            if let Some(layout) = m.get_mut("layout") {
                panes_spectrum_camera(layout);
            }
        }
    }
}
