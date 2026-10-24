//! Rig settings files of format version 2, read into the current shape.
//!
//! Version 3 added the `levels` section (the programme meters' ballistic, correlation
//! window, over threshold and correlation gate). A version 2 file lacks it because nothing
//! had been set, so it takes the defaults rather than reading as damaged; every other
//! section is the same.

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
