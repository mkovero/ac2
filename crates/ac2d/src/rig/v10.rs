//! Rig settings files of format version 10, read into the current shape.
//!
//! They kept no paired rigs (an empty `peers` section), and their monitor session took no
//! remote inputs. Every other section is the same.

use serde_json::{Map, Value};

/// `obj` (a version 10 file without its `format` and `version`) as version 11 sections.
pub(super) fn upgrade(obj: &mut Map<String, Value>) {
    obj.insert("peers".into(), Value::Array(Vec::new()));
    if let Some(Value::Object(s)) = obj.get_mut("monitor_session") {
        s.insert("remote_inputs".into(), Value::Array(Vec::new()));
        s.insert("link_offset".into(), Value::Null);
    }
}
