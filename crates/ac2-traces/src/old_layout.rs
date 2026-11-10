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
//!   session format 35 and rig settings version 7 ([`panes_this_rig`]). Runs after the
//!   steps above, on the panes they leave.
//! - Before every pane reference named its rig a pane named one rig for all it showed, a
//!   pane with no pick in a measurement's own layout showed that measurement, and a
//!   measurement kept a compare list of curves its panes drew over its own: session format
//!   40 and rig settings version 8 ([`panes_curves`]).
//! - Before the spectrum pane had a 3D spectrograph a pane carried no `spectrum_camera`:
//!   session format 41 and rig settings version 9 ([`panes_spectrum_camera`]). Independent
//!   of the steps above; runs after them.

use ac2_proto::model::MeasKind;
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

/// The measurement whose own layout a tree is, for [`panes_curves`]: its id as the file
/// has it and the pane kinds (`"transfer"`, `"spl"`, …) that draw it.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutOwner {
    /// Its id, as the file wrote it.
    pub meas: Value,
    /// The pane kinds that draw it.
    pub draws: Vec<&'static str>,
}

impl LayoutOwner {
    /// The owner of id `meas` (as the file wrote it) of kind `kind`.
    pub fn of(meas: Value, kind: &MeasKind) -> Self {
        Self {
            meas,
            draws: panes_drawing(kind),
        }
    }
}

/// The pane kinds (as a layout names them) that draw a measurement of kind `k`: the
/// transfer pane its transfer curve or a sweep's runs, the spectrum pane its levels, the
/// SPL pane a meter, the sweep pane a sweep.
pub fn panes_drawing(k: &MeasKind) -> Vec<&'static str> {
    let mut v = Vec::new();
    if k.publishes_tf() || matches!(k, MeasKind::Sweep { .. }) {
        v.push("transfer");
    }
    if k.publishes_levels() {
        v.push("spectrum");
    }
    if matches!(k, MeasKind::Spl { .. }) {
        v.push("spl");
    }
    if matches!(k, MeasKind::Sweep { .. }) {
        v.push("distortion");
    }
    v
}

const GRAPH_PANES: [&str; 3] = ["transfer", "spectrum", "distortion"];

fn tagged(kind: &str, fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    let mut m = Map::new();
    m.insert("type".into(), Value::from(kind));
    m.extend(fields.into_iter().map(|(k, v)| (k.to_owned(), v)));
    Value::Object(m)
}

fn this_rig() -> Value {
    tagged("this_rig", [])
}

fn entry(rig: Value, what: Value) -> Value {
    let mut m = Map::new();
    m.insert("rig".into(), rig);
    m.insert("what".into(), what);
    Value::Object(m)
}

/// Rewrites every pane in the layout tree `v` from one rig per pane to a rig per reference:
///
/// - a measurement pick becomes the measurement's group on a graph pane, its meter on an
///   SPL pane, nothing on any other (where no measurement was drawn);
/// - no pick becomes `owner`'s group (its meter on an SPL pane) on a pane of a kind that
///   draws it, else nothing: a pane with no pick in a measurement's own layout showed it;
/// - inputs, a pair and a walk take the pane's rig;
/// - the pane's `rig` goes onto each reference;
/// - `compared`, the owner's old compare list, follows its group in every pane of this rig
///   that draws that group, as single live curves and traces in the list's order.
///
/// Returns how many panes took the compare list: none means it is dropped, which the caller
/// reports. A pane is an object holding both `kind` and `chrome`; anything else is walked
/// through.
pub fn panes_curves(v: &mut Value, owner: Option<&LayoutOwner>, compared: &[Value]) -> usize {
    match v {
        Value::Object(obj) if is_pane(obj) => pane_curves(obj, owner, compared),
        Value::Object(obj) => obj
            .values_mut()
            .map(|c| panes_curves(c, owner, compared))
            .sum(),
        Value::Array(items) => items
            .iter_mut()
            .map(|c| panes_curves(c, owner, compared))
            .sum(),
        _ => 0,
    }
}

fn pane_curves(
    obj: &mut Map<String, Value>,
    owner: Option<&LayoutOwner>,
    compared: &[Value],
) -> usize {
    let rig = obj.remove("rig").unwrap_or_else(this_rig);
    let kind = obj
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let content = obj.remove("content").filter(|c| !c.is_null());
    let pick = |rig: Value, meas: Value| {
        if kind == "spl" {
            tagged("meter", [("rig", rig), ("meas", meas)])
        } else if GRAPH_PANES.contains(&kind.as_str()) {
            tagged(
                "curves",
                [(
                    "entries",
                    Value::Array(vec![entry(rig, tagged("group", [("meas", meas)]))]),
                )],
            )
        } else {
            tagged("nothing", [])
        }
    };
    let content = match content {
        None => match owner.filter(|o| o.draws.contains(&kind.as_str())) {
            Some(o) => pick(this_rig(), o.meas.clone()),
            None => tagged("nothing", []),
        },
        Some(Value::Object(mut c)) => match c.get("type").and_then(Value::as_str) {
            Some("meas") => pick(rig, c.remove("meas").unwrap_or(Value::Null)),
            Some("inputs" | "pair" | "walk") => {
                c.insert("rig".into(), rig);
                Value::Object(c)
            }
            // Not a shape this step knows: left for the reader to refuse and name.
            _ => Value::Object(c),
        },
        Some(other) => other,
    };
    obj.insert("content".into(), content);
    let Some(o) = owner else { return 0 };
    if compared.is_empty() {
        return 0;
    }
    let own_group = entry(this_rig(), tagged("group", [("meas", o.meas.clone())]));
    let Some(Value::Array(entries)) = obj.get_mut("content").and_then(|c| c.get_mut("entries"))
    else {
        return 0;
    };
    if entries.first() != Some(&own_group) {
        return 0;
    }
    for c in compared {
        let what = match c.get("type").and_then(Value::as_str) {
            Some("meas") => tagged("live", [("meas", c["meas"].clone())]),
            Some("trace") => tagged("trace", [("trace", c["trace"].clone())]),
            _ => continue,
        };
        let e = entry(this_rig(), what);
        if !entries.contains(&e)
            && entries.len() < ac2_proto::model::layout::PaneContent::MAX_ENTRIES
        {
            entries.push(e);
        }
    }
    1
}

/// Gives every pane in the layout tree `v` without a `spectrum_camera` the default one: a
/// pane saved before the 3D spectrograph never showed it, so it has never been framed.
pub fn panes_spectrum_camera(v: &mut Value) {
    match v {
        Value::Object(obj) if is_pane(obj) => {
            obj.entry("spectrum_camera").or_insert_with(|| {
                serde_json::to_value(ac2_proto::model::layout::SpectrumCamera::DEFAULT)
                    .unwrap_or(Value::Null)
            });
        }
        Value::Object(obj) => obj.values_mut().for_each(panes_spectrum_camera),
        Value::Array(items) => items.iter_mut().for_each(panes_spectrum_camera),
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
        panes_curves(&mut v, None, &[]);
        panes_spectrum_camera(&mut v);
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

    fn pane40(kind: &str, content: Value, rig: Value) -> Value {
        json!({"type": "pane", "pane": {"kind": kind, "chrome": "full",
            "content": content, "rig": rig}})
    }

    fn group(meas: u64, rig: Value) -> Value {
        json!({"type": "curves", "entries": [{"rig": rig, "what": {"type": "group", "meas": meas}}]})
    }

    #[test]
    fn a_pick_names_its_rig_and_becomes_a_group_or_a_meter() {
        let pinned = json!({"type": "rig", "id": 7, "name": "ketunkolo"});
        let this = json!({"type": "this_rig"});
        let meas = |m: u64| json!({"type": "meas", "meas": m});
        let mut v = json!({"type": "split", "a": pane40("transfer", meas(3), this.clone()),
            "b": {"type": "split",
                "a": pane40("spl", meas(2), pinned.clone()),
                "b": {"type": "split",
                    "a": pane40("programme", json!({"type": "pair", "left": 0, "right": 1}),
                        pinned.clone()),
                    "b": pane40("walk", json!({"type": "walk", "walk": 4}), this.clone())}}});
        assert_eq!(panes_curves(&mut v, None, &[]), 0);
        assert_eq!(v["a"]["pane"]["content"], group(3, this.clone()));
        assert_eq!(
            v["b"]["a"]["pane"]["content"],
            json!({"type": "meter", "rig": pinned.clone(), "meas": 2})
        );
        assert_eq!(
            v["b"]["b"]["a"]["pane"]["content"],
            json!({"type": "pair", "left": 0, "right": 1, "rig": pinned})
        );
        assert_eq!(
            v["b"]["b"]["b"]["pane"]["content"],
            json!({"type": "walk", "walk": 4, "rig": this})
        );
        assert!(
            v["a"]["pane"].get("rig").is_none(),
            "the pane's rig is gone"
        );
    }

    #[test]
    fn no_pick_is_the_owner_where_it_draws_and_nothing_elsewhere() {
        let this = json!({"type": "this_rig"});
        let spl = ac2_proto::model::MeasKind::Spl {
            config: ac2_proto::model::SplConfig::on_input(
                0,
                ac2_proto::model::Weighting::A,
                ac2_proto::model::TimeWeighting::Fast,
            ),
        };
        let owner = LayoutOwner::of(json!(5), &spl);
        assert_eq!(owner.draws, vec!["spl"]);
        let mut v = json!({"type": "split", "a": pane40("spl", Value::Null, this.clone()),
            "b": pane40("transfer", Value::Null, this.clone())});
        panes_curves(&mut v, Some(&owner), &[]);
        assert_eq!(
            v["a"]["pane"]["content"],
            json!({"type": "meter", "rig": this, "meas": 5})
        );
        assert_eq!(v["b"]["pane"]["content"], json!({"type": "nothing"}));
    }

    #[test]
    fn a_compare_list_follows_its_group_in_the_owners_panes() {
        let this = json!({"type": "this_rig"});
        let pinned = json!({"type": "rig", "id": 7, "name": "ketunkolo"});
        let spectrum = ac2_proto::model::MeasKind::Spectrum {
            config: ac2_proto::model::SpectrumConfig::on_input(0),
        };
        let owner = LayoutOwner::of(json!(1), &spectrum);
        assert_eq!(owner.draws, vec!["spectrum"]);
        let compared = [
            json!({"type": "meas", "meas": 2}),
            json!({"type": "trace", "trace": 9}),
        ];
        let mut v = json!({"type": "split",
            "a": pane40("spectrum", Value::Null, this.clone()),
            "b": {"type": "split",
                "a": pane40("spectrum", json!({"type": "meas", "meas": 3}), this.clone()),
                "b": pane40("spectrum", json!({"type": "meas", "meas": 1}), pinned)}});
        assert_eq!(panes_curves(&mut v, Some(&owner), &compared), 1);
        assert_eq!(
            v["a"]["pane"]["content"]["entries"],
            json!([
                {"rig": this, "what": {"type": "group", "meas": 1}},
                {"rig": this, "what": {"type": "live", "meas": 2}},
                {"rig": this, "what": {"type": "trace", "trace": 9}},
            ])
        );
        // Another measurement's pane, and another rig's measurement 1, keep their group alone.
        assert_eq!(
            v["b"]["a"]["pane"]["content"]["entries"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            v["b"]["b"]["pane"]["content"]["entries"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
        let mut none = pane40("spectrum", json!({"type": "meas", "meas": 3}), this);
        assert_eq!(
            panes_curves(&mut none, Some(&owner), &compared),
            0,
            "no pane draws it"
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

    #[test]
    fn every_pane_gets_the_default_camera_and_keeps_its_own() {
        let mut v = json!({"type": "split", "a":
            {"type": "pane", "pane": {"kind": "spectrum", "chrome": "full"}},
            "b": {"type": "pane", "pane": {"kind": "spectrum", "chrome": "full",
                "spectrum_camera": {"yaw_deg": 10, "pitch_deg": 20, "zoom_step": 1}}}});
        panes_spectrum_camera(&mut v);
        assert_eq!(
            v["a"]["pane"]["spectrum_camera"],
            json!({"yaw_deg": 0, "pitch_deg": 8, "zoom_step": 0})
        );
        assert_eq!(
            v["b"]["pane"]["spectrum_camera"],
            json!({"yaw_deg": 10, "pitch_deg": 20, "zoom_step": 1})
        );
    }
}
