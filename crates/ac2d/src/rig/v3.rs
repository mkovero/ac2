//! Rig settings files of format version 3, read into the current shape.
//!
//! Its meters' pane layouts hold three plates (legend, cursor values, mark readout); today's
//! hold the spectrum's list of tracked peaks too, read as where the client puts it. Every
//! other section is the same.

use serde_json::{Map, Value};

/// `obj` (a version 3 file without its `format` and `version`) as version 4 sections.
pub(super) fn upgrade(obj: &mut Map<String, Value>) {
    if let Some(Value::Array(meters)) = obj.get_mut("meters") {
        for m in meters {
            if let Some(layout) = m.get_mut("layout") {
                ac2_traces::old_layout::plates_add_peaks(layout);
            }
        }
    }
}
