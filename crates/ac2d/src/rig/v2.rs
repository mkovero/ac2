//! Rig settings files of format version 2, read into the current shape.
//!
//! Such a file has no `levels` section (the programme meters' ballistic, correlation window,
//! over threshold and correlation gate): nothing had been set, so the defaults are the right
//! reading and the file is not damaged. Every other section is the same.

use ac2_proto::model::LevelsSettings;
use serde_json::{Map, Value};

/// `obj` (a version 2 file without its `format` and `version`) as version 3 sections.
pub(super) fn upgrade(obj: &mut Map<String, Value>) {
    if !obj.contains_key("levels")
        && let Ok(levels) = serde_json::to_value(LevelsSettings::default())
    {
        obj.insert("levels".into(), levels);
    }
}
