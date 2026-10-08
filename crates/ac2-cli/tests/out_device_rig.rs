//! `ac2 session open` with the playback on another device than the capture: a simulated rig
//! listed as an input-only and an output-only endpoint (as WASAPI lists one interface).
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use ac2_audio::fake::{FAKE_INPUT_ID, FAKE_OUTPUT_ID, FakeDrive};
use ac2_audio::{FakeBackend, FakeConfig, FakeEndpoints};
use ac2_cli::{Cli, Out, run_reporting};
use ac2_client::Endpoints;
use ac2d::{Daemon, DaemonConfig, Listen};
use clap::Parser;

async fn ac2(ep: &Endpoints, args: &[&str]) -> (u8, String, String) {
    let mut argv = vec![
        "ac2",
        "--ctrl-endpoint",
        &ep.ctrl,
        "--data-endpoint",
        &ep.data,
    ];
    argv.extend_from_slice(args);
    let cli = Cli::try_parse_from(argv).unwrap();
    let (mut so, mut se) = (Vec::new(), Vec::new());
    let code = {
        let mut out = Out::new(false, &mut so);
        run_reporting(&cli, &mut out, &mut se).await
    };
    (
        code,
        String::from_utf8(so).unwrap(),
        String::from_utf8(se).unwrap(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_open_plays_on_the_out_device() {
    let backend = FakeBackend::new(FakeConfig {
        drive: FakeDrive::Manual,
        endpoints: FakeEndpoints::Split,
        ..FakeConfig::default()
    })
    .unwrap();
    let listen = Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    };
    let h = Daemon::start(DaemonConfig::new(Arc::new(backend), listen, -10.0)).unwrap();
    let ep = Endpoints {
        ctrl: h.ctrl_endpoint().to_owned(),
        data: h.data_endpoint().to_owned(),
    };

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

    // Named explicitly, by id or by name.
    let (code, text, err) = ac2(&ep, &open(&["--out-device", FAKE_OUTPUT_ID])).await;
    assert_eq!(code, 0, "{text} {err}");
    assert!(
        text.contains(&format!("output {FAKE_OUTPUT_ID} × 2")),
        "{text}"
    );
    let (code, text, err) = ac2(
        &ep,
        &open(&[
            "--out-device",
            "simulated output endpoint",
            "--outputs",
            "1",
        ]),
    )
    .await;
    assert_eq!(code, 0, "{text} {err}");
    assert!(
        text.contains(&format!("output {FAKE_OUTPUT_ID} × 1")),
        "{text}"
    );

    // The capture endpoint named as the output: refused before the daemon is asked.
    let (code, text, err) = ac2(&ep, &open(&["--out-device", FAKE_INPUT_ID])).await;
    assert_ne!(code, 0, "{text}");
    assert!(err.contains("has no outputs"), "{err}");
    h.shutdown();
}
