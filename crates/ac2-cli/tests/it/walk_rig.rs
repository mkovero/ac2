//! `plan new|list|show|export|import` and `walk new|status|next|prev|skip|redo` against a real
//! daemon: a rectangle saved and read back, a plan file round trip under another name,
//! positions suggested on the plan, and the walk moved point to point.
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

async fn ok(ep: &Endpoints, args: &[&str]) -> Value {
    let (code, text) = run(ep, args).await;
    assert_eq!(code, 0, "{args:?}: {text}");
    document(&text)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plans_and_a_walk_by_name() {
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
    let tmp = tempfile::tempdir().unwrap();

    let p = ok(
        &ep,
        &[
            "plan",
            "new",
            "Hall",
            "--width",
            "20",
            "--depth",
            "30",
            "--ceiling",
            "9",
        ],
    )
    .await;
    assert_eq!(p["name"], "Hall");
    assert_eq!(p["zones"][0]["outline"][0], serde_json::json!([1.0, 1.0]));
    let (code, text) = run(
        &ep,
        &["plan", "new", "Tiny", "--width", "1", "--depth", "9"],
    )
    .await;
    assert_ne!(code, 0, "{text}");

    let file = tmp.path().join("hall.json");
    let f = file.to_str().unwrap();
    ok(&ep, &["plan", "export", "Hall", f]).await;
    let back = ok(&ep, &["plan", "import", f, "--name", "Hall 2"]).await;
    assert_eq!(back["outline"], p["outline"]);
    let list = ok(&ep, &["plan", "list"]).await;
    let names: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Hall", "Hall 2"]);
    assert_eq!(ok(&ep, &["plan", "show", "Hall 2"]).await["name"], "Hall 2");

    ok(
        &ep,
        &[
            "meas", "new", "tf", "--ref", "1", "--meas", "2", "--name", "Seat",
        ],
    )
    .await;
    let w = ok(
        &ep,
        &[
            "walk", "new", "Mains", "--plan", "Hall", "--meas", "Seat", "--points", "4",
        ],
    )
    .await;
    let labels: Vec<&str> = w["points"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["P1", "P2", "P3", "P4"]);
    assert_eq!(w["step"], 0);
    for p in w["points"].as_array().unwrap() {
        assert_eq!(p["at"][2], 1.2, "at the zone's ear height");
    }

    assert_eq!(ok(&ep, &["walk", "prev"]).await["step"], 3, "wraps round");
    assert_eq!(ok(&ep, &["walk", "next"]).await["step"], 0);
    let w = ok(&ep, &["walk", "skip", "--walk", "Mains"]).await;
    assert_eq!(w["points"][0]["state"]["type"], "skipped");
    assert_eq!(w["step"], 1);
    let w = ok(&ep, &["walk", "redo", "P1"]).await;
    assert_eq!(w["points"][0]["state"]["type"], "pending");
    assert_eq!(w["step"], 0);
    assert_eq!(ok(&ep, &["walk", "status"]).await["name"], "Mains");

    // A point the walk lacks, and a measurement another walk captures with, are refused.
    let (code, text) = run(&ep, &["walk", "redo", "P9"]).await;
    assert_ne!(code, 0, "{text}");
    assert!(text.contains("no point"), "{text}");
    let (code, text) = run(
        &ep,
        &[
            "walk", "new", "Again", "--plan", "Hall", "--meas", "Seat", "--points", "2",
        ],
    )
    .await;
    assert_ne!(code, 0, "{text}");
    assert!(text.contains("captures for the walk Mains"), "{text}");
    h.shutdown();
}

/// `sim run` at given points of a plan with a speaker: waits for the result, `sim status`
/// shows it again, and a plan without a ceiling is refused by the daemon by name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_simulation_of_a_plan() {
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
    let tmp = tempfile::tempdir().unwrap();
    let mut room = ok(
        &ep,
        &[
            "plan",
            "new",
            "Room",
            "--width",
            "8",
            "--depth",
            "6",
            "--ceiling",
            "3",
        ],
    )
    .await;
    room["name"] = "Room S".into();
    room["markers"] = serde_json::json!([
        { "name": "Main", "kind": "speaker", "at": [1.0, 3.0, 1.5], "heading_deg": null }
    ]);
    let file = tmp.path().join("room.json");
    std::fs::write(&file, room.to_string()).unwrap();
    ok(&ep, &["plan", "import", file.to_str().unwrap()]).await;

    let s = ok(
        &ep,
        &[
            "sim",
            "run",
            "--plan",
            "Room S",
            "--point",
            "6,2,1.2",
            "--point",
            "Bar=7,5,1.2",
            "--rays",
            "2000",
            "--order",
            "1",
        ],
    )
    .await;
    assert_eq!(s["status"]["type"], "done", "{s}");
    assert_eq!(s["absorption"]["type"], "assumed");
    let labels: Vec<&str> = s["sources"][0]["receivers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["P1", "Bar"]);
    assert_eq!(ok(&ep, &["sim", "status"]).await["id"], s["id"]);

    let (code, text) = run(&ep, &["sim", "run", "--plan", "Room", "--point", "6,2,1.2"]).await;
    assert_ne!(code, 0, "{text}");
    assert!(text.contains("speaker"), "no speaker in the plan: {text}");
    h.shutdown();
}
