//! `ac2 session open` with the playback on another device than the capture: a simulated rig
//! listed as an input-only and an output-only device (as an OS lists a built-in mic and
//! speakers).
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use ac2_audio::fake::{FAKE_INPUT_ID, FAKE_OUTPUT_ID, FAKE_SPLIT_NAME, FakeDrive};
use ac2_audio::{FakeBackend, FakeConfig, FakeEndpoints};
use ac2_client::Endpoints;

use super::common::{self, rig_on};

async fn ac2(ep: &Endpoints, args: &[&str]) -> (u8, String, String) {
    let r = common::ac2(ep, false, args).await;
    (r.code, r.stdout, r.stderr)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_open_plays_on_the_out_device() {
    let backend = FakeBackend::new(FakeConfig {
        drive: FakeDrive::Manual,
        endpoints: FakeEndpoints::Split,
        ..FakeConfig::default()
    })
    .unwrap();
    let (h, ep) = rig_on(Arc::new(backend), |_| {});

    let (code, devices, _) = ac2(&ep, &["devices"]).await;
    assert_eq!(code, 0);
    assert!(devices.contains(FAKE_INPUT_ID) && devices.contains(FAKE_OUTPUT_ID));
    assert!(devices.contains("(default)"), "{devices}");

    // Without --out-device the capture endpoint has no outputs: the system default output
    // plays.
    let open = |extra: &'static [&'static str]| {
        let mut a = vec![
            "session",
            "open",
            "--backend",
            "fake",
            "--device",
            FAKE_INPUT_ID,
            "--in",
            "1-2",
        ];
        a.extend_from_slice(extra);
        a
    };
    let (code, text, err) = ac2(&ep, &open(&[])).await;
    assert_eq!(code, 0, "{text} {err}");
    assert!(
        text.contains(&format!("output {FAKE_OUTPUT_ID} × 2")),
        "{text}"
    );
    assert!(text.contains("another device"), "{text}");

    // Nothing named: the system default capture and playback endpoints, not a refusal
    // for listing more than one device.
    let (code, text, err) = ac2(
        &ep,
        &["session", "open", "--backend", "fake", "--in", "1-2"],
    )
    .await;
    assert_eq!(code, 0, "{text} {err}");
    assert!(text.contains(FAKE_INPUT_ID), "{text}");
    assert!(
        text.contains(&format!("output {FAKE_OUTPUT_ID} × 2")),
        "{text}"
    );

    // Named explicitly, by id or by name.
    let (code, text, err) = ac2(&ep, &open(&["--out-device", FAKE_OUTPUT_ID])).await;
    assert_eq!(code, 0, "{text} {err}");
    assert!(
        text.contains(&format!("output {FAKE_OUTPUT_ID} × 2")),
        "{text}"
    );
    let (code, text, err) = ac2(
        &ep,
        &open(&["--out-device", FAKE_SPLIT_NAME, "--outputs", "1"]),
    )
    .await;
    assert_eq!(code, 0, "{text} {err}");
    assert!(
        text.contains(&format!("output {FAKE_OUTPUT_ID} × 1")),
        "{text}"
    );

    // Both endpoints share one name: each direction finds its own.
    let (code, text, err) = ac2(
        &ep,
        &[
            "session",
            "open",
            "--backend",
            "fake",
            "--device",
            FAKE_SPLIT_NAME,
            "--out-device",
            FAKE_SPLIT_NAME,
            "--in",
            "1-2",
        ],
    )
    .await;
    assert_eq!(code, 0, "{text} {err}");
    assert!(
        text.contains(&format!("output {FAKE_OUTPUT_ID} × 2")),
        "{text}"
    );

    // The capture endpoint named as the output: refused before the daemon is asked.
    let (code, text, err) = ac2(&ep, &open(&["--out-device", FAKE_INPUT_ID])).await;
    assert_ne!(code, 0, "{text}");
    assert!(err.contains("has no playback side"), "{err}");
    h.shutdown();
}
