//! `align new|show` against a real daemon (fake backend, pink noise on its loop): operands
//! by name, the band as typed, refusals from the parser and the daemon, and the readout of
//! the alignment's fit.
#![allow(clippy::unwrap_used)]

use ac2_cli::{Cli, Out, run_reporting};
use ac2_client::{Client, ClientConfig, Endpoints, OnDrop};
use ac2_proto::Command;
use ac2_proto::model::{
    DeviceSelector, GeneratorDesired, GeneratorSettings, LoopbackRoute, SessionConfig, Signal,
};
use ac2_proto::units::Dbfs;
use ac2d::{BackendChoice, Daemon, DaemonConfig, Listen};
use clap::Parser;
use serde_json::Value;

async fn run(ep: &Endpoints, json: bool, args: &[&str]) -> (i32, String) {
    let argv: Vec<&str> = ["ac2"]
        .into_iter()
        .chain(json.then_some("--json"))
        .chain(["--ctrl-endpoint", &ep.ctrl, "--data-endpoint", &ep.data])
        .chain(args.iter().copied())
        .collect();
    let cli = Cli::try_parse_from(argv).unwrap();
    let (mut so, mut se) = (Vec::new(), Vec::new());
    let code = {
        let mut out = Out::new(json, &mut so);
        run_reporting(&cli, &mut out, &mut se).await
    };
    let text = String::from_utf8(so).unwrap();
    (
        i32::from(code),
        format!("{text}{}", String::from_utf8_lossy(&se)),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn alignment_by_name() {
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
    let c = Client::connect(ClientConfig::new(ep.clone(), "align-rig-test"))
        .await
        .unwrap();
    c.wait_synced(std::time::Duration::from_secs(5))
        .await
        .unwrap();
    c.call(Command::SessionOpen {
        config: SessionConfig {
            backend: None,
            input_device: DeviceSelector::Default,
            output_device: DeviceSelector::Default,
            input_channels: vec![0, 1, 2, 3],
            output_channels: 2,
            sample_rate_hz: None,
            buffer_frames: None,
            loopback: Some(LoopbackRoute {
                output: 0,
                input: 0,
            }),
        },
    })
    .await
    .unwrap();
    for (name, input) in [("Sub", "2"), ("Main", "3")] {
        let (code, text) = run(
            &ep,
            true,
            &[
                "meas", "new", "tf", "--ref", "1", "--meas", input, "--name", name, "--start",
            ],
        )
        .await;
        assert_eq!(code, 0, "{text}");
    }

    let lease = c
        .acquire_lease(false, OnDrop::StopAndRelease)
        .await
        .unwrap();
    lease
        .set(GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Pink,
                level: Dbfs(-20.0),
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: true,
        })
        .await
        .unwrap();

    // The band is asked and must make sense; A and B differ.
    for (args, says) in [
        (&["align", "new", "Sub", "Main"][..], "--band"),
        (
            &["align", "new", "Sub", "Main", "--band", "120..60"][..],
            "0 < FROM < TO",
        ),
        (
            &["align", "new", "Sub", "Main", "--band", "60-120"][..],
            "FROM..TO",
        ),
        (
            &["align", "new", "Sub", "Sub", "--band", "60..120"][..],
            "the same operand",
        ),
        (
            &[
                "align",
                "new",
                "Sub",
                "Main",
                "--band",
                "60..120",
                "--max-delay",
                "2s",
            ][..],
            "at most 1000 ms",
        ),
    ] {
        let r = Cli::try_parse_from(
            [
                "ac2",
                "--ctrl-endpoint",
                &ep.ctrl,
                "--data-endpoint",
                &ep.data,
            ]
            .into_iter()
            .chain(args.iter().copied()),
        );
        let text = match r {
            Err(e) => e.to_string(),
            Ok(_) => run(&ep, true, args).await.1,
        };
        assert!(text.contains(says), "{args:?}: {text}");
    }

    // Made, started and read: named after its operands, its band as typed, the readout's
    // words.
    let (code, text) = run(
        &ep,
        true,
        &["align", "new", "Sub", "Main", "--band", "60..120"],
    )
    .await;
    assert_eq!(code, 0, "{text}");
    let doc: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(doc["name"], "Sub ↔ Main");
    assert_eq!(doc["band"]["lo"], 60.0);
    assert_eq!(doc["band"]["hi"], 120.0);
    let headline = doc["readout"]["headline"].as_str().unwrap();
    assert!(headline.starts_with("Delay "), "{text}");
    assert!(headline.contains(" @ 20 °C · "), "{text}");
    assert_eq!(doc["align"]["outcome"]["type"], "fit", "{text}");

    let (code, text) = run(&ep, false, &["align", "show", "Sub ↔ Main"]).await;
    assert_eq!(code, 0, "{text}");
    assert!(text.starts_with("Sub ↔ Main: Delay "), "{text}");
    assert!(text.contains("\n  residual ±"), "{text}");
    assert!(text.contains(" over 60–120 Hz · summation "), "{text}");

    let (code, text) = run(&ep, false, &["align", "show", "Sub"]).await;
    assert_ne!(code, 0);
    assert!(text.contains("Sub is not an alignment"), "{text}");
    let (code, text) = run(&ep, false, &["meas", "list"]).await;
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("alignment: #1 ↔ #2 over 60–120 Hz"), "{text}");
}
