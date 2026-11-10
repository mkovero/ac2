//! `ac2 selftest frames` against a real daemon on the fake backend: every frame of a running
//! spectrum is logged with its spacing, age and hop, then a summary that adds up.
#![allow(clippy::unwrap_used)]

use ac2_cli::{Cli, Out, run_reporting};
use ac2_client::Endpoints;
use ac2d::{BackendChoice, Daemon, DaemonConfig, Listen};
use clap::Parser;
use serde_json::Value;

async fn ac2(ep: &Endpoints, args: &[&str]) -> (u8, String) {
    let argv: Vec<&str> = [
        "ac2",
        "--json",
        "--ctrl-endpoint",
        &ep.ctrl,
        "--data-endpoint",
        &ep.data,
    ]
    .into_iter()
    .chain(args.iter().copied())
    .collect();
    let cli = Cli::try_parse_from(argv).unwrap();
    let (mut so, mut se) = (Vec::new(), Vec::new());
    let code = {
        let mut out = Out::new(cli.json, &mut so);
        run_reporting(&cli, &mut out, &mut se).await
    };
    let text = format!(
        "{}{}",
        String::from_utf8(so).unwrap(),
        String::from_utf8_lossy(&se)
    );
    (code, text)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn selftest_frames_logs_every_frame_of_a_spectrum() {
    let backend = ac2d::backend(BackendChoice::Fake).unwrap();
    let listen = Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    };
    let h = Daemon::start(DaemonConfig::new(backend, listen, -10.0)).unwrap();
    let ep = Endpoints {
        ctrl: h.ctrl_endpoint().to_owned(),
        data: h.data_endpoint().to_owned(),
    };
    for args in [
        &["session", "open", "--backend", "fake", "--in", "1-2"][..],
        &[
            "meas", "new", "spectrum", "--input", "1", "--name", "Spec", "--start",
        ][..],
    ] {
        let (code, text) = ac2(&ep, args).await;
        assert_eq!(code, 0, "{args:?}: {text}");
    }
    let (_, before) = ac2(&ep, &["state", "dump"]).await;

    let (code, text) = ac2(
        &ep,
        &["selftest", "frames", "--meas", "Spec", "--seconds", "2"],
    )
    .await;
    assert_eq!(code, 0, "{text}");
    let lines: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}")))
        .collect();
    let (last, frames) = lines.split_last().unwrap();
    assert!(frames.iter().all(|f| f["frame"].is_object()), "{text}");
    let s = &last["summary"];
    let n = s["frames"].as_u64().unwrap();
    assert!(n >= 5, "{s}");
    assert_eq!(
        frames.len() as u64,
        n + s["repeats"].as_u64().unwrap() + s["unapplied"].as_u64().unwrap(),
        "{s}"
    );
    assert!(s["interval_ms"]["p50"].as_f64().unwrap() > 0.0, "{s}");
    assert!(s["hop_ms"]["p50"].as_f64().unwrap() > 0.0, "{s}");
    assert!(s["age_ms"]["max"].as_f64().unwrap() < 1000.0, "{s}");
    assert!(s["mbit_s"].as_f64().unwrap() > 0.0, "{s}");
    assert_eq!(last["meas"], "Spec");
    assert!(last["note"].as_str().unwrap().contains("clock"), "{last}");

    // Read-only: the daemon's state is as it was.
    let (_, after) = ac2(&ep, &["state", "dump"]).await;
    assert_eq!(strip_volatile(&before), strip_volatile(&after));

    // A missing name is refused before anything is subscribed.
    let (code, text) = ac2(&ep, &["selftest", "frames", "--meas", "Nobody"]).await;
    assert_eq!(code, 1, "{text}");
}

/// The measurements of a state dump: the rest (clocks, autosave) moves on its own while the
/// meter runs.
fn strip_volatile(dump: &str) -> Value {
    let v: Value = serde_json::from_str(dump).unwrap();
    assert!(v["state"]["measurements"].is_array(), "{v}");
    v["state"]["measurements"].clone()
}
