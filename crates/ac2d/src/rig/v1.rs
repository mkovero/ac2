//! Rig settings files of format version 1, read into the current shape.
//!
//! Version 1 kept each rig meter as a session file's measurement, so every field a session
//! measurement gained made the whole file unreadable for the next build. Version 2 keeps
//! the narrow [`super::RigMeter`]. Its sections are otherwise the same, so a version 1
//! file is turned into version 2 JSON here and then read like any other:
//! - `measurements` is `meters`;
//! - the session-only fields of each meter are dropped: they never belonged to the rig;
//! - keys version 1 files written before them lack take what their absence meant then.
//!
//! The result goes through [`super::v2::upgrade`] like any version 2 file; the daemon
//! writes the current version straight after, the version 1 file kept beside it.

use ac2_proto::model::ArtnetFade;
use serde_json::{Map, Value};

/// What a session measurement holds and a rig meter does not: the transfer delay, the
/// compare picks, the display offset of live curves.
const SESSION_ONLY: [&str; 3] = ["delay", "compared", "offset"];

/// `obj` (a version 1 file without its `format` and `version`) as version 2 sections.
pub(super) fn upgrade(obj: &mut Map<String, Value>) {
    let meters = obj
        .remove("measurements")
        .unwrap_or_else(|| Value::Array(Vec::new()));
    obj.insert("meters".into(), meters);
    if let Some(Value::Array(ms)) = obj.get_mut("meters") {
        for m in ms.iter_mut().filter_map(Value::as_object_mut) {
            for k in SESSION_ONLY {
                m.remove(k);
            }
        }
    }
    let absent = [
        ("monitor_at_start", Value::Bool(false)),
        ("monitor_session", Value::Null),
        ("osc_target", Value::Null),
        ("artnet_target", Value::Null),
        ("artnet_fade", Value::from(ArtnetFade::DEFAULT.seconds())),
    ];
    for (k, v) in absent {
        obj.entry(k).or_insert(v);
    }
}
