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
//! - Before the Programme pane had a meters and a goniometer view a pane carried no
//!   `programme_view`, and a pair's pane drew both: session format 34 and rig settings
//!   version 6 ([`programme_views`]). Every caller runs it after [`plates_add_loudness`].
//! - Before a pane named the rig it shows a layout was one daemon's by being in its file:
//!   session format 35 and rig settings version 7 ([`panes_this_rig`]). Runs last, on the
//!   panes the earlier steps leave.

use ac2_proto::model::layout::PaneLayout;
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

/// Gives every pane in the layout tree `v` that lacks one a `programme_view` of `meters`, and
/// turns a Programme pane metering a pair into the two panes a pair now brings: its tree
/// node (`{"type": "pane", "pane": …}`) becomes a row split of a goniometer pane on the left
/// and a meters pane on the right, both with the pane's content, chrome and levels. The
/// goniometer copy drops its `plates`, since a goniometer has none and a pane without the
/// key takes the whole default. A tree holds at most [`PaneLayout::MAX_PANES`] panes, and
/// one over that fails the whole layout, so pair panes are split only while the total stays
/// within it; the rest stay one meters pane each. A pane is an object holding both `kind`
/// and `chrome`; anything else is walked through.
pub fn programme_views(v: &mut Value) {
    let mut panes = count_panes(v);
    add_views(v, &mut panes);
}

/// Gives every pane in the layout tree `v` without a `rig` the rig of the file it is in
/// (`{"type": "this_rig"}`): every pane of such a file showed that file's daemon. A pane
/// is an object holding both `kind` and `chrome`; anything else is walked through.
pub fn panes_this_rig(v: &mut Value) {
    match v {
        Value::Object(obj) if is_pane(obj) => {
            obj.entry("rig").or_insert_with(|| {
                let mut r = Map::new();
                r.insert("type".into(), Value::from("this_rig"));
                Value::Object(r)
            });
        }
        Value::Object(obj) => obj.values_mut().for_each(panes_this_rig),
        Value::Array(items) => items.iter_mut().for_each(panes_this_rig),
        _ => {}
    }
}

fn is_pane(obj: &Map<String, Value>) -> bool {
    obj.contains_key("kind") && obj.contains_key("chrome")
}

fn count_panes(v: &Value) -> usize {
    match v {
        Value::Object(obj) if is_pane(obj) => 1,
        Value::Object(obj) => obj.values().map(count_panes).sum(),
        Value::Array(items) => items.iter().map(count_panes).sum(),
        _ => 0,
    }
}

/// The pane inside a tree node `{"type": "pane", "pane": …}` that meters a pair and has no
/// view yet.
fn old_pair_pane(obj: &Map<String, Value>) -> Option<&Map<String, Value>> {
    if obj.get("type").and_then(Value::as_str) != Some("pane") {
        return None;
    }
    let pane = obj.get("pane")?.as_object()?;
    let pair = pane.get("kind").and_then(Value::as_str) == Some("programme")
        && pane
            .get("content")
            .and_then(|c| c.get("type"))
            .and_then(Value::as_str)
            == Some("pair")
        && !pane.contains_key("programme_view");
    (is_pane(pane) && pair).then_some(pane)
}

fn add_views(v: &mut Value, panes: &mut usize) {
    match v {
        Value::Object(obj) => {
            if is_pane(obj) {
                obj.entry("programme_view")
                    .or_insert_with(|| Value::from("meters"));
                return;
            }
            if *panes < PaneLayout::MAX_PANES
                && let Some(pane) = old_pair_pane(obj)
            {
                let mut gonio = pane.clone();
                gonio.insert("programme_view".into(), Value::from("goniometer"));
                gonio.remove("plates");
                let mut meters = pane.clone();
                meters.insert("programme_view".into(), Value::from("meters"));
                let node = |p: Map<String, Value>| {
                    let mut n = Map::new();
                    n.insert("type".into(), Value::from("pane"));
                    n.insert("pane".into(), Value::Object(p));
                    Value::Object(n)
                };
                let mut split = Map::new();
                split.insert("type".into(), Value::from("split"));
                split.insert("axis".into(), Value::from("row"));
                split.insert("ratio".into(), Value::from(0.5));
                split.insert("a".into(), node(gonio));
                split.insert("b".into(), node(meters));
                *obj = split;
                *panes += 1;
                return;
            }
            for child in obj.values_mut() {
                add_views(child, panes);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|c| add_views(c, panes)),
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

    fn pane(kind: &str, content: Value) -> Value {
        json!({"type": "pane", "pane": {"kind": kind, "transfer_view": "magnitude",
            "chrome": "no_grid", "content": content,
            "levels": {"transfer": {"lo": -6.0, "hi": 6.0}},
            "plates": {"loudness": {"at": [0.5, 0.5], "scale": 2.0}}}})
    }

    #[test]
    fn a_pair_pane_becomes_a_goniometer_beside_its_meters() {
        let pair = json!({"type": "pair", "left": 2, "right": 3});
        let inputs = json!({"type": "inputs", "inputs": [0, 4]});
        let mut v = json!({"type": "split", "axis": "column", "ratio": 0.5,
            "a": pane("programme", pair.clone()),
            "b": {"type": "split", "axis": "row", "ratio": 0.3,
                "a": pane("programme", inputs.clone()),
                "b": pane("transfer", Value::Null)}});
        programme_views(&mut v);
        let with = |p: Value, view: &str| {
            let mut p = p;
            p["pane"]["programme_view"] = Value::from(view);
            p
        };
        let mut gonio = with(pane("programme", pair.clone()), "goniometer");
        gonio["pane"]
            .as_object_mut()
            .expect("a pane")
            .remove("plates");
        assert_eq!(
            v,
            json!({"type": "split", "axis": "column", "ratio": 0.5,
                "a": {"type": "split", "axis": "row", "ratio": 0.5,
                    "a": gonio, "b": with(pane("programme", pair), "meters")},
                "b": {"type": "split", "axis": "row", "ratio": 0.3,
                    "a": with(pane("programme", inputs), "meters"),
                    "b": with(pane("transfer", Value::Null), "meters")}})
        );
        // A tree of today's shape is kept as it is.
        let before = v.clone();
        programme_views(&mut v);
        assert_eq!(v, before);
    }

    #[test]
    fn a_pair_pane_at_the_tree_root_is_split_there() {
        let mut v = pane("programme", json!({"type": "pair", "left": 0, "right": 1}));
        programme_views(&mut v);
        assert_eq!(v["type"], "split");
        assert_eq!(v["a"]["pane"]["programme_view"], "goniometer");
        assert_eq!(v["b"]["pane"]["programme_view"], "meters");
        assert_eq!(v["b"]["pane"]["chrome"], "no_grid");
        assert_eq!(v["a"]["pane"]["levels"], v["b"]["pane"]["levels"]);
    }

    /// A row of `n` panes, the given ones metering a pair.
    fn row(n: usize, pairs: &[usize]) -> Value {
        let mut tree = Value::Null;
        for i in (0..n).rev() {
            let p = if pairs.contains(&i) {
                pane("programme", json!({"type": "pair", "left": 0, "right": 1}))
            } else {
                pane("spl", Value::Null)
            };
            tree = if tree.is_null() {
                p
            } else {
                json!({"type": "split", "axis": "row", "ratio": 0.5, "a": p, "b": tree})
            };
        }
        tree
    }

    #[test]
    fn pair_panes_split_only_while_the_tree_stays_within_its_pane_limit() {
        let mut v = row(15, &[3, 9]);
        programme_views(&mut v);
        panes_this_rig(&mut v);
        assert_eq!(count_panes(&v), PaneLayout::MAX_PANES);
        let text = v.to_string();
        assert_eq!(text.matches("\"goniometer\"").count(), 1, "{text}");
        assert_eq!(text.matches("\"meters\"").count(), 15, "{text}");
        let layout: PaneLayout = serde_json::from_value(strip_for_wire(v)).expect("a wire layout");
        layout.check().expect("within the pane limit");
    }

    #[test]
    fn every_pane_gains_the_rig_of_its_file() {
        let pinned = json!({"type": "rig", "id": 7, "name": "ketunkolo"});
        let mut v = json!({"type": "split", "a": {"type": "pane", "pane": {
            "kind": "spl", "chrome": "full"
        }}, "b": {"type": "pane", "pane": {
            "kind": "transfer", "chrome": "full", "rig": pinned.clone()
        }}});
        panes_this_rig(&mut v);
        assert_eq!(v["a"]["pane"]["rig"], json!({"type": "this_rig"}));
        assert_eq!(
            v["b"]["pane"]["rig"], pinned,
            "a pane naming its rig keeps it"
        );
    }

    /// The test panes carry only the keys the walk reads; fill the rest the wire requires.
    fn strip_for_wire(mut v: Value) -> Value {
        if let Value::Object(obj) = &mut v {
            if is_pane(obj) {
                obj.insert("spectrum_view".into(), Value::from("spectrum"));
                obj.insert("spl_view".into(), Value::from("meter"));
                obj.insert("sweep_view".into(), Value::from("response"));
                obj.remove("plates");
                obj.remove("levels");
            } else {
                for c in obj.values_mut() {
                    *c = strip_for_wire(c.take());
                }
            }
        }
        v
    }
}
