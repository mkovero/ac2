//! Rig settings files of format version 7, read into the current shape.
//!
//! Its meters' panes do not name the rig they show: a meter's layout was this daemon's by
//! being in its file, which is what [`RigRef::ThisRig`](ac2_proto::model::layout::RigRef)
//! now says. Every other section is the same.

use serde_json::{Map, Value};

/// `obj` (a version 7 file without its `format` and `version`) as version 8 sections.
pub(super) fn upgrade(obj: &mut Map<String, Value>) {
    if let Some(Value::Array(meters)) = obj.get_mut("meters") {
        for m in meters {
            if let Some(layout) = m.get_mut("layout") {
                ac2_traces::old_layout::panes_this_rig(layout);
            }
        }
    }
}
