//! `ac2 osc …`, `ac2 rig …` and `ac2 meas scope` against a real daemon on the simulated rig:
//! a meter made the rig's, its cues set and shown by name, the OSC target and the monitor
//! at start, and the warning a `session close` prints while a rig meter runs.
#![allow(clippy::unwrap_used)]

use std::net::UdpSocket;

use ac2_cli::{Cli, Out, run_reporting};
use ac2_client::Endpoints;
use ac2d::{BackendChoice, Daemon, DaemonConfig, Listen};
use clap::Parser;
use serde_json::Value;

/// Exit code, stdout as JSON, stderr.
async fn ac2(ep: &Endpoints, args: &[&str]) -> (u8, Value, String) {
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
    let so = String::from_utf8(so).unwrap();
    let se = String::from_utf8(se).unwrap();
    let v = serde_json::from_str(&so).unwrap_or_else(|e| panic!("{args:?}: {e}: {so} {se}"));
    (code, v, se)
}

async fn ok(ep: &Endpoints, args: &[&str]) -> (Value, String) {
    let (code, v, se) = ac2(ep, args).await;
    assert_eq!(code, 0, "{args:?}: {v} {se}");
    (v, se)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rig_meter_cues_status_and_close_warning() {
    let backend = ac2d::backend(BackendChoice::Fake).unwrap();
    let listen = Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    };
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = DaemonConfig::new(backend, listen, -10.0);
    cfg.rig_settings = Some(dir.path().join("rig.json"));
    cfg.rig_logs = Some(dir.path().join("rig-logs"));
    let h = Daemon::start(cfg).unwrap();
    let ep = Endpoints {
        ctrl: h.ctrl_endpoint().to_owned(),
        data: h.data_endpoint().to_owned(),
    };

    ok(
        &ep,
        &["session", "open", "--backend", "fake", "--in", "1-4"],
    )
    .await;
    ok(
        &ep,
        &["meas", "new", "spl", "--input", "1", "--name", "Stage"],
    )
    .await;
    ok(
        &ep,
        &[
            "spl",
            "leq",
            "set",
            "--meas",
            "Stage",
            "--windows",
            "1min,c:15min",
            "--limit",
            "1min=99db",
        ],
    )
    .await;
    let (m, _) = ok(&ep, &["meas", "scope", "Stage", "rig"]).await;
    assert_eq!(m["config"]["scope"], "rig", "{m}");

    // A cue on a window by name, then one more state on the same cue by index.
    let (q, _) = ok(
        &ep,
        &[
            "osc",
            "cue",
            "Stage",
            "window:1min",
            "--over",
            "/light/red i:1",
            "--ok",
            "/light/off",
            "--append-level",
        ],
    )
    .await;
    assert_eq!(q["label"], "LAeq 1 min", "{q}");
    assert_eq!(q["cue"]["over"]["path"], "/light/red", "{q}");
    assert_eq!(q["cue"]["over"]["args"][0]["value"], 1, "{q}");
    assert_eq!(q["cue"]["over"]["append_level"], true, "{q}");
    let (q, _) = ok(
        &ep,
        &[
            "osc",
            "cue",
            "Stage",
            "window:1",
            "--near",
            "/light/amber 0.5 stage",
        ],
    )
    .await;
    assert_eq!(q["cue"]["near"]["args"][0]["type"], "float", "{q}");
    assert_eq!(q["cue"]["near"]["args"][1]["value"], "stage", "{q}");
    assert_eq!(q["cue"]["over"]["path"], "/light/red", "kept: {q}");
    ok(
        &ep,
        &["osc", "cue", "Stage", "meter", "--fault", "/light/blink"],
    )
    .await;
    // No such window: named in words.
    let (code, e, _) = ac2(&ep, &["osc", "cue", "Stage", "window:30min", "--ok", "/x"]).await;
    assert_eq!(code, 1, "{e}");
    let msg = e["error"]["msg"].as_str().unwrap();
    assert!(msg.contains("no LAeq 30 min window"), "{msg}");
    assert!(msg.contains("LCeq 15 min"), "{msg}");

    // Target, monitor at start, test message.
    let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = rx.local_addr().unwrap().to_string();
    let (rig, _) = ok(&ep, &["osc", "target", &addr]).await;
    assert_eq!(rig["osc_target"], addr.as_str(), "{rig}");
    ok(&ep, &["rig", "monitor-at-start", "on"]).await;
    let (t, _) = ok(&ep, &["osc", "test", "/ping", "i:7", "-3", "s:hi"]).await;
    assert_eq!(t["sent"]["args"][1]["value"], -3, "{t}");

    let (st, _) = ok(&ep, &["rig", "status"]).await;
    assert_eq!(st["osc_target"], addr.as_str(), "{st}");
    assert_eq!(st["monitor_at_start"], true, "{st}");
    assert!(st["osc"]["sent"].as_u64().unwrap() >= 1, "{st}");
    let meters = st["meters"].as_array().unwrap();
    assert_eq!(meters.len(), 1, "{st}");
    assert_eq!(meters[0]["name"], "Stage");
    let subjects: Vec<&str> = meters[0]["cues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["subject"].as_str().unwrap())
        .collect();
    assert_eq!(subjects, ["meter", "LAeq 1 min"], "{st}");

    // Clearing one cue.
    let (q, _) = ok(&ep, &["osc", "cue", "Stage", "meter", "--clear"]).await;
    assert!(q["cue"].is_null(), "{q}");

    // Closing the session leaves the rig meter blind: the reply's warning, by name.
    let (_, se) = ok(&ep, &["session", "close"]).await;
    let w: Value = serde_json::from_str(se.trim()).unwrap_or_else(|e| panic!("{e}: {se}"));
    let w = w["warning"].as_str().unwrap();
    assert!(
        w.contains("Stage (1)") && w.contains("stop metering"),
        "{w}"
    );
    // Closed already: nothing to warn about.
    let (_, se) = ok(&ep, &["session", "close"]).await;
    assert!(se.is_empty(), "{se}");

    ok(&ep, &["osc", "target", "off"]).await;
    let (m, _) = ok(&ep, &["meas", "scope", "Stage", "session"]).await;
    assert_eq!(m["config"]["scope"], "session", "{m}");
    h.shutdown();
}
