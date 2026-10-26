//! Pane layouts of earlier shapes, read into today's by salvage and the rig file's upgrade.
//!
//! - Before a pane could meter inputs it named only a measurement it was picked to show
//!   (`meas`), where it now names what it shows (`content`): session format 31 and rig
//!   settings version 2 ([`pane_meas_to_content`]).
//! - Before the spectrum's list of tracked peaks was a plate a pane's `plates` held three
//!   plates, without `peaks`: session format 32 and rig settings version 3
//!   ([`plates_add_peaks`]).
//! - Before the Programme pane's loudness readout was a plate a pane's `plates` held four
//!   plates, without `loudness`: session format 33 and rig settings version 4
//!   ([`plates_add_loudness`]). A tree older than both lacks `peaks` too, so every caller
//!   runs [`plates_add_peaks`] first.

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

/// Gives every pane's `plates` in the layout tree `v` that lacks one a `peaks` plate where
/// the client puts it, at the plot's text size: a list that was not a plate had not been
/// moved. A pane is an object holding both `kind` and `chrome`; anything else is walked
/// through.
pub fn plates_add_peaks(v: &mut Value) {
    plates_add(v, "peaks");
}

/// Gives every pane's `plates` in the layout tree `v` that lacks one a `loudness` plate
/// docked where the pane puts it, at the plot's text size: a column that was not a plate
/// had not been moved. The same walk as [`plates_add_peaks`].
pub fn plates_add_loudness(v: &mut Value) {
    plates_add(v, "loudness");
}

/// A pane with no `plates` key is left so: the pane's whole default stands for them, and a
/// table holding only the new plate would miss its required siblings.
fn plates_add(v: &mut Value, plate: &str) {
    match v {
        Value::Object(obj) => {
            if obj.contains_key("kind") && obj.contains_key("chrome") {
                if let Some(Value::Object(plates)) = obj.get_mut("plates")
                    && !plates.contains_key(plate)
                {
                    let mut auto = Map::new();
                    auto.insert("at".into(), Value::Null);
                    auto.insert("scale".into(), Value::from(1.0));
                    plates.insert(plate.into(), Value::Object(auto));
                }
                return;
            }
            for child in obj.values_mut() {
                plates_add(child, plate);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|c| plates_add(c, plate)),
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

    #[test]
    fn three_plates_gain_an_automatic_peak_list() {
        let plate = |x: f64| json!({"at": [x, 0.0], "scale": 2.0});
        let auto = json!({"at": null, "scale": 1.0});
        let mut v = json!({"type": "split", "a": {"type": "pane", "pane": {
            "kind": "spectrum", "chrome": "full",
            "plates": {"legend": plate(0.1), "readout": plate(0.2), "mark": plate(0.3)}
        }}, "b": {"type": "pane", "pane": {"kind": "spl", "chrome": "full"}}});
        plates_add_peaks(&mut v);
        assert_eq!(
            v["a"]["pane"]["plates"],
            json!({"legend": plate(0.1), "readout": plate(0.2), "mark": plate(0.3),
                "peaks": auto})
        );
        // No plates at all: the pane's own default stands for them.
        assert!(v["b"]["pane"].get("plates").is_none());
        // A layout of today's shape is kept as it is.
        let mut today = v.clone();
        today["a"]["pane"]["plates"]["peaks"] = plate(0.4);
        let before = today.clone();
        plates_add_peaks(&mut today);
        assert_eq!(today, before);
    }

    #[test]
    fn four_plates_gain_a_docked_loudness_readout() {
        let plate = |x: f64| json!({"at": [x, 0.0], "scale": 2.0});
        let auto = json!({"at": null, "scale": 1.0});
        let mut v = json!({"type": "split", "a": {"type": "pane", "pane": {
            "kind": "programme", "chrome": "full",
            "plates": {"legend": plate(0.1), "readout": plate(0.2), "mark": plate(0.3),
                "peaks": plate(0.4)}
        }}, "b": {"type": "pane", "pane": {"kind": "spl", "chrome": "full"}}});
        plates_add_loudness(&mut v);
        assert_eq!(
            v["a"]["pane"]["plates"],
            json!({"legend": plate(0.1), "readout": plate(0.2), "mark": plate(0.3),
                "peaks": plate(0.4), "loudness": auto})
        );
        assert!(v["b"]["pane"].get("plates").is_none());
        let mut today = v.clone();
        today["a"]["pane"]["plates"]["loudness"] = plate(0.5);
        let before = today.clone();
        plates_add_loudness(&mut today);
        assert_eq!(today, before);
    }

    #[test]
    fn a_three_plate_tree_gains_both_in_order() {
        let auto = json!({"at": null, "scale": 1.0});
        let mut v = json!({"kind": "spectrum", "chrome": "full",
            "plates": {"legend": auto, "readout": auto, "mark": auto}});
        plates_add_peaks(&mut v);
        plates_add_loudness(&mut v);
        assert_eq!(
            v["plates"],
            json!({"legend": auto, "readout": auto, "mark": auto, "peaks": auto,
                "loudness": auto})
        );
    }
}
