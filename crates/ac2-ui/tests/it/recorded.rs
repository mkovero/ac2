//! What a real `ac2d` on the simulated rig answered, recorded in `fixtures/daemon/` by
//! `ac2-e2e` (whose quick tier checks the files still match the daemon). Regenerate with
//! `AC2_RECORD_FIXTURES=1 cargo nextest run -p ac2-e2e -E 'test(/^recorded::/)'`.
//!
//! Shared by the reducer's unit tests (`src/state/tests.rs`) and the integration tests.
#![allow(dead_code)]

use ac2_proto::model::{BackendInfo, LoopbackDetection, State};
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
    serde_json::from_value(doc.value).unwrap_or_else(|e| panic!("fixtures/daemon/{name}: {e}"))
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
