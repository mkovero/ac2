//! What a real `ac2d` on the simulated rig answered, recorded in `fixtures/daemon/` by
//! `ac2-e2e` (whose quick tier checks the files still match the daemon). Regenerate with
//! `AC2_RECORD_FIXTURES=1 cargo nextest run -p ac2-e2e -E 'test(/^recorded::/)'`.
//!
//! Shared by the reducer's unit tests (`src/state/tests.rs`) and the integration tests.
#![allow(dead_code)]

use ac2_proto::GridDef;
use ac2_proto::model::{BackendInfo, LoopbackDetection, State, TraceData};
use serde::de::DeserializeOwned;

/// Fixture `name`'s value. A fixture recorded at another protocol version describes a
/// daemon that no longer exists: that fails here, by name, rather than as a test that
/// misreads the state it was given.
fn load<T: DeserializeOwned>(name: &str) -> T {
    #[derive(serde::Deserialize)]
    struct Doc {
        proto_version: u16,
        value: serde_json::Value,
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/daemon")
        .join(name);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("fixtures/daemon/{name}: {e}"));
    let doc: Doc =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("fixtures/daemon/{name}: {e}"));
    assert_eq!(
        doc.proto_version,
        ac2_proto::PROTO_VERSION,
        "fixtures/daemon/{name} was recorded at protocol {}, this build speaks {}: regenerate \
         with AC2_RECORD_FIXTURES=1 cargo nextest run -p ac2-e2e -E 'test(/^recorded::/)'",
        doc.proto_version,
        ac2_proto::PROTO_VERSION,
    );
    let mut value = doc.value;
    rebase(&mut value, wall_ns());
    serde_json::from_value(value).unwrap_or_else(|e| panic!("fixtures/daemon/{name}: {e}"))
}

fn wall_ns() -> i128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i128)
}

/// Each instant recorded as `"now-<ns>"` (how long before the recording's end it was) put
/// that long before `now`: a session opened a minute before the recording reads as opened a
/// minute ago, whenever the test runs.
fn rebase(v: &mut serde_json::Value, now: i128) {
    use serde_json::Value;
    match v {
        Value::String(s) => {
            if let Some(ago) = s.strip_prefix("now-").and_then(|n| n.parse::<i128>().ok()) {
                let at = u64::try_from(now - ago).unwrap_or(0);
                *v = Value::from(at);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| rebase(x, now)),
        Value::Object(m) => m.values_mut().for_each(|x| rebase(x, now)),
        _ => {}
    }
}

/// The state of a daemon just started, with no session (as on real audio).
pub fn empty_state() -> State {
    load("empty_state.json")
}

/// `session.devices` on the simulated rig: its one backend and device.
pub fn backends() -> Vec<BackendInfo> {
    load("session_devices.json")
}

/// `session.detect_loopback` on the simulated rig, out 1 at −30 dBFS: in 1 is the loopback.
pub fn detection() -> LoopbackDetection {
    load("detect_loopback.json")
}

/// A rig built with real commands on the simulated rig (`ac2-e2e`'s `recorded/scenes.rs`):
/// the daemon's state, the grids its measurements and traces are on, every trace's data.
pub struct Scene {
    pub state: State,
    pub grids: Vec<GridDef>,
    pub traces: Vec<TraceData>,
}

/// Scene `name` (`fixtures/daemon/rig_<name>.json`).
pub fn scene(name: &str) -> Scene {
    #[derive(serde::Deserialize)]
    struct Raw {
        state: State,
        grids: Vec<GridDef>,
        traces: Vec<serde_json::Value>,
    }
    let raw: Raw = load(&format!("rig_{name}.json"));
    Scene {
        state: raw.state,
        grids: raw.grids,
        traces: raw.traces.into_iter().map(trace).collect(),
    }
}

/// A trace's data as JSON holds it: a column's bins with no value (NaN, past a grid's top
/// or where the curve is undefined) are `null`, which JSON has in place of NaN.
fn trace(mut v: serde_json::Value) -> TraceData {
    let columns = ["mag_db", "phase_deg", "coherence"];
    let mut taken: Vec<Option<Vec<f32>>> = Vec::new();
    for c in columns {
        let col: Option<Vec<Option<f32>>> =
            serde_json::from_value(v[c].take()).expect("a trace column");
        taken.push(col.map(|x| x.into_iter().map(|b| b.unwrap_or(f32::NAN)).collect()));
        v[c] = serde_json::Value::Array(Vec::new());
    }
    v["mag_db"] = serde_json::json!([]);
    let mut t: TraceData = serde_json::from_value(v).expect("a trace");
    let [mag, phase, coherence]: [Option<Vec<f32>>; 3] = taken.try_into().expect("three columns");
    t.mag_db = mag.unwrap_or_default();
    t.phase_deg = phase;
    t.coherence = coherence;
    t
}
