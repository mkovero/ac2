//! Rig settings files of format version 8, read into the current shape.
//!
//! Its meters' panes named one rig for all a pane showed, and a pane with no pick in a
//! meter's own layout showed that meter: now every reference names its rig and a pane with
//! no pick holds nothing ([`ac2_traces::old_layout::panes_curves`]). Every other section is
//! the same.

use ac2_proto::model::MeasConfig;
use ac2_traces::old_layout::{LayoutOwner, panes_curves};
use serde_json::{Map, Value};

/// `obj` (a version 8 file without its `format` and `version`) as version 9 sections.
pub(super) fn upgrade(obj: &mut Map<String, Value>) {
    if let Some(Value::Array(meters)) = obj.get_mut("meters") {
        for m in meters {
            // A meter whose configuration does not read stands for no pane: its section
            // reader reports it, and its panes with no pick hold nothing.
            let owner = m
                .get("config")
                .and_then(|c| serde_json::from_value::<MeasConfig>(c.clone()).ok())
                .zip(m.get("id").cloned())
                .map(|(c, id)| LayoutOwner::of(id, &c.kind));
            if let Some(layout) = m.get_mut("layout") {
                panes_curves(layout, owner.as_ref(), &[]);
            }
        }
    }
}
