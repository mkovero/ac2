//! Ownership from the CLI against a real daemon (`docs/design/measurement-tree.md`): a
//! sweep measurement made with `meas new sweep` waits (no job), traces move with `trace
//! move`, a math channel is filed with `--under`, and `meas rm` of a measurement that owns
//! traces asks for `--keep-traces` or `--delete-traces`; a target is filed with `--meas`.
#![allow(clippy::unwrap_used)]

use super::common::{document, rig, run_json as run};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owners_from_the_cli() {
    let (h, ep) = rig();
    let (code, text) = run(
        &ep,
        &[
            "meas", "new", "tf", "--ref", "1", "--meas", "2", "--name", "Main L",
        ],
    )
    .await;
    assert_eq!(code, 0, "{text}");

    // A sweep measurement: settings only, no level no measurement.
    let (code, text) = run(
        &ep,
        &[
            "meas",
            "new",
            "sweep",
            "--ref",
            "1",
            "--meas",
            "2",
            "--out",
            "1",
            "--name",
            "Genelec 1 m",
        ],
    )
    .await;
    assert_ne!(code, 0);
    assert!(text.contains("--level"), "{text}");
    let (code, text) = run(
        &ep,
        &[
            "meas",
            "new",
            "sweep",
            "--ref",
            "1",
            "--meas",
            "2",
            "--out",
            "1,2",
            "--level",
            "-50dbfs",
            "--duration",
            "6s",
            "--repeats",
            "2",
            "--name",
            "Genelec 1 m",
        ],
    )
    .await;
    assert_eq!(code, 0, "{text}");
    let m = document(&text);
    assert_eq!(m["config"]["kind"]["type"], "sweep");
    assert_eq!(m["config"]["kind"]["config"]["level"], -50.0);
    assert_eq!(m["config"]["kind"]["config"]["repeats"], 2);
    assert_eq!(
        m["config"]["kind"]["config"]["outputs"],
        serde_json::json!([0, 1])
    );
    assert_eq!(m["running"], false);
    let (code, text) = run(&ep, &["meas", "start", "Genelec 1 m"]).await;
    assert_ne!(code, 0);
    assert!(text.contains("sweep.run plays it"), "{text}");
    let (code, text) = run(
        &ep,
        &[
            "meas", "new", "spl", "--input", "1", "--level", "-50dbfs", "--name", "x",
        ],
    )
    .await;
    assert_ne!(code, 0);
    assert!(text.contains("--level applies to a sweep"), "{text}");

    // An import lands in the imported group; `trace move` files it under a measurement.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("1083 94cm.txt");
    std::fs::write(&file, "20 -3.0 10\n1000 -1.0 5\n20000 -4.0 0\n").unwrap();
    let (code, text) = run(&ep, &["trace", "import", file.to_str().unwrap()]).await;
    assert_eq!(code, 0, "{text}");
    assert_eq!(document(&text)["edit"]["owner"]["type"], "imported");
    let (code, text) = run(&ep, &["trace", "move", "1083 94cm", "--to", "Main L"]).await;
    assert_eq!(code, 0, "{text}");
    let moved = document(&text);
    assert_eq!(moved[0]["edit"]["owner"]["meas"], 1, "{moved}");
    let (code, text) = run(&ep, &["trace", "move", "1083 94cm"]).await;
    assert_ne!(code, 0);
    assert!(text.contains("--imported"), "{text}");

    // A math channel over it, listed under the measurement it is made on.
    let (code, text) = run(
        &ep,
        &[
            "math",
            "new",
            "Main L / trace:1083 94cm",
            "--under",
            "Main L",
            "--name",
            "Main ÷ file",
        ],
    )
    .await;
    assert_eq!(code, 0, "{text}");
    assert_eq!(
        document(&text)["config"]["kind"]["config"]["owner"]["meas"],
        1
    );

    // Deleting a measurement that owns traces: the operator says what becomes of them.
    let (code, text) = run(&ep, &["meas", "rm", "Main L"]).await;
    assert_ne!(code, 0);
    assert!(
        text.contains("owns 1 trace(s) and 1 math channel(s)"),
        "{text}"
    );
    assert!(text.contains("--keep-traces"), "{text}");
    // Kept, the math channel still computes from Main L: refused, nothing changes.
    let (code, text) = run(&ep, &["meas", "rm", "Main L", "--keep-traces"]).await;
    assert_ne!(code, 0);
    assert!(text.contains("operand"), "{text}");
    let (code, text) = run(&ep, &["meas", "rm", "Main L", "--delete-traces"]).await;
    assert_eq!(code, 0, "{text}");
    let (code, text) = run(&ep, &["trace", "list"]).await;
    assert_eq!(code, 0, "{text}");
    assert_eq!(document(&text), serde_json::json!([]));
    let (code, text) = run(&ep, &["meas", "list"]).await;
    assert_eq!(code, 0, "{text}");
    let names: Vec<String> = document(&text)
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["config"]["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["Genelec 1 m"]);

    // A target curve made or loaded for a measurement is listed under it with `--meas`.
    let (code, text) = run(
        &ep,
        &["trace", "target", "loudness", "--meas", "Genelec 1 m"],
    )
    .await;
    assert_eq!(code, 0, "{text}");
    let t = document(&text);
    assert_eq!(t["edit"]["owner"]["meas"], 2, "{t}");
    assert_eq!(t["edit"]["name"], "loudness 85 → 100 phon");
    let (code, text) = run(
        &ep,
        &[
            "trace",
            "import",
            file.to_str().unwrap(),
            "--target",
            "--meas",
            "Genelec 1 m",
        ],
    )
    .await;
    assert_eq!(code, 0, "{text}");
    assert_eq!(document(&text)["edit"]["owner"]["meas"], 2);
    let (code, text) = run(&ep, &["trace", "target", "flat", "--meas", "nope"]).await;
    assert_ne!(code, 0);
    assert!(text.contains("nope"), "{text}");
    h.shutdown();
}
