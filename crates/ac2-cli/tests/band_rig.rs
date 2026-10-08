//! `spl bands` against a real daemon on the simulated rig, from an empty daemon: a 63 Hz
//! tone reaches input 2 at about 94 dB SPL once calibrated at 1 kHz. `spl bands set
//! --preset finland-545-lf --duration 5s` turns the band meter on; `spl bands watch --json`
//! with the mic in the bedroom shows eleven bands with 63 Hz the worst, far over its limit, and the headline naming it;
//! `spl bands log` reads the tone's seconds back with their average and writes them as a
//! `<Hz> <dB>` file that `spl bands transfer` reads back; at FOH without a transfer nothing
//! is judged; overlapping spans of one meter are refused; `spl bands estimate` stores a
//! typed transfer; `spl bands transfer` with typed levels moves the limits to the mic and
//! predicts the dwelling's LAeq.
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
        .rfind(|v| v["bands"].is_array())
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
            "--duration",
            "5s",
            "--tonal",
            "3",
            "--mic",
            "bedroom",
        ],
    )
    .await;
    assert_eq!(code, 0, "{text}");
    let set = document(&text);
    let bands = &set["config"]["kind"]["config"]["bands"];
    assert_eq!(bands["duration"], 5.0);
    assert_eq!(bands["night"][5], 42.0);
    assert_eq!(bands["day"][5], 47.0);
    assert_eq!(bands["predicted"]["night"], 25.0);
    assert_eq!(bands["correction"]["tonal"], "plus3");
    assert_eq!(bands["mic"], "dwelling");

    // A bedroom monitor: the window fills with the tone; 63 Hz is the worst band, tens of
    // dB over.
    tokio::time::sleep(Duration::from_secs(6)).await;
    let last = watch_last(&ep, "2.5s").await;
    assert_eq!(last["name"], "FOH SPL");
    assert_eq!(last["scale"], "db_spl");
    assert_eq!(last["limits_from"], "at_mic", "the mic is in the bedroom");
    assert_eq!(last["correction_db"], 3.0);
    let bs = last["bands"].as_array().unwrap();
    assert_eq!(bs.len(), 11, "{last}");
    let hz: Vec<f64> = bs
        .iter()
        .map(|b| b["nominal_hz"].as_f64().unwrap())
        .collect();
    assert_eq!(hz.first(), Some(&20.0));
    assert_eq!(hz.last(), Some(&200.0));
    assert_eq!(last["worst"], 5, "{last}");
    assert_eq!(last["worst_hz"], 63.0);
    let b63 = &bs[5];
    assert_eq!(b63["judgement"], "over", "{b63}");
    // About 94 dB of tone plus the +3 dB correction.
    assert!((b63["leq"].as_f64().unwrap() - 97.0).abs() < 1.5, "{b63}");
    assert_eq!(b63["text"]["name"], "63 Hz band Leq");
    assert_eq!(b63["text"]["state"], "OVER");
    let headline = last["text"]["headline"].as_str().unwrap();
    assert!(
        headline.starts_with("63 Hz band Leq ") && headline.contains(" dB over its limit · "),
        "{headline}"
    );
    assert!(
        last["text"]["period"]
            .as_str()
            .unwrap()
            .contains("limits ("),
        "{last}"
    );
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
    // the bedroom hands to the one at FOH.
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

    // At FOH without a transfer the bedroom's limits are not judged.
    let (code, text) = run(&ep, &["spl", "bands", "set", "--mic", "foh"]).await;
    assert_eq!(code, 0, "{text}");
    tokio::time::sleep(Duration::from_secs(1)).await;
    let last = watch_last(&ep, "1.5s").await;
    assert_eq!(last["limits_from"], "no_transfer");
    assert_eq!(
        last["text"]["headline"],
        "no band transfer — limits are for the bedroom, measure the transfer"
    );
    assert!(last["worst"].is_null(), "{last}");

    // The span file as the dwelling, 0 dB attenuation everywhere from FOH's own span.
    let (code, text) = run(
        &ep,
        &[
            "spl",
            "bands",
            "transfer",
            "--foh",
            span_file.to_str().unwrap(),
            "--dwelling",
            span_file.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(code, 0, "{text}");
    let t = &document(&text)["config"]["kind"]["config"]["bands"]["transfer"];
    assert_eq!(t["origin"], "measured");
    assert_eq!(t["bands"][5]["attenuation"], 0.0);
    // One mic cannot be at FOH and in the bedroom over the same seconds.
    let (code, text) = run(
        &ep,
        &[
            "spl",
            "bands",
            "transfer",
            "--foh",
            "FOH SPL@-10s..-5s",
            "--dwelling",
            "FOH SPL@-6s..-1s",
        ],
    )
    .await;
    assert_eq!(code, 1, "{text}");
    assert!(text.contains("spans overlap on FOH SPL"), "{text}");

    // An estimated transfer, typed where the bedroom cannot be reached.
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

    // A transfer from typed levels: 30 dB quieter in the dwelling in every limited band.
    let foh = dir.path().join("foh.txt");
    let dwelling = dir.path().join("bedroom.txt");
    let lf = [
        20.0, 25.0, 31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0,
    ];
    let lines = |db: f64| lf.iter().map(|f| format!("{f} {db}\n")).collect::<String>();
    std::fs::write(&foh, lines(80.0)).unwrap();
    std::fs::write(&dwelling, lines(50.0)).unwrap();
    let (code, text) = run(
        &ep,
        &[
            "spl",
            "bands",
            "transfer",
            "--foh",
            foh.to_str().unwrap(),
            "--dwelling",
            dwelling.to_str().unwrap(),
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
            "--dwelling",
            dwelling.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(code, 1, "{text}");

    tokio::time::sleep(Duration::from_secs(2)).await;
    let last = watch_last(&ep, "1.5s").await;
    assert_eq!(last["limits_from"], "transferred");
    assert_eq!(last["worst"], 5);
    assert_eq!(
        last["text"]["limits_from"],
        "limits transferred from the dwelling"
    );
    let p = &last["predicted"];
    assert!(p.is_object(), "{last}");
    let line = last["text"]["predicted"].as_str().unwrap();
    assert!(line.starts_with("predicted dwelling LAeq "), "{line}");

    // Off: the band meter goes, the SPL meter carries on.
    let (code, text) = run(&ep, &["spl", "bands", "set", "--off"]).await;
    assert_eq!(code, 0, "{text}");
    assert!(document(&text)["config"]["kind"]["config"]["bands"].is_null());
    lease.end().await.unwrap();
    drop(c);
    h.shutdown();
}
