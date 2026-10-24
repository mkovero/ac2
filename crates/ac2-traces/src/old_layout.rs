//! Pane layouts written before a pane could meter inputs: a pane then named only a
//! measurement it was picked to show (`meas`), where it now names what it shows (`content`).
//! Session format 31 and rig settings version 2 store layouts of that shape; salvage and the
//! rig file's upgrade read them through [`pane_meas_to_content`].

use serde_json::{Map, Value};

/// Rewrites every pane in the layout tree `v` from the `meas` pick to the `content` pick:
/// `"meas": <id>` becomes `"content": {"type": "meas", "meas": <id>}`, a `null` pick none.
/// A pane is an object holding both `kind` and `chrome`; anything else is walked through.
pub fn pane_meas_to_content(v: &mut Value) {
    match v {
        Value::Object(obj) => {
            if obj.contains_key("kind") && obj.contains_key("chrome") {
                if let Some(meas) = obj.remove("meas")
                    && !meas.is_null()
                    && !obj.contains_key("content")
                {
                    let mut content = Map::new();
                    content.insert("type".into(), Value::from("meas"));
                    content.insert("meas".into(), meas);
                    obj.insert("content".into(), Value::Object(content));
                }
                return;
            }
            for child in obj.values_mut() {
                pane_meas_to_content(child);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(pane_meas_to_content),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_pick_becomes_content_and_a_null_pick_goes() {
        let mut v = json!({"type": "split", "children": [
            {"type": "pane", "pane": {"kind": "spl", "chrome": "full", "meas": 3}},
            {"type": "pane", "pane": {"kind": "transfer", "chrome": "full", "meas": null}},
        ]});
        pane_meas_to_content(&mut v);
        assert_eq!(
            v,
            json!({"type": "split", "children": [
                {"type": "pane", "pane": {"kind": "spl", "chrome": "full",
                    "content": {"type": "meas", "meas": 3}}},
                {"type": "pane", "pane": {"kind": "transfer", "chrome": "full"}},
            ]})
        );
    }
}
