//! `meas eq …` against a real daemon on the fake backend: a set made, edited and shown by
//! section number, the vocabulary the app uses (`3 · peaking 1.25 kHz −4.0 dB Q 2.0
//! (half-gain)`, `assumes none of it in`, `in console (rev 4)`), the daemon's refusals by
//! message, export / import in both formats, copy and delete.
#![allow(clippy::unwrap_used)]

use ac2_cli::{Cli, Out, run_reporting};
use ac2_client::Endpoints;
use ac2d::{BackendChoice, Daemon, DaemonConfig, Listen};
use clap::Parser;

async fn run(ep: &Endpoints, args: &[&str]) -> (i32, String) {
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
        let mut out = Out::new(false, &mut so);
        run_reporting(&cli, &mut out, &mut se).await
    };
    (
        i32::from(code),
        format!(
            "{}{}",
            String::from_utf8(so).unwrap(),
            String::from_utf8_lossy(&se)
        ),
    )
}

async fn ok(ep: &Endpoints, args: &[&str]) -> String {
    let (code, text) = run(ep, args).await;
    assert_eq!(code, 0, "{args:?}: {text}");
    text
}

async fn refused(ep: &Endpoints, args: &[&str], says: &str) {
    let (code, text) = run(ep, args).await;
    assert_ne!(code, 0, "{args:?}: {text}");
    assert!(text.contains(says), "{args:?}: {text}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn eq_sets_by_section_number() {
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
    let ep = &ep;
    for (name, input) in [("Main L", "2"), ("Main R", "3")] {
        ok(
            ep,
            &[
                "meas", "new", "tf", "--ref", "1", "--meas", input, "--name", name,
            ],
        )
        .await;
    }
    let l = "Main L";
    refused(
        ep,
        &["meas", "eq", "add", l, "peaking", "1k", "-6", "--q", "2"],
        "Main L has no EQ set",
    )
    .await;
    let t = ok(ep, &["meas", "eq", "new", l]).await;
    assert!(
        t.starts_with("Main L · EQ 1 (set 1, rev ")
            && t.contains("· digital 48 kHz · Q measured half-gain · not marked"),
        "{t}"
    );
    ok(
        ep,
        &["meas", "eq", "add", l, "peaking", "1.25k", "-4", "--q", "2"],
    )
    .await;
    ok(
        ep,
        &["meas", "eq", "add", l, "high-pass", "80", "--lr", "4"],
    )
    .await;
    let t = ok(
        ep,
        &[
            "meas",
            "eq",
            "add",
            l,
            "low-shelf",
            "120",
            "3",
            "--s",
            "1",
            "--at",
            "1",
        ],
    )
    .await;
    assert!(
        t.contains("\n  1 · low shelf 120 Hz +3.0 dB S 1.00\n"),
        "{t}"
    );
    assert!(
        t.contains("\n  2 · peaking 1.25 kHz −4.0 dB Q 2.0 (half-gain)\n"),
        "{t}"
    );
    assert!(t.contains("\n  3 · high-pass 80.0 Hz LR 4"), "{t}");
    assert!(
        t.contains("\n  predicts Main L after EQ 1 · assumes none of it in"),
        "{t}"
    );

    let t = ok(ep, &["meas", "eq", "set", l, "2", "--gain", "-3.5"]).await;
    assert!(t.contains("2 · peaking 1.25 kHz −3.5 dB Q 2.0"), "{t}");
    let t = ok(ep, &["meas", "eq", "set", l, "3", "--off"]).await;
    assert!(t.contains("3 · high-pass 80.0 Hz LR 4 · off"), "{t}");
    refused(
        ep,
        &["meas", "eq", "set", l, "3", "--gain", "1"],
        "a high-pass section has no gain",
    )
    .await;
    refused(
        ep,
        &["meas", "eq", "set", l, "9", "--on"],
        "EQ 1 has no section 9 (it has 3)",
    )
    .await;
    // The daemon's refusals reach the operator as they are.
    refused(
        ep,
        &["meas", "eq", "add", l, "low-pass", "100", "--lr", "3"],
        "low-pass",
    )
    .await;
    refused(
        ep,
        &["meas", "eq", "add", l, "peaking", "30k", "-3", "--q", "1"],
        "peaking",
    )
    .await;
    refused(
        ep,
        &["meas", "eq", "add", l, "peaking", "1k", "-3"],
        "peaking needs --q or --oct",
    )
    .await;

    let t = ok(ep, &["meas", "eq", "bypass", l]).await;
    assert!(t.lines().next().unwrap().ends_with("· bypassed"), "{t}");
    let t = ok(ep, &["meas", "eq", "bypass", l, "--off"]).await;
    assert!(!t.contains("bypassed"), "{t}");
    let t = ok(ep, &["meas", "eq", "mark", l]).await;
    let rev = t
        .split("rev ")
        .nth(1)
        .and_then(|r| r.split(')').next())
        .unwrap()
        .to_owned();
    assert!(t.contains(&format!("in console (rev {rev})")), "{t}");
    let t = ok(ep, &["meas", "eq", "show", l]).await;
    assert!(
        t.contains("predicts Main L after EQ 1 · EQ 1 in · no change"),
        "{t}"
    );
    let t = ok(ep, &["meas", "eq", "set", l, "2", "--gain", "-5"]).await;
    assert!(t.contains(&format!("· change since rev {rev}")), "{t}");
    let t = ok(ep, &["meas", "eq", "mark", l, "--clear"]).await;
    assert!(t.contains("not marked"), "{t}");

    // Export and import, both formats.
    let dir = tempfile::tempdir().unwrap();
    let ac2_file = dir.path().join("eq1.txt");
    let apo_file = dir.path().join("eq1-apo.txt");
    let (a, p) = (ac2_file.to_str().unwrap(), apo_file.to_str().unwrap());
    ok(ep, &["meas", "eq", "export", l, a]).await;
    assert!(
        std::fs::read_to_string(a)
            .unwrap()
            .starts_with("# ac2 eq 1\n")
    );
    ok(ep, &["meas", "eq", "export", l, p, "--format", "apo"]).await;
    let apo_text = std::fs::read_to_string(p).unwrap();
    assert!(
        apo_text.contains("Filter: ON PK Fc 1250 Hz Gain -5 dB Q 2"),
        "{apo_text}"
    );
    assert!(
        apo_text.contains("Filter: OFF HPQ Fc 80 Hz Q 0.7071"),
        "{apo_text}"
    );
    ok(
        ep,
        &["meas", "eq", "add", l, "high-pass", "40", "--bessel", "2"],
    )
    .await;
    refused(
        ep,
        &["meas", "eq", "export", l, p, "--format", "apo"],
        "4 · high-pass: Bessel sections sit at different frequencies",
    )
    .await;
    ok(
        ep,
        &[
            "meas",
            "eq",
            "export",
            l,
            p,
            "--format",
            "apo",
            "--skip-unsupported",
        ],
    )
    .await;
    assert!(
        std::fs::read_to_string(p)
            .unwrap()
            .contains("# skipped 4 · high-pass")
    );

    // An APO file imports as a new set; then one set has to be named.
    let t = ok(ep, &["meas", "eq", "import", l, p]).await;
    assert!(t.starts_with("Main L · EQ 2 (set 2"), "{t}");
    assert!(
        t.contains("2 · peaking 1.25 kHz −5.0 dB Q 2.0 (half-gain)"),
        "{t}"
    );
    refused(
        ep,
        &["meas", "eq", "rm", l, "1"],
        "Main L has 2 EQ sets (EQ 1 (id 1), EQ 2 (id 2)): add --set",
    )
    .await;
    std::fs::write(
        &apo_file,
        "Filter: ON PK Fc 100 Hz Gain -3 dB Q 1\nChannel: L\n",
    )
    .unwrap();
    refused(
        ep,
        &["meas", "eq", "import", l, p],
        "line 2: \"Channel\" is not read",
    )
    .await;
    // The ac2 text puts a whole set back.
    let t = ok(ep, &["meas", "eq", "import", l, a, "--set", "EQ 2"]).await;
    assert!(t.contains("3 · high-pass 80.0 Hz LR 4 · off"), "{t}");
    assert!(!t.contains("Bessel"), "{t}");

    let t = ok(
        ep,
        &["meas", "eq", "copy", l, "--to", "Main R", "--set", "EQ 1"],
    )
    .await;
    assert!(t.starts_with("Main R · EQ 1"), "{t}");
    ok(
        ep,
        &["meas", "eq", "on", l, "live", "--off", "--set", "EQ 1"],
    )
    .await;
    let t = ok(ep, &["meas", "eq", "show", l]).await;
    assert!(!t.contains("predicts Main L after EQ 1 ·"), "{t}");
    refused(
        ep,
        &["meas", "eq", "on", l, "no such curve", "--set", "EQ 1"],
        "no trace named",
    )
    .await;
    let t = ok(ep, &["meas", "eq", "delete", l, "--set", "EQ 2"]).await;
    assert_eq!(t.trim(), "deleted EQ 2 of Main L");
    h.shutdown();
}
