//! Rig settings files of format version 5, read into the current shape.
//!
//! Its monitor session names no spare inputs: none could be opened then, so zero is the
//! right reading. Every other section is the same.

use serde_json::{Map, Value};

/// `obj` (a version 5 file without its `format` and `version`) as version 6 sections.
pub(super) fn upgrade(obj: &mut Map<String, Value>) {
    if let Some(Value::Object(session)) = obj.get_mut("monitor_session")
        && !session.contains_key("spare_inputs")
    {
        session.insert("spare_inputs".into(), Value::from(0));
    }
}
