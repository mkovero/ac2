//! `ac2 rec …` and `ac2 session replay` against a real daemon on the simulated rig: a
//! recording bounded by its time limit ends by itself, is listed, and replays as a session
//! on the recorded inputs; a recorder's WAV imported beside it does too.
#![allow(clippy::unwrap_used)]

use std::time::{Duration, Instant};

use ac2_cli::{Cli, Out, run_reporting};
use ac2_client::Endpoints;
use ac2d::{BackendChoice, Daemon, DaemonConfig, Listen};
use clap::Parser;
use serde_json::Value;

async fn ac2(ep: &Endpoints, args: &[&str]) -> (u8, String, String) {
    let argv: Vec<&str> = [
        "ac2",
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
    (
        code,
        String::from_utf8(so).unwrap(),
        String::from_utf8(se).unwrap(),
    )
}

async fn json(ep: &Endpoints, args: &[&str]) -> Value {
    let mut a = vec!["--json"];
    a.extend_from_slice(args);
    let (code, out, err) = ac2(ep, &a).await;
    assert_eq!(code, 0, "{args:?}: {out} {err}");
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("{args:?}: {e}: {out}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn record_list_and_replay() {
    let dir = tempfile::tempdir().unwrap();
    let backend = ac2d::backend(BackendChoice::Fake).unwrap();
    let listen = Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    };
    let mut cfg = DaemonConfig::new(backend, listen, -10.0);
    cfg.recording_dir = Some(dir.path().to_owned());
    let h = Daemon::start(cfg).unwrap();
    let ep = Endpoints {
        ctrl: h.ctrl_endpoint().to_owned(),
        data: h.data_endpoint().to_owned(),
    };

    // Without a session there is nothing to record.
    let (code, _, err) = ac2(&ep, &["rec", "start", "--max", "1s"]).await;
    assert_ne!(code, 0);
    assert!(err.contains("session"), "{err}");

    json(
        &ep,
        &["session", "open", "--backend", "fake", "--in", "1,2"],
    )
    .await;
    let run = json(
        &ep,
        &[
            "rec",
            "start",
            "--max",
            "0.5s",
            "--name",
            "short",
            "--max-size",
            "1GB",
        ],
    )
    .await;
    assert_eq!(run["name"], "short");
    assert_eq!(run["inputs"], serde_json::json!([0, 1]));
    assert_eq!(run["status"]["type"], "recording");
    assert_eq!(run["max_bytes"], 1_000_000_000u64);
    let (_, text, _) = ac2(&ep, &["rec", "status"]).await;
    assert!(
        text.starts_with("REC ") || text.starts_with("recorded short"),
        "{text}"
    );

    let deadline = Instant::now() + Duration::from_secs(20);
    let ended = loop {
        let s = json(&ep, &["rec", "status"]).await;
        if s["status"]["type"] == "ended" {
            break s;
        }
        assert!(
            Instant::now() < deadline,
            "the time limit never ended it: {s}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(ended["status"]["reason"]["type"], "duration_limit");
    assert_eq!(ended["frames"], 24_000);
    let (_, text, _) = ac2(&ep, &["rec", "status"]).await;
    assert!(
        text.starts_with("recorded short · 0:00 · 192 kB (time limit)"),
        "{text}"
    );

    let l = json(&ep, &["rec", "list"]).await;
    assert_eq!(l[0]["name"], "short");
    assert_eq!(l[0]["end"]["type"], "duration_limit");

    let s = json(&ep, &["session", "replay", "short", "--fast"]).await;
    assert_eq!(s["open"]["backend"], "replay");
    assert_eq!(s["open"]["replay"]["frames"], 24_000);
    assert_eq!(s["open"]["replay"]["pace"], "fast");
    let (_, text, _) = ac2(&ep, &["session", "status"]).await;
    assert!(text.contains("replaying short"), "{text}");
    assert!(text.contains("recorded short"), "{text}");
    let (code, _, err) = ac2(&ep, &["session", "replay", "missing"]).await;
    assert_ne!(code, 0);
    assert!(err.contains("missing"), "{err}");

    // A recorder's 16-bit WAV, imported into the recording directory, lists and replays.
    let wav = dir.path().join("ZOOM0001.WAV");
    let data: Vec<u8> = (0..4800i32)
        .flat_map(|i| (((i % 48) * 600) as i16).to_le_bytes())
        .collect();
    let mut b = b"RIFF".to_vec();
    b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&[1, 0, 1, 0]);
    b.extend_from_slice(&48_000u32.to_le_bytes());
    b.extend_from_slice(&96_000u32.to_le_bytes());
    b.extend_from_slice(&[2, 0, 16, 0]);
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    std::fs::write(&wav, b).unwrap();
    let (code, text, err) = ac2(
        &ep,
        &["rec", "import", wav.to_str().unwrap(), "--name", "bedroom"],
    )
    .await;
    assert_eq!(code, 0, "{err}");
    assert!(text.contains("1 channel(s) at 48000 Hz, 0.1 s"), "{text}");
    assert!(text.contains("ac2 session replay"), "{text}");
    let (code, _, err) = ac2(
        &ep,
        &["rec", "import", wav.to_str().unwrap(), "--name", "bedroom"],
    )
    .await;
    assert_ne!(code, 0);
    assert!(err.contains("exists already"), "{err}");
    let l = json(&ep, &["rec", "list"]).await;
    assert!(
        l.as_array().unwrap().iter().any(|r| r["name"] == "bedroom"),
        "{l}"
    );
    let s = json(&ep, &["session", "replay", "bedroom", "--fast"]).await;
    assert_eq!(s["open"]["replay"]["frames"], 4800);
    h.shutdown();
}
