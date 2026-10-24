//! Reading a session of another format version, best effort and never silently: every
//! setting of this version the old file cannot fill, and every old setting this version has
//! no place for, is named in a note.
//!
//! The daemon reads one manifest version only (`ac2_traces::session`). This reader is the
//! client's: it builds the current typed values from the old JSON, so the daemon is only
//! ever given commands it checks as usual.

use std::collections::BTreeSet;
use std::path::Path;

use ac2_proto::model::{
    BandFraction, ColourFamily, MeasConfig, MeasKind, PaneLayout, RtaConfig, SpectrumConfig,
    SplConfig, TimeWeighting, TraceOwner, TransferConfig, Weighting,
};
use ac2_proto::units::{MeasId, TraceId};
use ac2_traces::session::{self, SavedDelay, SavedMeasurement};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// A session read leniently.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Plan {
    /// The manifest's format version, if it says.
    pub version: Option<u64>,
    /// Measurements, in manifest order.
    pub measurements: Vec<OldMeas>,
    /// Stored traces, in manifest order.
    pub traces: Vec<OldTrace>,
    /// Entries that could not be read at all.
    pub unread: Vec<Unread>,
    /// Per-second SPL logs the session kept.
    pub spl_logs: Vec<OldSplLog>,
}

/// A measurement as the old session had it, in this version's types.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OldMeas {
    /// Its id in the old session (math channels and traces name it).
    pub old_id: Option<MeasId>,
    /// Configuration.
    pub config: MeasConfig,
    /// Running when saved.
    pub running: bool,
    /// Applied delay and tracking of a transfer measurement.
    pub delay: Option<SavedDelay>,
    /// Colour family the operator chose.
    pub colour: Option<ColourFamily>,
    /// Its own pane layout.
    pub layout: Option<PaneLayout>,
    /// What was not read as saved.
    pub notes: Vec<String>,
}

/// A stored trace entry of the old manifest.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OldTrace {
    /// Its id in the old session (math channels name it).
    pub old_id: Option<TraceId>,
    /// Data file, relative to the session directory.
    pub file: String,
    /// Name, for the report.
    pub name: String,
    /// Owner, by old measurement id.
    pub owner: TraceOwner,
    /// The old editable properties, read onto the imported trace's.
    pub edit: Value,
    /// Label of a mic curve applied after capture.
    pub mic_curve: Option<String>,
    /// What was not read as saved.
    pub notes: Vec<String>,
}

/// A per-second SPL log the old session kept.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OldSplLog {
    /// Its SPL measurement, by old id.
    pub meas: Option<MeasId>,
    /// Log file, relative to the session directory.
    pub file: String,
}

/// An entry that could not be read at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unread {
    /// Which entry (`measurement 3`, a file name).
    pub what: String,
    /// Why.
    pub reason: String,
}

/// Reads the manifest of `dir`, any version.
pub(crate) fn read(dir: &Path) -> Result<Plan, String> {
    let path = dir.join(session::MANIFEST);
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let v: Value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("{}: not a session manifest: {e}", path.display()))?;
    if v.get("format").and_then(Value::as_str) != Some(session::FORMAT) {
        return Err(format!("{}: not an ac2 session", path.display()));
    }
    let mut plan = Plan {
        version: v.get("version").and_then(Value::as_u64),
        measurements: Vec::new(),
        traces: Vec::new(),
        unread: Vec::new(),
        spl_logs: Vec::new(),
    };
    for (i, m) in list(&v, "measurements").iter().enumerate() {
        match old_meas(m) {
            Ok(m) => plan.measurements.push(m),
            Err(reason) => plan.unread.push(Unread {
                what: format!("measurement {}", i + 1),
                reason,
            }),
        }
    }
    for (i, t) in list(&v, "traces").iter().enumerate() {
        match old_trace(t) {
            Ok(t) => plan.traces.push(t),
            Err(reason) => plan.unread.push(Unread {
                what: format!("trace {}", i + 1),
                reason,
            }),
        }
    }
    for l in list(&v, "spl_logs") {
        if let Some(file) = l.get("file").and_then(Value::as_str) {
            plan.spl_logs.push(OldSplLog {
                meas: l.get("meas").and_then(MeasId::deserialize_value),
                file: file.to_owned(),
            });
        }
    }
    // A trace file no entry names is one a save left behind (or an entry too broken to
    // name its file): its metadata is gone, so it is reported, not guessed at.
    let named: BTreeSet<&str> = plan.traces.iter().map(|t| t.file.as_str()).collect();
    if let Ok(rd) = std::fs::read_dir(dir.join("traces")) {
        let mut stray: Vec<String> = rd
            .flatten()
            .filter_map(|e| {
                let n = format!("traces/{}", e.file_name().to_string_lossy());
                (!named.contains(n.as_str())).then_some(n)
            })
            .collect();
        stray.sort();
        for f in stray {
            plan.unread.push(Unread {
                what: f,
                reason: "no trace entry of the manifest names it".into(),
            });
        }
    }
    Ok(plan)
}

fn list<'v>(v: &'v Value, key: &str) -> &'v [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

trait FromValue: Sized {
    fn deserialize_value(v: &Value) -> Option<Self>;
}

impl<T: DeserializeOwned> FromValue for T {
    fn deserialize_value(v: &Value) -> Option<Self> {
        T::deserialize(v).ok()
    }
}

/// The keys of a saved measurement in this version.
fn meas_keys() -> Vec<String> {
    let sample = SavedMeasurement {
        id: MeasId(0),
        config: MeasConfig {
            scope: ac2_proto::model::MeasScope::Session,
            name: String::new(),
            kind: MeasKind::Spectrum {
                config: SpectrumConfig::on_input(0),
            },
        },
        running: false,
        delay: None,
        colour: None,
        layout: None,
        compared: Vec::new(),
        offset: ac2_proto::units::Db(0.0),
        eq: Vec::new(),
    };
    match serde_json::to_value(sample) {
        Ok(Value::Object(m)) => m.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

fn old_meas(v: &Value) -> Result<OldMeas, String> {
    if let Some(m) = SavedMeasurement::deserialize_value(v) {
        return Ok(OldMeas {
            old_id: Some(m.id),
            config: m.config,
            running: m.running,
            delay: m.delay,
            colour: m.colour,
            layout: m.layout,
            notes: Vec::new(),
        });
    }
    let obj = v.as_object().ok_or("not an object")?;
    let mut notes = Vec::new();
    let config = obj.get("config").ok_or("no configuration")?;
    let config = meas_config(config, &mut notes)?;
    let old_id = obj.get("id").and_then(MeasId::deserialize_value);
    if old_id.is_none() {
        notes.push("its old id is unreadable: no trace or math channel finds it".into());
    }
    let running = match obj.get("running").and_then(Value::as_bool) {
        Some(r) => r,
        None => {
            notes.push("`running` unreadable: left stopped".into());
            false
        }
    };
    let optional = |key: &str| obj.get(key).filter(|v| !v.is_null());
    let delay = optional("delay").and_then(|d| lenient::<SavedDelay>(d, None, "delay", &mut notes));
    let colour =
        optional("colour").and_then(|d| lenient::<ColourFamily>(d, None, "colour", &mut notes));
    let layout =
        optional("layout").and_then(|d| lenient::<PaneLayout>(d, None, "layout", &mut notes));
    let known = meas_keys();
    for (k, v) in obj {
        if !known.contains(k) {
            notes.push(format!("`{k}` is not kept by this version: dropped ({v})"));
        }
    }
    Ok(OldMeas {
        old_id,
        config,
        running,
        delay,
        colour,
        layout,
        notes,
    })
}

/// A measurement configuration, field by field onto this version's default of its kind. A
/// sweep (whose level is never invented) and a math channel (whose expression has no
/// default) are taken whole or not at all.
fn meas_config(v: &Value, notes: &mut Vec<String>) -> Result<MeasConfig, String> {
    let name = v
        .get("name")
        .and_then(Value::as_str)
        .ok_or("no name")?
        .to_owned();
    let ty = v
        .pointer("/kind/type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let kind = match ty {
        "transfer" => Some(MeasKind::Transfer {
            config: TransferConfig::with_inputs(0, 1),
        }),
        "spectrum" => Some(MeasKind::Spectrum {
            config: SpectrumConfig::on_input(0),
        }),
        "rta" => Some(MeasKind::Rta {
            config: RtaConfig::on_input(0, BandFraction::Third),
        }),
        "spl" => Some(MeasKind::Spl {
            config: SplConfig::on_input(0, Weighting::A, TimeWeighting::Fast),
        }),
        _ => None,
    };
    let base = kind.map(|kind| MeasConfig {
        scope: ac2_proto::model::MeasScope::Session,
        name,
        kind,
    });
    // Every measurement of a format without `scope` was the session's: nothing to note.
    let mut v = v.clone();
    if let Some(o) = v.as_object_mut() {
        o.entry("scope").or_insert_with(|| Value::from("session"));
    }
    cue_messages_as_actions(&mut v);
    let mut own = Vec::new();
    let c = lenient(&v, base.as_ref(), "", &mut own);
    match c {
        Some(c) => {
            notes.extend(own);
            Ok(c)
        }
        None => Err(own.join("; ")),
    }
}

fn old_trace(v: &Value) -> Result<OldTrace, String> {
    let file = v
        .get("file")
        .and_then(Value::as_str)
        .ok_or("names no data file")?
        .to_owned();
    let meta = v.get("meta").unwrap_or(&Value::Null);
    let edit = meta.get("edit").cloned().unwrap_or(Value::Null);
    let mut notes = Vec::new();
    let owner = match edit.get("owner").and_then(TraceOwner::deserialize_value) {
        Some(o) => o,
        None => {
            notes.push("its owner is unreadable: filed under Imported".into());
            TraceOwner::Imported
        }
    };
    let mic_curve = match meta.get("mic_curve").filter(|m| !m.is_null()) {
        Some(m) => Some(
            m.pointer("/curve/label")
                .and_then(Value::as_str)
                .unwrap_or("a mic curve")
                .to_owned(),
        ),
        None => v
            .get("mic_curve_points")
            .filter(|p| !p.is_null())
            .map(|_| "a mic curve".to_owned()),
    };
    Ok(OldTrace {
        old_id: meta.get("id").and_then(TraceId::deserialize_value),
        name: edit
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&file)
            .to_owned(),
        file,
        owner,
        edit,
        mic_curve,
        notes,
    })
}

/// `old` as a `T`: whole if it fits; otherwise, with a `base`, each old field that fits
/// is put onto `base`, largest pieces first, and each that does not (and each setting the
/// old value lacks) is named in `notes`. `None` (with the reason in `notes`) when nothing
/// can be made of it.
pub(crate) fn lenient<T: DeserializeOwned + Serialize>(
    old: &Value,
    base: Option<&T>,
    at: &str,
    notes: &mut Vec<String>,
) -> Option<T> {
    let whole = match T::deserialize(old) {
        Ok(t) => return Some(t),
        Err(e) => e,
    };
    let Some(mut doc) = base.and_then(|b| serde_json::to_value(b).ok()) else {
        notes.push(format!("{}unreadable ({whole}): dropped", label(at, "")));
        return None;
    };
    let fits = |d: &Value| T::deserialize(d).is_ok();
    overlay(&mut doc, "", old, &fits, at, notes);
    T::deserialize(&doc).ok()
}

fn label(at: &str, ptr: &str) -> String {
    let p = format!("{at}{}", ptr.replace('/', "."));
    let p = p.trim_start_matches('.');
    if p.is_empty() {
        String::new()
    } else {
        format!("`{p}` ")
    }
}

fn child(ptr: &str, key: &str) -> String {
    format!("{ptr}/{}", key.replace('~', "~0").replace('/', "~1"))
}

/// Puts the fields of `old` onto the object or array at `ptr` of `doc`, each only if the
/// whole document still fits.
fn overlay(
    doc: &mut Value,
    ptr: &str,
    old: &Value,
    fits: &dyn Fn(&Value) -> bool,
    at: &str,
    notes: &mut Vec<String>,
) {
    let Some(cur) = doc.pointer(ptr).cloned() else {
        return;
    };
    match (&cur, old) {
        (Value::Object(cur), Value::Object(old)) => {
            // A tagged enum of another variant: its fields belong to that variant, so
            // none of them can be put onto this one.
            if cur.get("type").is_some() && cur.get("type") != old.get("type") {
                notes.push(format!(
                    "{}{} does not fit this version: kept {}",
                    label(at, ptr),
                    Value::Object(old.clone()),
                    Value::Object(cur.clone())
                ));
                return;
            }
            for (k, v) in old {
                let p = child(ptr, k);
                let prev = cur.get(k).cloned();
                set(doc, ptr, k, Some(v.clone()));
                if fits(doc) {
                    continue;
                }
                set(doc, ptr, k, prev.clone());
                if prev.is_none()
                    && let Some((nk, nv)) = moved(k, v)
                    && !old.contains_key(nk)
                {
                    let before = cur.get(nk).cloned();
                    set(doc, ptr, nk, Some(nv.clone()));
                    if fits(doc) {
                        continue;
                    }
                    set(doc, ptr, nk, before);
                }
                match prev {
                    Some(prev) if prev.is_object() || prev.is_array() => {
                        overlay(doc, &p, v, fits, at, notes);
                    }
                    Some(prev) => notes.push(format!(
                        "{}= {v} does not fit this version: kept {prev}",
                        label(at, &p)
                    )),
                    None => notes.push(format!(
                        "{}is not a setting of this version: dropped ({v})",
                        label(at, &p)
                    )),
                }
            }
            for (k, v) in cur {
                let moved_here = old
                    .iter()
                    .any(|(ok, ov)| moved(ok, ov).is_some_and(|(nk, _)| nk == k));
                if !old.contains_key(k) && !moved_here {
                    notes.push(format!(
                        "{}was not in the old session: {v} (this version's default)",
                        label(at, &child(ptr, k))
                    ));
                }
            }
        }
        (Value::Array(cur), Value::Array(old)) => {
            // Each old element onto this version's first element as its template.
            let Some(template) = cur.first() else {
                notes.push(format!(
                    "{}= {} does not fit this version: kept []",
                    label(at, ptr),
                    Value::Array(old.clone())
                ));
                return;
            };
            if let Some(slot) = doc.pointer_mut(ptr) {
                *slot = Value::Array(vec![template.clone(); old.len()]);
            }
            if !fits(doc) {
                if let Some(slot) = doc.pointer_mut(ptr) {
                    *slot = Value::Array(cur.clone());
                }
                notes.push(format!(
                    "{}= {} does not fit this version: kept {}",
                    label(at, ptr),
                    Value::Array(old.clone()),
                    Value::Array(cur.clone())
                ));
                return;
            }
            for (i, v) in old.iter().enumerate() {
                let p = format!("{ptr}/{i}");
                if let Some(slot) = doc.pointer_mut(&p) {
                    let prev = std::mem::replace(slot, v.clone());
                    if fits(doc) {
                        continue;
                    }
                    if let Some(slot) = doc.pointer_mut(&p) {
                        *slot = prev;
                    }
                }
                overlay(doc, &p, v, fits, at, notes);
            }
        }
        _ => notes.push(format!(
            "{}= {old} does not fit this version: kept {cur}",
            label(at, ptr)
        )),
    }
}

/// A cue of format 26 held one OSC message per state; this version holds an action (an OSC
/// message and DMX levels) per state. Every state that is an old message becomes an action
/// sending that message and holding no levels, wherever a `cue` sits in the config.
fn cue_messages_as_actions(v: &mut Value) {
    match v {
        Value::Object(o) => {
            for (k, child) in o.iter_mut() {
                if k == "cue"
                    && let Value::Object(cue) = child
                {
                    for state in ["ok", "near", "over", "fault"] {
                        if let Some(m) = cue.get_mut(state)
                            && m.get("path").is_some()
                        {
                            let msg = m.take();
                            *m = serde_json::json!({ "osc": msg, "dmx": [] });
                        }
                    }
                } else {
                    cue_messages_as_actions(child);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(cue_messages_as_actions),
        _ => {}
    }
}

/// Settings an earlier version kept under another key or shape, as this version has them.
/// Tried only where this version has no field of the old name.
fn moved(key: &str, v: &Value) -> Option<(&'static str, Value)> {
    match key {
        // A spectrum's one FFT length became a fixed resolution beside multi-resolution.
        "fft_len" => Some((
            "resolution",
            serde_json::json!({ "type": "fixed", "fft_len": v }),
        )),
        _ => None,
    }
}

/// Sets (or with `None` removes) `key` of the object at `ptr`.
fn set(doc: &mut Value, ptr: &str, key: &str, v: Option<Value>) {
    if let Some(Value::Object(o)) = doc.pointer_mut(ptr) {
        match v {
            Some(v) => {
                o.insert(key.to_owned(), v);
            }
            None => {
                o.remove(key);
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use ac2_proto::model::{MeasKind, SpecLevel, SpecResolution, SplConfig, Window};
    use serde_json::json;

    fn spl() -> MeasConfig {
        let c = SplConfig::on_input(2, Weighting::C, TimeWeighting::Slow);
        MeasConfig {
            scope: ac2_proto::model::MeasScope::Session,
            name: "FOH SPL".into(),
            kind: MeasKind::Spl { config: c },
        }
    }

    #[test]
    fn an_osc_cue_of_format_26_becomes_an_action_with_no_levels() {
        let c = spl();
        let msg = json!({ "path": "/light/red", "args": [{ "type": "int", "value": 1 }],
                          "append_level": true });
        let mut v = serde_json::to_value(&c).unwrap();
        let old_cue = json!({ "ok": null, "near": null, "over": msg, "fault": null });
        v["kind"]["config"]["cue"] = old_cue.clone();
        v["kind"]["config"]["leq"]["windows"][0]["cue"] = old_cue;
        let mut notes = Vec::new();
        let got = meas_config(&v, &mut notes).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        let MeasKind::Spl { config: got } = got.kind else {
            panic!("an spl meter")
        };
        let want = ac2_proto::model::CueAction {
            osc: Some(serde_json::from_value(msg).unwrap()),
            dmx: vec![],
        };
        for cue in [got.cue.as_deref(), got.leq.windows[0].cue.as_deref()] {
            let cue = cue.expect("the cue is kept");
            assert_eq!(cue.over.as_ref(), Some(&want));
            assert_eq!(cue.ok, None);
        }
    }

    /// A layout saved before panes carried a pick and level ranges reads whole: each pane
    /// follows its measurement, with no level range of the operator's.
    #[test]
    fn a_layout_of_format_27_reads_with_no_picks_or_levels() {
        let pane = json!({ "kind": "spectrum", "transfer_view": "magnitude",
                           "spectrum_view": "spectrograph", "spl_view": "meter_leq",
                           "sweep_view": "response", "chrome": "full" });
        let v = json!({ "id": 7, "config": serde_json::to_value(spl()).unwrap(),
                        "running": false, "delay": null, "colour": null,
                        "layout": { "type": "pane", "pane": pane } });
        let m = old_meas(&v).unwrap();
        assert!(m.notes.is_empty(), "{:?}", m.notes);
        let layout = m.layout.expect("the layout");
        let panes = layout.panes();
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].content, None);
        assert!(panes[0].levels.is_empty());
    }

    #[test]
    fn a_config_that_fits_is_taken_whole() {
        let c = spl();
        let mut notes = Vec::new();
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(meas_config(&v, &mut notes).unwrap(), c);
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn a_field_this_version_lacks_is_dropped_and_named() {
        let c = spl();
        let mut v = serde_json::to_value(&c).unwrap();
        v["kind"]["config"]["loudness_war"] = json!(true);
        let mut notes = Vec::new();
        assert_eq!(meas_config(&v, &mut notes).unwrap(), c);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].contains("`kind.config.loudness_war` is not a setting"),
            "{notes:?}"
        );
    }

    #[test]
    fn a_setting_the_old_file_lacks_takes_the_default_and_says_so() {
        let c = spl();
        let mut v = serde_json::to_value(&c).unwrap();
        v["kind"]["config"]
            .as_object_mut()
            .unwrap()
            .remove("weighting");
        let mut notes = Vec::new();
        let got = meas_config(&v, &mut notes).unwrap();
        let MeasKind::Spl { config } = got.kind else {
            panic!("{got:?}")
        };
        // Input and time weighting come from the old file; the weighting is the default.
        assert_eq!(config.input, 2);
        assert_eq!(config.time_weighting, TimeWeighting::Slow);
        assert_eq!(config.weighting, Weighting::A);
        assert!(
            notes
                .iter()
                .any(|n| n.contains("`kind.config.weighting` was not in the old session")),
            "{notes:?}"
        );
    }

    #[test]
    fn a_value_of_the_wrong_shape_keeps_the_default() {
        let c = spl();
        let mut v = serde_json::to_value(&c).unwrap();
        v["kind"]["config"]["input"] = json!("one");
        let mut notes = Vec::new();
        let got = meas_config(&v, &mut notes).unwrap();
        let MeasKind::Spl { config } = got.kind else {
            panic!("{got:?}")
        };
        assert_eq!(config.input, 0);
        assert_eq!(config.weighting, Weighting::C);
        assert!(notes[0].contains("does not fit this version"), "{notes:?}");
    }

    #[test]
    fn an_fft_length_of_format_23_is_a_fixed_resolution() {
        let v = json!({"name": "Spectrum 1", "kind": {"type": "spectrum", "config": {
            "input": 3, "fft_len": 4096, "window": "blackman_harris4", "averaging": {"type": "off"},
            "smoothing": "twenty_fourth"}}});
        let mut notes = Vec::new();
        let got = meas_config(&v, &mut notes).unwrap();
        let MeasKind::Spectrum { config } = got.kind else {
            panic!("{got:?}")
        };
        assert_eq!(config.resolution, SpecResolution::Fixed { fft_len: 4096 });
        assert_eq!(config.input, 3);
        assert_eq!(config.window, Window::BlackmanHarris4);
        // Before format 25 every spectrum read tone power: the level is filled in as
        // tone and named.
        assert_eq!(config.level, SpecLevel::Tone);
        // The display settings of format 28 take their defaults too.
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[0].contains("kind.config.display"), "{notes:?}");
        assert!(notes[1].contains("kind.config.level"), "{notes:?}");
    }

    #[test]
    fn a_measurement_of_format_27_draws_with_the_defaults() {
        // Format 27 kept no display settings, compare list or offset with a measurement.
        let mut config = serde_json::to_value(MeasConfig {
            scope: ac2_proto::model::MeasScope::Session,
            name: "Spectrum 1".into(),
            kind: MeasKind::Spectrum {
                config: SpectrumConfig::on_input(3),
            },
        })
        .unwrap();
        config["kind"]["config"]
            .as_object_mut()
            .unwrap()
            .remove("display");
        let v = json!({"id": 4, "config": config, "running": true, "delay": null,
                       "colour": null, "layout": null});
        let m = old_meas(&v).unwrap();
        assert_eq!(m.old_id, Some(MeasId(4)));
        assert!(m.running);
        assert_eq!(
            m.config.kind.display(),
            Some(&ac2_proto::model::SpectrumDisplay::default())
        );
        assert_eq!(m.notes.len(), 1, "{:?}", m.notes);
        assert!(m.notes[0].contains("kind.config.display"), "{:?}", m.notes);
    }

    #[test]
    fn a_sweep_is_never_rebuilt_from_defaults() {
        let v = json!({"name": "Sweep", "kind": {"type": "sweep", "config": {"level": -20.0}}});
        let mut notes = Vec::new();
        assert!(meas_config(&v, &mut notes).is_err());
    }
}
