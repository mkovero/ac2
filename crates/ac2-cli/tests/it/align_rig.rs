//! `align new|show` against a real daemon (fake backend, pink noise on its loop): operands
//! by name, the band as typed, refusals from the parser and the daemon, and the readout of
//! the alignment's fit.
#![allow(clippy::unwrap_used)]

use ac2_cli::Cli;
use ac2_client::{Endpoints, OnDrop};
use ac2_proto::Command;
use ac2_proto::model::{GeneratorDesired, GeneratorSettings, LoopbackRoute, SessionConfig, Signal};
use ac2_proto::units::Dbfs;
use clap::Parser;
use serde_json::Value;

use super::common::{ac2, client, rig, session};

async fn run(ep: &Endpoints, json: bool, args: &[&str]) -> (i32, String) {
    ac2(ep, json, args).await.text()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn alignment_by_name() {
    let (_h, ep) = rig();
    let c = client(&ep, "align-rig-test").await;
    c.call(Command::SessionOpen {
        config: SessionConfig {
            loopback: Some(LoopbackRoute {
                output: 0,
                input: 0,
            }),
            ..session(vec![0, 1, 2, 3])
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
    assert!(headline.contains(": delay "), "{text}");
    assert!(headline.contains(" @ 20 °C · "), "{text}");
    assert_eq!(doc["align"]["outcome"]["type"], "fit", "{text}");

    let (code, text) = run(&ep, false, &["align", "show", "Sub ↔ Main"]).await;
    assert_eq!(code, 0, "{text}");
    assert!(
        text.starts_with("Sub ↔ Main\n  ") && text.contains(": delay "),
        "{text}"
    );
    assert!(text.contains("\n  residual ±"), "{text}");
    assert!(text.contains(" over 60–120 Hz · summation "), "{text}");

    let (code, text) = run(&ep, false, &["align", "show", "Sub"]).await;
    assert_ne!(code, 0);
    assert!(text.contains("Sub is not an alignment"), "{text}");
    let (code, text) = run(&ep, false, &["meas", "list"]).await;
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("alignment: #1 ↔ #2 over 60–120 Hz"), "{text}");
}
