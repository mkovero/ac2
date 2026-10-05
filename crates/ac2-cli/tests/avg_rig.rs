//! `meas new avg` against a real daemon: members by name, the method and phase reference
//! as typed, and the daemon's refusals (a member deleted under its average, too few
//! members) reach the operator as errors.
#![allow(clippy::unwrap_used)]

use ac2_cli::{Cli, Out, run_reporting};
use ac2_client::Endpoints;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn average_by_member_names() {
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
    for (name, input) in [("Seat 1", "2"), ("Seat 2", "3"), ("Seat 3", "4")] {
        let (code, text) = run(
            &ep,
            &[
                "meas", "new", "tf", "--ref", "1", "--meas", input, "--name", name,
            ],
        )
        .await;
        assert_eq!(code, 0, "{text}");
    }

    let (code, text) = run(
        &ep,
        &["meas", "new", "avg", "--name", "x", "--of", "Seat 1"],
    )
    .await;
    assert_eq!(code, 1, "{text}");
    assert!(text.contains("at least 2 members"), "{text}");
    let (code, text) = run(
        &ep,
        &[
            "meas", "new", "tf", "--ref", "1", "--meas", "2", "--name", "x", "--of", "1,2",
        ],
    )
    .await;
    assert_ne!(code, 0, "{text}");

    let (code, text) = run(
        &ep,
        &[
            "meas",
            "new",
            "avg",
            "--name",
            "Audience",
            "--of",
            "Seat 1,Seat 3",
            "--method",
            "complex",
            "--phase-ref",
            "Seat 3",
        ],
    )
    .await;
    assert_eq!(code, 0, "{text}");
    let m = document(&text);
    let cfg = &m["config"]["kind"]["config"];
    assert_eq!(m["config"]["kind"]["type"], "spatial_average");
    assert_eq!(cfg["members"], serde_json::json!([1, 3]));
    assert_eq!(cfg["method"], "complex");
    assert_eq!(cfg["reference"]["meas"], 3);

    let (code, text) = run(&ep, &["meas", "list"]).await;
    assert_eq!(code, 0, "{text}");
    let (code, text) = run(&ep, &["meas", "rm", "Seat 3"]).await;
    assert_ne!(code, 0, "{text}");
    assert!(text.contains("member of the average Audience"), "{text}");
    // Seat 2 is no member: it goes.
    let (code, text) = run(&ep, &["meas", "rm", "Seat 2"]).await;
    assert_eq!(code, 0, "{text}");
    h.shutdown();
}
