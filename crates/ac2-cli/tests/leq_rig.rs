//! `spl leq` against a real daemon on the simulated rig: a 1 kHz tone at −20 dBFS reaches
//! input 2 at −26 dBFS, calibrated to read 94 dB SPL. `spl leq set` gives the meter a 5 s
//! window limited to 90 dB and a 10 s one to 96 dB; `spl leq watch --json` prints one line a
//! second with the first window over and the second near; `spl leq export` writes the
//! per-second log as CSV.
#![allow(clippy::unwrap_used)]

use std::time::{Duration, Instant};

use ac2_cli::{Cli, Out, run_reporting};
use ac2_client::{Client, ClientConfig, ClientError, Endpoints, OnDrop};
use ac2_proto::model::{
    DeviceSelector, GeneratorDesired, GeneratorSettings, MeasConfig, MeasKind, SessionConfig,
    Signal, SplConfig, TimeWeighting, Weighting,
};
use ac2_proto::units::{DbSpl, Dbfs, Hz};
use ac2_proto::{Command, ReplyBody};
use ac2d::{BackendChoice, Daemon, DaemonConfig, Listen};
use clap::Parser;
use serde_json::Value;

/// The output of one `ac2 --json …` run, and its exit code.
async fn run(ep: &Endpoints, args: &[&str]) -> (i32, String) {
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
        let mut out = Out::new(true, &mut so);
        run_reporting(&cli, &mut out, &mut se).await
    };
    let text = String::from_utf8(so).unwrap();
    (
        i32::from(code),
        format!("{text}{}", String::from_utf8_lossy(&se)),
    )
}

fn lines(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

/// The one (pretty) JSON document a command printed before any stderr text.
fn document(text: &str) -> Value {
    let mut de = serde_json::Deserializer::from_str(text).into_iter::<Value>();
    de.next().expect("a document").expect("json")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn leq_set_watch_and_export() {
    let backend = ac2d::backend(BackendChoice::Fake).unwrap();
    let listen = Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    };
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = DaemonConfig::new(backend, listen, -10.0);
    cfg.cal_store = Some(dir.path().join("calibrations.json"));
    let h = Daemon::start(cfg).unwrap();
    let ep = Endpoints {
        ctrl: h.ctrl_endpoint().to_owned(),
        data: h.data_endpoint().to_owned(),
    };
    let c = Client::connect(ClientConfig::new(ep.clone(), "leq-rig-test"))
        .await
        .unwrap();
    c.wait_synced(Duration::from_secs(5)).await.unwrap();

    // No meter yet: the commands say how to make one.
    let (code, text) = run(&ep, &["spl", "leq", "set", "--limit", "5s=90db"]).await;
    assert_eq!(code, 1, "{text}");
    assert!(text.contains("no SPL meter"), "{text}");

    c.call(Command::SessionOpen {
        config: SessionConfig {
            backend: None,
            input_device: DeviceSelector::Default,
            output_device: DeviceSelector::Default,
            input_channels: vec![0, 1],
            output_channels: 2,
            sample_rate_hz: None,
            buffer_frames: None,
            loopback: None,
        },
    })
    .await
    .unwrap();
    let ReplyBody::Measurement(m) = c
        .call(Command::MeasCreate {
            config: MeasConfig {
                name: "FOH SPL".into(),
                kind: MeasKind::Spl {
                    config: SplConfig::on_input(1, Weighting::A, TimeWeighting::Fast),
                },
            },
        })
        .await
        .unwrap()
    else {
        panic!("measurement");
    };
    c.call(Command::MeasStart { meas: m.id }).await.unwrap();

    // The tone, and the calibration against it.
    let lease = c
        .acquire_lease(false, OnDrop::StopAndRelease)
        .await
        .unwrap();
    lease
        .set(GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Sine { freq: Hz(1000.0) },
                level: Dbfs(-20.0),
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: true,
        })
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match c
            .call(Command::CalSpl {
                input: 1,
                mic: "M30".into(),
                calibrator_level: DbSpl(94.0),
                calibrator_freq: Hz(1000.0),
            })
            .await
        {
            Ok(_) => break,
            Err(ClientError::Daemon(p))
                if p.msg.contains("not steady") || p.msg.contains("no calibrator signal") => {}
            other => panic!("{other:?}"),
        }
        assert!(Instant::now() < deadline, "never steady");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    // Windows, limits, horizon: one command; the reply is the measurement.
    let (code, text) = run(
        &ep,
        &[
            "spl",
            "leq",
            "set",
            "--input",
            "2",
            "--windows",
            "5s,10s",
            "--limit",
            "5s=90db",
            "--limit",
            "10s=96db",
            "--horizon",
            "2s",
        ],
    )
    .await;
    assert_eq!(code, 0, "{text}");
    let set = document(&text);
    let leq = &set["config"]["kind"]["config"]["leq"];
    assert_eq!(leq["horizon"], 2.0);
    assert_eq!(leq["windows"][0]["duration"], 5.0);
    assert_eq!(leq["windows"][0]["limit"], 90.0);
    assert_eq!(leq["windows"][1]["limit"], 96.0);
    // A limit on a window the meter does not have is refused before anything is sent.
    let (code, text) = run(&ep, &["spl", "leq", "set", "--limit", "30min=99db"]).await;
    assert_eq!(code, 1);
    assert!(text.contains("no LAeq 30 min window"), "{text}");

    // Watch: one line per second; 94 dB is over 90 and near 96 once the windows are full.
    tokio::time::sleep(Duration::from_secs(10)).await;
    let started = Instant::now();
    let (code, text) = run(&ep, &["spl", "leq", "watch", "--for", "3.5s"]).await;
    assert_eq!(code, 0, "{text}");
    assert!(started.elapsed() < Duration::from_secs(8));
    let ls: Vec<Value> = lines(&text)
        .into_iter()
        .filter(|v| v["windows"].is_array())
        .collect();
    assert!((2..=5).contains(&ls.len()), "{} lines: {text}", ls.len());
    let seqs: Vec<u64> = ls.iter().map(|v| v["seq"].as_u64().unwrap()).collect();
    assert!(seqs.windows(2).all(|w| w[1] > w[0]), "a line per new frame");
    let last = ls.last().unwrap();
    assert_eq!(last["name"], "FOH SPL");
    assert_eq!(last["scale"], "db_spl");
    assert_eq!(last["horizon_s"], 2.0);
    let w0 = &last["windows"][0];
    assert_eq!(w0["name"], "LAeq 5 s");
    assert_eq!(w0["judgement"], "over");
    assert!((w0["leq"].as_f64().unwrap() - 94.0).abs() < 0.3, "{w0}");
    assert_eq!(w0["limit"], 90.0);
    assert_eq!(w0["text"]["state"], "OVER");
    assert_eq!(w0["cannot_recover"], true);
    assert!(w0["text"]["headroom"].is_null(), "{w0}");
    assert!(
        w0["text"]["recover"]
            .as_str()
            .unwrap()
            .starts_with("cooling down"),
        "{w0}"
    );
    let w1 = &last["windows"][1];
    assert_eq!(w1["judgement"], "near");
    assert_eq!(w1["elapsed_s"], 10.0);
    assert_eq!(w1["incomplete"], false);
    // 8 s at 94 dB stay in the 10 s window over the 2 s horizon: the next 2 s may hold at
    // most 10·lg((10·10^9.6 − 8·10^9.4) / 2) = 102.1 dB.
    let want = 10.0 * ((10.0 * 10f64.powf(9.6) - 8.0 * 10f64.powf(9.4)) / 2.0).log10();
    assert!(
        (w1["allowed"].as_f64().unwrap() - want).abs() < 0.3,
        "{w1} vs {want}"
    );

    // Export: the per-second log as CSV.
    let file = dir.path().join("foh.csv");
    let (code, text) = run(
        &ep,
        &[
            "spl",
            "leq",
            "export",
            "--meas",
            "FOH SPL",
            "-o",
            file.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(code, 0, "{text}");
    let summary = document(&text);
    let n = summary["rows"].as_u64().unwrap();
    assert!(n >= 12, "{summary}");
    let csv = std::fs::read_to_string(&file).unwrap();
    let mut it = csv.lines();
    assert_eq!(it.next(), Some("# ac2 spl log v1"));
    assert!(csv.contains("# name: FOH SPL"));
    assert!(csv.contains("# mic: M30"));
    let rows: Vec<&str> = csv
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("start_utc"))
        .collect();
    assert_eq!(rows.len() as u64, n);
    // The newest seconds: calibrated, the tone at 94 dB(A).
    let f: Vec<&str> = rows.last().unwrap().split(',').collect();
    assert_eq!(f[3], "dB SPL");
    assert!((f[4].parse::<f64>().unwrap() - 94.0).abs() < 0.2, "{f:?}");

    // The run in watch's JSON: the whole log's clock from its first second and its total.
    let run_json = &last["run"];
    assert!(
        run_json["running_s"].as_f64().unwrap() >= 12.0,
        "{run_json}"
    );
    assert!(
        (run_json["laeq"].as_f64().unwrap() - 94.0).abs() < 0.3,
        "{run_json}"
    );
    assert!(run_json["lceq"].is_number() && run_json["lzeq"].is_number());
    assert_eq!(run_json["trimmed"], false);
    assert!(run_json["gaps_s"].as_f64().unwrap() >= 0.0);
    assert!(run_json["started_at"].as_str().unwrap().ends_with('Z'));
    let line = run_json["text"]["line"].as_str().unwrap();
    assert!(line.starts_with("running 0:00:"), "{line}");
    assert!(line.contains("LAeq total 94."), "{line}");

    // A new log: refused without --yes, naming what ends; with it, the ended log is
    // written whole and the clock starts over.
    let (code, text) = run(&ep, &["spl", "leq", "new"]).await;
    assert_eq!(code, 1, "{text}");
    assert!(text.contains("this ends FOH SPL's SPL log"), "{text}");
    assert!(text.contains("--yes"), "{text}");
    let ended = dir.path().join("soundcheck.csv");
    let (code, text) = run(
        &ep,
        &[
            "spl",
            "leq",
            "new",
            "--yes",
            "--export",
            ended.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(code, 0, "{text}");
    let summary = document(&text);
    let exported = summary["exported_rows"].as_u64().unwrap();
    assert!(exported >= n, "{summary}");
    assert!(exported >= summary["ended_rows"].as_u64().unwrap());
    let csv = std::fs::read_to_string(&ended).unwrap();
    assert!(csv.contains("# name: FOH SPL"));
    // The previous log stays exportable.
    let (code, text) = run(&ep, &["spl", "leq", "export", "--previous"]).await;
    assert_eq!(code, 0, "{text}");
    assert_eq!(text.lines().count(), csv.lines().count(), "{text}");
    let (code, text) = run(&ep, &["spl", "leq", "watch", "--for", "2.5s"]).await;
    assert_eq!(code, 0, "{text}");
    let after = lines(&text)
        .into_iter()
        .rev()
        .find(|v| v["run"].is_object())
        .expect("a line with the new run");
    assert!(
        after["run"]["running_s"].as_f64().unwrap() < 6.0,
        "{}",
        after["run"]
    );
    assert!(after["logged"].as_u64().unwrap() < n, "{after}");
    // The windows fill again and are judged on their budgets: the 10 s one, 94 dB so far
    // against 96, near; its least level (the rest silent) under its Leq so far; with more
    // than the 2 s horizon to fill, its headroom holds until it is full.
    let w1 = &after["windows"][1];
    assert_eq!(w1["filling"], true, "{w1}");
    assert_eq!(w1["judgement"], "near", "{w1}");
    assert_eq!(w1["on_course"], false, "{w1}");
    assert!(w1["over_in_s"].is_null(), "{w1}");
    assert!(
        w1["least"].as_f64().unwrap() < w1["leq"].as_f64().unwrap(),
        "{w1}"
    );
    let e = w1["elapsed_s"].as_f64().unwrap();
    assert_eq!(w1["allowed_until_full"], e <= 8.0, "{w1}");
    let headroom = w1["text"]["headroom"].as_str().unwrap();
    let want = if e <= 8.0 {
        "until full: stay ≤ "
    } else {
        "next 2 s: stay ≤ "
    };
    assert!(headroom.starts_with(want), "{w1}");
    assert!(
        w1["text"]["filling"]
            .as_str()
            .unwrap()
            .starts_with("so far · "),
        "{w1}"
    );
    // The 5 s one, 94 against 90: on course until its budget is spent (2.5 times the
    // limit's power: after 2 s), then over.
    let w0 = &after["windows"][0];
    if w0["judgement"] == "near" {
        assert_eq!(w0["on_course"], true, "{w0}");
        assert!(w0["over_in_s"].as_f64().unwrap() < 2.0, "{w0}");
        assert!(
            w0["text"]["course"]
                .as_str()
                .unwrap()
                .starts_with("on course — over in "),
            "{w0}"
        );
    } else {
        assert_eq!(w0["judgement"], "over", "{w0}");
        assert_eq!(w0["on_course"], false, "{w0}");
    }

    lease.end().await.unwrap();
    drop(c);
    h.shutdown();
}
