//! What a real `ac2d` on the simulated rig answers, recorded for the app's tests that need
//! daemon state but no daemon (`ac2-ui` links none): `fixtures/daemon/*.json`, each the value
//! wrapped with the `PROTO_VERSION` it was recorded at.
//!
//! The quick tier records again and compares with the checked-in files, so a daemon change
//! that alters what the app is fed fails here, not as a silently stale picture. After an
//! intended change, regenerate every fixture with
//!
//! ```text
//! AC2_RECORD_FIXTURES=1 cargo nextest run -p ac2-e2e -E 'test(/^recorded::/)'
//! ```

use std::path::PathBuf;

use ac2_e2e::{Setup, sim_daemon};
use ac2_proto::model::{BackendInfo, BackendKind, LoopbackDetection, State};
use ac2_proto::units::Dbfs;
use ac2_proto::{Command, ReplyBody};
use serde::Serialize;
use serde_json::Value;

use crate::harness::{Cli, R};

mod scenes;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/daemon")
}

/// `value` as its fixture file holds it: under the protocol version it was recorded at.
fn doc<T: Serialize>(value: &T) -> R<Value> {
    Ok(serde_json::json!({
        "proto_version": ac2_proto::PROTO_VERSION,
        "value": round_measured(serde_json::to_value(value)?),
    }))
}

/// What the machine recording puts in a state, not the daemon, pinned so every host records
/// the same:
/// - the system clock, whatever the operating system states (Linux: synced or not, with the
///   kernel's error bound; macOS and Windows: nothing);
/// - whether the host has a battery-backed clock, and the host's time zone with the offset
///   in force;
/// - a local client's tag, the routing id libzmq gave its connection, which starts from the
///   C library's random generator and so differs by platform.
fn pin_host(mut v: Value) -> Value {
    fn walk(v: &mut Value) {
        match v {
            Value::Object(m) => {
                for (k, x) in m.iter_mut() {
                    match x {
                        Value::String(s) if k == "client" && s.starts_with("local-") => {
                            *s = "local-006b8b4567".to_owned();
                        }
                        x => walk(x),
                    }
                }
            }
            Value::Array(a) => a.iter_mut().for_each(walk),
            _ => {}
        }
    }
    if let Some(e) = v.pointer_mut("/clocks/system") {
        *e = serde_json::json!({"type": "synced", "maxerror_us": 1000});
    }
    if let Some(e) = v.pointer_mut("/clocks/rtc") {
        *e = Value::Bool(true);
    }
    if let Some(e) = v.pointer_mut("/rig/zone") {
        *e = serde_json::json!({"name": "Europe/Helsinki", "utc_offset_s": 7200});
    }
    walk(&mut v);
    v
}

/// Wall-clock instants (`…_at`, `since`, `at`: nanoseconds since 1970) as how long before
/// `now` they were, `"now-<ns>"`: the app reads them against its own clock (a stream open
/// for 3 h, calibrated yesterday), so a test rebases them on its own now
/// (`ac2-ui/tests/it/recorded.rs`).
fn pin_times(mut v: Value, now: i128) -> Value {
    fn walk(v: &mut Value, now: i128) {
        match v {
            Value::Object(m) => {
                for (k, x) in m.iter_mut() {
                    let instant = k.ends_with("_at") || k == "since" || k == "at";
                    // A `WallNs` is a decimal string in JSON.
                    match x.as_str().and_then(|s| s.parse::<u64>().ok()) {
                        // Zero is "never" (a fresh rig's unset instants), not 1970.
                        Some(t) if instant && t > 0 => {
                            *x = Value::String(format!("now-{}", now - i128::from(t)));
                        }
                        _ => walk(x, now),
                    }
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| walk(x, now)),
            _ => {}
        }
    }
    walk(&mut v, now);
    v
}

/// Measured on the simulated rig's noise, so different every run: how close two recordings
/// of one measured quantity must be, and the decimals a fixture keeps of it.
fn measured(key: &str) -> Option<(f64, i32)> {
    match key {
        // Normalised cross-correlation of a burst: a weak path moves by about 0.01.
        "correlation" => Some((0.05, 3)),
        // A path's gain in dB: noise moves a weak one by about 0.2 dB.
        "gain" => Some((1.0, 1)),
        // A calibration's reading of the calibrator, and the sensitivity it gives, dB: the
        // rig's noise moves them by about 0.001 dB.
        "measured" | "sensitivity" => Some((0.05, 2)),
        _ => None,
    }
}

/// `v` with each measured number rounded to the decimals its fixture keeps.
fn round_measured(mut v: Value) -> Value {
    fn walk(v: &mut Value) {
        match v {
            Value::Object(m) => {
                for (k, x) in m.iter_mut() {
                    match (measured(k), x.as_f64()) {
                        (Some((_, dp)), Some(f)) => {
                            let p = 10f64.powi(dp);
                            // `+ 0.0` turns a rounded −0 into 0, so the sign of noise
                            // around zero does not show in the file.
                            *x = Value::from((f * p).round() / p + 0.0);
                        }
                        _ => walk(x),
                    }
                }
            }
            Value::Array(a) => a.iter_mut().for_each(walk),
            _ => {}
        }
    }
    walk(&mut v);
    v
}

/// `a` and `b` alike, measured numbers within their tolerance; the first difference's path.
fn differs(a: &Value, b: &Value, at: &str) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            if x.len() != y.len() {
                return Some(format!("{at}: keys {:?} vs {:?}", x.keys(), y.keys()));
            }
            x.iter().find_map(|(k, va)| {
                let path = format!("{at}/{k}");
                let Some(vb) = y.get(k) else {
                    return Some(format!("{path}: missing"));
                };
                match (measured(k), va.as_f64(), vb.as_f64()) {
                    (Some((tol, _)), Some(fa), Some(fb)) => {
                        ((fa - fb).abs() > tol).then(|| format!("{path}: {fa} vs {fb}"))
                    }
                    _ => differs(va, vb, &path),
                }
            })
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => x
            .iter()
            .zip(y)
            .enumerate()
            .find_map(|(i, (va, vb))| differs(va, vb, &format!("{at}/{i}"))),
        // Two recordings take a different few milliseconds between an event and the
        // snapshot: a relative instant is only checked to be one.
        (Value::String(x), Value::String(y)) if x.starts_with("now-") && y.starts_with("now-") => {
            None
        }
        _ => (a != b).then(|| format!("{at}: {a} vs {b}")),
    }
}

/// Records every fixture from a fresh daemon with no stream (as on real audio).
fn record() -> R<Vec<(&'static str, Value)>> {
    let d = sim_daemon(Setup::Empty)?;
    let cli = Cli::connect(d.endpoints())?;
    let state: State = (*cli.state().ok_or("no state")?).clone();
    let backends = match cli.call(Command::StreamDevices)? {
        ReplyBody::Backends(b) => b,
        other => return Err(format!("stream.devices: {other:?}").into()),
    };
    let device = only_fake(&backends)?;
    // A quiet burst on the simulated rig (never real audio): out 1 returns on in 1.
    let lease = cli.block_on(cli_lease(&cli))?;
    let detection = match cli.call(Command::StreamDetectLoopback {
        lease_token: lease.token(),
        backend: BackendKind::Fake,
        input_device: device.clone(),
        output_device: device,
        output: 0,
        level: Dbfs(-30.0),
    })? {
        ReplyBody::LoopbackDetection(x) => x,
        other => return Err(format!("stream.detect_loopback: {other:?}").into()),
    };
    drop(lease);
    Ok(vec![
        (
            "empty_state.json",
            doc(&pin_host(serde_json::to_value(&state)?))?,
        ),
        ("stream_devices.json", doc(&backends)?),
        ("detect_loopback.json", doc(&detection)?),
    ])
}

async fn cli_lease(cli: &Cli) -> R<ac2_client::StimulusLease> {
    Ok(cli
        .client()
        .acquire_lease(false, ac2_client::OnDrop::StopAndRelease)
        .await?)
}

/// The simulated rig's one device: the daemon here runs no other backend.
fn only_fake(b: &[BackendInfo]) -> R<ac2_proto::model::DeviceId> {
    match b {
        [one] if one.kind == BackendKind::Fake && one.devices.len() == 1 => {
            Ok(one.devices[0].id.clone())
        }
        _ => Err(format!("expected the simulated rig alone: {b:?}").into()),
    }
}

/// `fixtures` compared with the checked-in files; under `AC2_RECORD_FIXTURES=1`, each that
/// differs is rewritten (one that differs only by the rig's noise is left as it is, so a
/// regeneration changes only what the daemon changed).
fn check(fixtures: Vec<(String, Value)>) -> R {
    let rewrite = std::env::var("AC2_RECORD_FIXTURES").is_ok_and(|v| v == "1");
    let mut stale = Vec::new();
    for (name, now) in fixtures {
        let path = dir().join(&name);
        let kept: Option<Value> = match std::fs::read_to_string(&path) {
            Ok(t) => Some(serde_json::from_str(&t)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let differs = match &kept {
            Some(kept) => differs(kept, &now, ""),
            None => Some("missing".to_owned()),
        };
        match (differs, rewrite) {
            (None, _) => {}
            (Some(_), true) => {
                std::fs::create_dir_all(dir())?;
                std::fs::write(&path, serde_json::to_string_pretty(&now)? + "\n")?;
            }
            (Some(d), false) => stale.push(format!("{name}: {d}")),
        }
    }
    assert!(
        stale.is_empty(),
        "fixtures/daemon no longer match the daemon ({stale:?}); regenerate with \
         AC2_RECORD_FIXTURES=1 cargo nextest run -p ac2-e2e -E 'test(/^recorded::/)'"
    );
    Ok(())
}

/// The fixtures are what the daemon answers now; under `AC2_RECORD_FIXTURES=1`, rewritten.
#[test]
fn fixtures_match_the_daemon() -> R {
    check(
        record()?
            .into_iter()
            .map(|(n, v)| (n.to_owned(), v))
            .collect(),
    )
}

/// Scene `name` built by `build` on a fresh simulated rig and saved as "friday": the
/// daemon's state once saved, the grids its measurements and traces are on and the data of
/// every stored trace (an EQ set's curve), as `fixtures/daemon/rig_<name>.json`.
fn scene(name: &str, build: fn(&Cli) -> R) -> R {
    let sessions = tempfile::tempdir()?;
    let d = ac2_e2e::sim_daemon_with(Setup::Empty, |c| {
        c.session_dir = Some(sessions.path().to_owned());
    })?;
    let cli = Cli::connect(d.endpoints())?;
    build(&cli)?;
    // Saved, as an operator's rig is between changes: an unsaved session's label takes the
    // top bar's room first and would be in every picture.
    cli.call(Command::FileSave {
        session: ac2_proto::model::SessionRef::Name {
            name: "friday".into(),
        },
        overwrite: false,
    })?;
    let snap = cli.block_on(cli.client().snapshot())?;
    let state = snap.state;
    let mut grids = Vec::new();
    let traced = state.traces.iter().map(|t| t.grid_id);
    for id in state
        .measurements
        .iter()
        .filter_map(|m| m.grid_id)
        .chain(traced)
    {
        let g = cli.block_on(cli.client().grid(id))?;
        if !grids.contains(&*g) {
            grids.push((*g).clone());
        }
    }
    let mut traces = Vec::new();
    for t in &state.traces {
        match cli.call(Command::TraceGet { trace: t.id })? {
            ReplyBody::TraceData(data) => traces.push(*data),
            other => return Err(format!("trace.get: {other:?}").into()),
        }
    }
    let value = pin_times(
        serde_json::json!({
            "state": pin_host(serde_json::to_value(&state)?),
            "grids": grids,
            "traces": traces,
        }),
        wall_ns(),
    );
    check(vec![(format!("rig_{name}.json"), doc(&value)?)])
}

fn wall_ns() -> i128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i128)
}

#[test]
fn rig() -> R {
    scene("base", scenes::rig)
}

#[test]
fn rig_math_average() -> R {
    scene("math_average", scenes::math_average)
}

#[test]
fn rig_spectrum_math() -> R {
    scene("spectrum_math", scenes::spectrum_math)
}

#[test]
fn rig_alignment() -> R {
    scene("alignment", scenes::alignment)
}

#[test]
fn rig_walk() -> R {
    scene("walk", scenes::walk)
}

#[test]
fn rig_eq() -> R {
    scene("eq", scenes::eq)
}

#[test]
fn rig_monitor() -> R {
    scene("monitor", scenes::rig_monitor)
}

#[test]
fn rig_calibrations() -> R {
    scene("calibrations", scenes::calibrations)
}

/// Each fixture parses back into the type it was recorded from (what `ac2-ui` reads; the
/// scenes, whose instants it rebases first, are parsed by every picture of them).
#[test]
fn fixtures_parse() -> R {
    fn value<T: serde::de::DeserializeOwned>(name: &str) -> R<T> {
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(dir().join(name))?)?;
        Ok(serde_json::from_value(doc["value"].clone())?)
    }
    let _: State = value("empty_state.json")?;
    let _: Vec<BackendInfo> = value("stream_devices.json")?;
    let _: LoopbackDetection = value("detect_loopback.json")?;
    Ok(())
}
