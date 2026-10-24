//! Rig settings files of format version 2, read into the current shape.
//!
//! Such a file has no `levels` section (the programme meters' ballistic, correlation window,
//! over threshold and correlation gate): nothing had been set, so the defaults are the right
//! reading and the file is not damaged. Its meters' pane layouts name a picked measurement
//! as `meas`; they are read as today's `content` pick. Every other section is the same.

use ac2_proto::model::LevelsSettings;
use serde_json::{Map, Value};

/// `obj` (a version 2 file without its `format` and `version`) as version 3 sections.
pub(super) fn upgrade(obj: &mut Map<String, Value>) {
    if !obj.contains_key("levels")
        && let Ok(levels) = serde_json::to_value(LevelsSettings::default())
    {
        obj.insert("levels".into(), levels);
    }
    if let Some(Value::Array(meters)) = obj.get_mut("meters") {
        for m in meters {
            if let Some(layout) = m.get_mut("layout") {
                ac2_traces::old_layout::pane_meas_to_content(layout);
            }
        }
    }
}
