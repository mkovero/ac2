//! `spl bands` against a real daemon on the simulated rig, from an empty daemon: a 63 Hz
//! tone reaches input 2 at about 94 dB SPL once calibrated at 1 kHz. `spl bands set
//! --preset finland-545-lf --windows z:5s,a:5s --limit …` turns the band meter on with two
//! short windows; `spl bands watch --json` shows eleven bands per window, the A window's 63
//! Hz band 26 dB under the Z one, 63 Hz the worst and the headline naming its window;
//! `spl bands log` reads the tone's seconds back with their average and writes them as a
//! `<Hz> <dB>` file that `spl bands transfer` reads back; `--windows …,a:5s@63hz` makes a
//! single-band window whose limit is typed as `a:5s@63hz=30db`;
//! overlapping spans of one meter are refused; `spl bands estimate` stores a typed
//! transfer; `spl bands transfer` with typed levels moves the limits of a named place to
//! the mic and predicts the place's LAeq.
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

fn document(text: &str) -> Value {
    let mut de = serde_json::Deserializer::from_str(text).into_iter::<Value>();
    de.next().expect("a document").expect("json")
}

/// The newest `spl bands watch --json` line after `secs` of watching.
async fn watch_last(ep: &Endpoints, secs: &str) -> Value {
    let (code, text) = run(ep, &["spl", "bands", "watch", "--for", secs]).await;
    assert_eq!(code, 0, "{text}");
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .rfind(|v| v["windows"].is_array())
        .unwrap_or_else(|| panic!("no band line: {text}"))
}

async fn tone(lease: &ac2_client::StimulusLease, hz: f64) {
    lease
        .set(GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Sine { freq: Hz(hz) },
                level: Dbfs(-20.0),
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: true,
        })
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bands_set_watch_and_transfer() {
    let backend = ac2d::backend(BackendChoice::Fake).unwrap();
    let listen = Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    };
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = DaemonConfig::new(backend, listen, -10.0);
    cfg.cal_store = Some(dir.path().join("calibrations.json"));
    // The lease is held across many CLI runs; a slow runner can stall the client's refresher
    // past the 1.5 s expiry, and the lease is not under test here.
    cfg.lease_expiry = Duration::from_secs(60);
    let h = Daemon::start(cfg).unwrap();
    let ep = Endpoints {
        ctrl: h.ctrl_endpoint().to_owned(),
        data: h.data_endpoint().to_owned(),
    };
    let c = Client::connect(ClientConfig::new(ep.clone(), "band-rig-test"))
        .await
        .unwrap();
    c.wait_synced(Duration::from_secs(5)).await.unwrap();

    // An empty daemon: no meter to give a band meter.
    let (code, text) = run(&ep, &["spl", "bands", "set", "--preset", "finland-545-lf"]).await;
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

    // Watching a meter without a band meter says how to turn it on.
    let (code, text) = run(&ep, &["spl", "bands", "watch", "--for", "1s"]).await;
    assert_eq!(code, 1, "{text}");
    assert!(text.contains("--preset finland-545-lf"), "{text}");

    // Calibrated at 1 kHz, then the tone moves to 63 Hz.
    let lease = c
        .acquire_lease(false, OnDrop::StopAndRelease)
        .await
        .unwrap();
    tone(&lease, 1000.0).await;
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
    tone(&lease, 63.0).await;

    let (code, text) = run(
        &ep,
        &[
            "spl",
            "bands",
            "set",
            "--preset",
            "finland-545-lf",
            "--windows",
            "z:5s,a:5s",
            "--limit",
            "z:5s:63hz=42db",
            "--day-offset",
            "z:5s=5db",
            "--limit",
            "a:5s:63hz=30db",
            "--tonal",
            "3",
        ],
    )
    .await;
    assert_eq!(code, 0, "{text}");
    let set = document(&text);
    let bands = &set["config"]["kind"]["config"]["bands"];
    let z = &bands["windows"][0];
    assert_eq!(z["duration"], 5.0);
    assert_eq!(z["weighting"], "z");
    assert_eq!(z["limits"]["type"], "night_day");
    assert_eq!(z["limits"]["night"][5], 42.0);
    assert_eq!(z["limits"]["day_offset"], 5.0);
    assert_eq!(bands["windows"][1]["limits"]["type"], "always");
    assert_eq!(bands["predicted"]["night"], 25.0);
    assert_eq!(bands["correction"]["tonal"], "plus3");

    // Without a transfer the limits are judged at the mic as typed: the windows fill with
    // the tone; 63 Hz is the worst band, tens of dB over, in the Z window.
    tokio::time::sleep(Duration::from_secs(6)).await;
    let last = watch_last(&ep, "2.5s").await;
    assert_eq!(last["name"], "FOH SPL");
    assert_eq!(last["scale"], "db_spl");
    assert_eq!(last["limits_from"], "at_mic");
    assert_eq!(last["correction_db"], 3.0);
    let ws = last["windows"].as_array().unwrap();
    assert_eq!(ws.len(), 2, "{last}");
    let bs = ws[0]["bands"].as_array().unwrap();
    assert_eq!(bs.len(), 11, "{last}");
    let hz: Vec<f64> = bs
        .iter()
        .map(|b| b["nominal_hz"].as_f64().unwrap())
        .collect();
    assert_eq!(hz.first(), Some(&20.0));
    assert_eq!(hz.last(), Some(&200.0));
    assert_eq!(last["worst"]["window"], 0, "{last}");
    assert_eq!(last["worst"]["nominal_hz"], 63.0);
    assert_eq!(ws[0]["worst_hz"], 63.0);
    let b63 = &bs[5];
    assert_eq!(b63["judgement"], "over", "{b63}");
    // About 94 dB of tone plus the +3 dB correction.
    assert!((b63["leq"].as_f64().unwrap() - 97.0).abs() < 1.5, "{b63}");
    assert_eq!(b63["text"]["name"], "63 Hz band LZeq 5 s");
    assert_eq!(b63["text"]["state"], "OVER");
    // A-weighting at 63 Hz is −26.2 dB.
    let a63 = &ws[1]["bands"][5];
    assert_eq!(ws[1]["weighting"], "a");
    assert!((a63["leq"].as_f64().unwrap() - 70.8).abs() < 1.5, "{a63}");
    assert_eq!(a63["text"]["name"], "63 Hz band LAeq 5 s");
    let headline = last["text"]["headline"].as_str().unwrap();
    assert!(
        headline.starts_with("63 Hz band LZeq 5 s ") && headline.contains(" dB over its limit · "),
        "{headline}"
    );
    assert!(
        ws[0]["text"]["period"]
            .as_str()
            .unwrap()
            .contains("limits ("),
        "{last}"
    );
    assert!(ws[1]["text"]["period"].is_null(), "one set day and night");
    assert!(last["predicted"].is_null(), "no transfer, no prediction");

    // The band log over three seconds of the tone: about 94 dB SPL at 63 Hz (the log
    // keeps the correction apart), every second calibrated, one row each.
    let (code, text) = run(
        &ep,
        &[
            "spl", "bands", "log", "--from", "-4s", "--until", "-1s", "--step", "1",
        ],
    )
    .await;
    assert_eq!(code, 0, "{text}");
    let l = document(&text);
    let a = &l["average"];
    let seconds = a["seconds"].as_u64().unwrap();
    assert!(seconds >= 2, "{l}");
    assert_eq!(a["uncalibrated"], 0);
    assert!((a["levels"][5].as_f64().unwrap() - 94.0).abs() < 1.5, "{a}");
    assert_eq!(l["rows"].as_array().unwrap().len() as u64, seconds);
    assert_eq!(l["rows"][0]["levels"].as_array().unwrap().len(), 28);
    // Too many rows for one reply is refused by the daemon, naming the step that fits.
    let (code, text) = run(
        &ep,
        &[
            "spl", "bands", "log", "--from", "-1s", "--until", "now", "--step", "0",
        ],
    )
    .await;
    assert_eq!(code, 1, "{text}");

    // The span's averages as a `<Hz> <dB>` file, read back as typed levels: what a rig in
    // the place hands to the one at FOH.
    let span_file = dir.path().join("span.txt");
    let (code, text) = run(
        &ep,
        &[
            "spl",
            "bands",
            "log",
            "--from",
            "-4s",
            "--until",
            "-1s",
            "--levels-out",
            span_file.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(code, 0, "{text}");
    let avg = document(&text)["average"]["levels"].clone();
    let written = std::fs::read_to_string(&span_file).unwrap();
    assert!(written.starts_with("# FOH SPL band log "), "{written}");
    let back = ac2_traces::band_levels::parse(&written).unwrap();
    for (i, b) in back.iter().enumerate() {
        match (b, avg[i].as_f64()) {
            (Some(b), Some(a)) => assert!((b - a).abs() < 0.006, "band {i}: {b} {a}"),
            (None, None) => {}
            other => panic!("band {i}: {other:?}"),
        }
    }

    // A single-band window: the A window on 63 Hz alone, its limit typed without naming
    // the band; the Z window kept with its limits; the log keeps every band.
    let (code, text) = run(
        &ep,
        &[
            "spl",
            "bands",
            "set",
            "--windows",
            "z:5s,a:5s@63hz",
            "--limit",
            "a:5s@63hz=30db",
        ],
    )
    .await;
    assert_eq!(code, 0, "{text}");
    let set = document(&text);
    let a = &set["config"]["kind"]["config"]["bands"]["windows"][1];
    assert_eq!(a["bands"], serde_json::json!({"low": 63.0, "high": 63.0}));
    assert_eq!(a["limits"]["limits"][5], 30.0);
    let z = &set["config"]["kind"]["config"]["bands"]["windows"][0];
    assert_eq!(z["limits"]["night"][5], 42.0, "kept");
    tokio::time::sleep(Duration::from_secs(1)).await;
    let last = watch_last(&ep, "1.5s").await;
    let a = &last["windows"][1];
    assert_eq!(a["bands_hz"], serde_json::json!([63.0, 63.0]), "{last}");
    assert_eq!(a["text"]["name"], "63 Hz LAeq 5 s");
    let bs = a["bands"].as_array().unwrap();
    assert_eq!(bs.len(), 1, "{last}");
    assert_eq!(bs[0]["nominal_hz"], 63.0);
    assert_eq!(bs[0]["limit"], 30.0);
    assert_eq!(bs[0]["judgement"], "over", "{last}");
    assert_eq!(last["windows"][0]["bands"].as_array().unwrap().len(), 11);
    assert_eq!(last["worst"]["nominal_hz"], 63.0);

    // The span file as the place, 0 dB attenuation everywhere from FOH's own span.
    let (code, text) = run(
        &ep,
        &[
            "spl",
            "bands",
            "transfer",
            "--foh",
            span_file.to_str().unwrap(),
            "--place-levels",
            span_file.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(code, 0, "{text}");
    let t = &document(&text)["config"]["kind"]["config"]["bands"]["transfer"];
    assert_eq!(t["origin"], "measured");
    assert_eq!(t["bands"][5]["attenuation"], 0.0);
    // One mic cannot be at FOH and at the place over the same seconds.
    let (code, text) = run(
        &ep,
        &[
            "spl",
            "bands",
            "transfer",
            "--foh",
            "FOH SPL@-10s..-5s",
            "--place-levels",
            "FOH SPL@-6s..-1s",
        ],
    )
    .await;
    assert_eq!(code, 1, "{text}");
    assert!(text.contains("spans overlap on FOH SPL"), "{text}");

    // An estimated transfer, typed where the place cannot be reached.
    let guess = dir.path().join("guess.txt");
    std::fs::write(&guess, "63 25\n125 30\n").unwrap();
    let (code, text) = run(
        &ep,
        &[
            "spl",
            "bands",
            "estimate",
            "--attenuation",
            guess.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(code, 0, "{text}");
    let t = &document(&text)["config"]["kind"]["config"]["bands"]["transfer"];
    assert_eq!(t["origin"], "estimated");
    assert_eq!(t["bands"][5]["status"], "unchecked");
    assert_eq!(t["bands"][5]["attenuation"], 25.0);
    assert_eq!(t["bands"][0]["status"], "missing");
    tokio::time::sleep(Duration::from_secs(1)).await;
    let last = watch_last(&ep, "1.5s").await;
    assert_eq!(last["limits_from"], "estimated");

    // A transfer from typed levels: 30 dB quieter at the place in every limited band.
    let foh = dir.path().join("foh.txt");
    let at_place = dir.path().join("flat4.txt");
    let lf = [
        20.0, 25.0, 31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0,
    ];
    let lines = |db: f64| lf.iter().map(|f| format!("{f} {db}\n")).collect::<String>();
    std::fs::write(&foh, lines(80.0)).unwrap();
    std::fs::write(&at_place, lines(50.0)).unwrap();
    let (code, text) = run(
        &ep,
        &[
            "spl",
            "bands",
            "transfer",
            "--foh",
            foh.to_str().unwrap(),
            "--place-levels",
            at_place.to_str().unwrap(),
            "--place",
            "flat 4",
        ],
    )
    .await;
    assert_eq!(code, 0, "{text}");
    let t = &document(&text)["config"]["kind"]["config"]["bands"]["transfer"]["bands"];
    assert_eq!(t[5]["status"], "unchecked");
    assert_eq!(t[5]["attenuation"], 30.0);
    assert_eq!(t[11]["status"], "missing");
    // A span of a band log the daemon has not logged there is refused by it.
    let (code, text) = run(
        &ep,
        &[
            "spl",
            "bands",
            "transfer",
            "--foh",
            "FOH SPL@2020-01-01T00:00:00Z..2020-01-01T00:00:30Z",
            "--place-levels",
            at_place.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(code, 1, "{text}");

    tokio::time::sleep(Duration::from_secs(2)).await;
    let last = watch_last(&ep, "1.5s").await;
    assert_eq!(last["limits_from"], "transferred");
    assert_eq!(last["worst"]["nominal_hz"], 63.0);
    assert_eq!(
        last["text"]["limits_from"],
        "limits moved from flat 4 through the band transfer"
    );
    let p = &last["predicted"];
    assert!(p.is_object(), "{last}");
    let line = last["text"]["predicted"].as_str().unwrap();
    assert!(
        line.starts_with("predicted LAeq 60 min in flat 4 "),
        "{line}"
    );

    // Off: the band meter goes, the SPL meter carries on.
    let (code, text) = run(&ep, &["spl", "bands", "set", "--off"]).await;
    assert_eq!(code, 0, "{text}");
    assert!(document(&text)["config"]["kind"]["config"]["bands"].is_null());
    lease.end().await.unwrap();
    drop(c);
    h.shutdown();
}
