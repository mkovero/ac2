//! `spl watch` against a real daemon on the simulated rig: a 1 kHz tone at −20 dBFS on
//! output 1 returns on input 1 (loopback, −20 dBFS) and input 2 (acoustic path, −6 dB).
//! The watch reads the input it was asked for, on a meter of its own: an older meter on the
//! same input (one a killed watch left behind, integrating since long before the tone)
//! never stands in for it.
#![allow(clippy::unwrap_used)]

use std::time::{Duration, Instant};

use ac2_cli::{Cli, Out, run_reporting};
use ac2_client::{Client, ClientConfig, Endpoints, OnDrop};
use ac2_proto::model::{
    DeviceSelector, GeneratorDesired, GeneratorSettings, MeasConfig, MeasKind, PeakWeighting,
    SessionConfig, Signal, SplConfig, TimeWeighting, Weighting,
};
use ac2_proto::units::{Dbfs, Hz};
use ac2_proto::{Command, ReplyBody};
use ac2d::{BackendChoice, Daemon, DaemonConfig, Listen};
use clap::Parser;
use serde_json::Value;

/// The `spl watch` JSON lines of one run.
async fn watch(ep: &Endpoints, args: &[&str]) -> Vec<Value> {
    let argv: Vec<&str> = [
        "ac2",
        "--json",
        "--ctrl-endpoint",
        &ep.ctrl,
        "--data-endpoint",
        &ep.data,
        "spl",
        "watch",
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
    assert_eq!(code, 0, "{text} {}", String::from_utf8_lossy(&se));
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["spl"].is_object())
        .collect()
}

fn num(v: &Value, k: &str) -> f64 {
    v["spl"][k].as_f64().unwrap_or(f64::NAN)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn spl_watch_reads_its_input_on_a_fresh_meter() {
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
    let c = Client::connect(ClientConfig::new(ep.clone(), "spl-rig-test"))
        .await
        .unwrap();
    c.wait_synced(Duration::from_secs(5)).await.unwrap();
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

    // A meter left running on input 2 with the same settings, integrating silence.
    let ReplyBody::Measurement(stale) = c
        .call(Command::MeasCreate {
            config: MeasConfig {
                name: "spl-in2".into(),
                kind: MeasKind::Spl {
                    config: SplConfig {
                        input: 1,
                        weighting: Weighting::Z,
                        time_weighting: TimeWeighting::Fast,
                        peak_weighting: PeakWeighting::C,
                        leq: ac2_proto::model::LeqConfig::default_windows(),
                    },
                },
            },
        })
        .await
        .unwrap()
    else {
        panic!("measurement");
    };
    c.call(Command::MeasStart { meas: stale.id }).await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;

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
    // The tone settles on both inputs.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let started = Instant::now();
    let two = watch(&ep, &["--input", "2", "--weight", "z", "--for", "1.5s"]).await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "--for ends the watch"
    );
    let last = two.last().expect("readings");
    assert_eq!(last["input"], 2);
    assert_ne!(
        last["meas"].as_u64(),
        Some(u64::from(stale.id.0)),
        "its own meter"
    );
    assert_eq!(last["spl"]["weighting"], "z");
    assert_eq!(last["spl"]["time_weighting"], "fast");
    for k in ["level", "leq", "lmax"] {
        let v = num(last, k);
        assert!((v + 26.0).abs() < 0.5, "input 2 {k}: {v} dBFS");
    }

    let one = watch(&ep, &["--input", "1", "--weight", "z", "--for", "1.5s"]).await;
    let last1 = one.last().expect("readings");
    assert_eq!(last1["input"], 1);
    assert_ne!(last1["meas"], last["meas"]);
    for k in ["level", "leq"] {
        let v = num(last1, k);
        assert!((v + 20.0).abs() < 0.5, "input 1 {k}: {v} dBFS");
    }
    // Every line of a run is one meter on one input.
    assert!(
        one.iter()
            .all(|v| v["meas"] == last1["meas"] && v["input"] == 1)
    );

    // The watches removed their meters; the older one is untouched.
    let s = c.snapshot().await.unwrap().state;
    let ids: Vec<u32> = s.measurements.iter().map(|m| m.id.0).collect();
    assert_eq!(ids, [stale.id.0]);
    assert!(s.measurements[0].running);

    lease.end().await.unwrap();
    drop(c);
    h.shutdown();
}
