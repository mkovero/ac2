//! Rig settings files of format version 4, read into the current shape.
//!
//! Its `levels` section has no loudness target: nothing had been set, so the default (−23
//! LUFS, the R128 programme level) is the right reading. A version 1 file has had the whole
//! default section inserted by [`super::v2`], which already holds it. Its meters' pane
//! layouts hold four plates; today's hold the Programme pane's loudness readout too, read as
//! docked where the pane puts it. Every other section is the same.

use ac2_proto::model::LevelsSettings;
use serde_json::{Map, Value};

/// `obj` (a version 4 file without its `format` and `version`) as version 5 sections.
pub(super) fn upgrade(obj: &mut Map<String, Value>) {
    if let Some(Value::Object(levels)) = obj.get_mut("levels")
        && !levels.contains_key("loudness_target")
    {
        let target = LevelsSettings::default().loudness_target.0;
        levels.insert("loudness_target".into(), Value::from(target));
    }
    if let Some(Value::Array(meters)) = obj.get_mut("meters") {
        for m in meters {
            if let Some(layout) = m.get_mut("layout") {
                ac2_traces::old_layout::plates_add_loudness(layout);
            }
        }
    }
}
