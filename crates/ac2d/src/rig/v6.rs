//! Rig settings files of format version 6, read into the current shape.
//!
//! Its meters' pane layouts carry no Programme view: a pair's Programme pane drew its bars and
//! its goniometer in one, and now a pair brings a meters pane and a goniometer pane. Every
//! other section is the same.

use serde_json::{Map, Value};

/// `obj` (a version 6 file without its `format` and `version`) as version 7 sections.
pub(super) fn upgrade(obj: &mut Map<String, Value>) {
    if let Some(Value::Array(meters)) = obj.get_mut("meters") {
        for m in meters {
            if let Some(layout) = m.get_mut("layout") {
                ac2_traces::old_layout::programme_views(layout);
            }
        }
    }
}
