//! `math new|set` against a real daemon: operands by name, the expression typed or by
//! `--op`, the method and phase reference as typed, an edit, `meas list`, and the daemon's
//! refusals (an operand deleted under its channel, too few operands, levels with ÷) reach
//! the operator as errors.
#![allow(clippy::unwrap_used)]

use super::common::{document, rig, run_json as run};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn math_channels_by_name() {
    let (h, ep) = rig();
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
        &["meas", "new", "spectrum", "--input", "2", "--name", "Spec"],
    )
    .await;
    assert_eq!(code, 0, "{text}");

    // Too few operands, an unspaced expression, levels with ÷, mixed kinds.
    for (args, says) in [
        (
            &["math", "new", "--op", "avg", "--of", "Seat 1"][..],
            "at least 2",
        ),
        (
            &["math", "new", "Seat 1/Seat 2"][..],
            "one operator between spaces",
        ),
        (&["math", "new", "Spec / Spec"][..], "the same operand"),
        (
            &["math", "new", "Seat 1 / Spec"][..],
            "not one of the transfer",
        ),
        (
            &["math", "new", "Seat 1 / Nobody"][..],
            "no measurement or stored trace",
        ),
    ] {
        let (code, text) = run(&ep, args).await;
        assert_ne!(code, 0, "{args:?}: {text}");
        assert!(text.contains(says), "{args:?}: {text}");
    }

    // The expression typed: named after itself.
    let (code, text) = run(&ep, &["math", "new", "Seat 1 / Seat 2"]).await;
    assert_eq!(code, 0, "{text}");
    let m = document(&text);
    assert_eq!(m["config"]["name"], "Seat 1 ÷ Seat 2");
    let cfg = &m["config"]["kind"]["config"];
    assert_eq!(m["config"]["kind"]["type"], "math");
    assert_eq!(cfg["domain"], "transfer");
    assert_eq!(cfg["expr"]["type"], "binary");
    assert_eq!(cfg["expr"]["op"], "divide");
    assert_eq!(cfg["expr"]["a"]["meas"], 1);
    assert_eq!(cfg["expr"]["b"]["meas"], 2);

    // An average by --op, method and phase reference as typed.
    let (code, text) = run(
        &ep,
        &[
            "math",
            "new",
            "--name",
            "Audience",
            "--op",
            "avg",
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
    assert_eq!(cfg["expr"]["type"], "average");
    assert_eq!(cfg["expr"]["of"][1]["meas"], 3);
    assert_eq!(cfg["expr"]["method"], "complex");
    assert_eq!(cfg["reference"]["operand"]["meas"], 3);

    // Edited: another operator, the name kept.
    let (code, text) = run(
        &ep,
        &[
            "math",
            "set",
            "Seat 1 ÷ Seat 2",
            "--op",
            "add",
            "--a",
            "Seat 1",
            "--b",
            "Seat 3",
        ],
    )
    .await;
    assert_eq!(code, 0, "{text}");
    let m = document(&text);
    assert_eq!(m["config"]["name"], "Seat 1 ÷ Seat 2");
    assert_eq!(m["config"]["kind"]["config"]["expr"]["op"], "add");
    let (code, text) = run(&ep, &["math", "set", "Audience", "--method", "power"]).await;
    assert_eq!(code, 0, "{text}");
    assert_eq!(
        document(&text)["config"]["kind"]["config"]["expr"]["method"],
        "power"
    );

    // Listed with the measurements.
    let (code, text) = run(&ep, &["meas", "list"]).await;
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("Audience"), "{text}");
    let (code, text) = run(&ep, &["meas", "rm", "Seat 3"]).await;
    assert_ne!(code, 0, "{text}");
    assert!(text.contains("operand of the math channel"), "{text}");
    // Seat 2 is no operand any more: it goes.
    let (code, text) = run(&ep, &["meas", "rm", "Seat 2"]).await;
    assert_eq!(code, 0, "{text}");
    h.shutdown();
}
