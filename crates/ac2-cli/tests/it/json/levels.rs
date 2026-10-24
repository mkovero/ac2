//! `ac2 levels`, `levels set` and `levels reset` against the fake daemon.

use super::{R, ac2, fake, ok_json};

#[tokio::test(flavor = "multi_thread")]
async fn levels_set_shows_the_rig_and_reset_clears_the_holds() -> R {
    let f = fake()?;
    f.lock().meters = true;
    // Without a session, there is nothing to meter.
    let r = ac2(&f, &["levels", "--json"]).await?;
    assert_ne!(r.code, 0);
    assert!(
        r.stdout.contains("no session is open"),
        "{} {}",
        r.stdout,
        r.stderr
    );
    ok_json(
        &f,
        &[
            "session",
            "open",
            "--backend",
            "fake",
            "--in",
            "1-4",
            "--json",
        ],
    )
    .await?;
    ok_json(
        &f,
        &[
            "session", "inputs", "--kind", "1=line", "--kind", "2=line", "--kind", "3=line",
            "--pair", "1,2=Main", "--json",
        ],
    )
    .await?;
    // Input 2 silent: the pair reads right quiet.
    f.lock().set_input_peak(1, f32::NEG_INFINITY);
    let v = ok_json(&f, &["levels", "--json"]).await?;
    assert_eq!(v["settings"]["ballistic"], "true_peak");
    let rows = v["inputs"].as_array().expect("array");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["input"], 1);
    assert_eq!(rows[0]["kind"], "line");
    assert_eq!(rows[0]["ballistic"], "true_peak");
    assert_eq!(rows[0]["ppm"], -12.0);
    assert!((rows[0]["true_peak"].as_f64().expect("number") + 11.8).abs() < 1e-6);
    assert_eq!(rows[0]["max"], -12.0);
    assert_eq!(rows[0]["overs"], 0);
    assert_eq!(rows[0]["clip"], false);
    assert_eq!(
        rows[1]["ppm"],
        serde_json::Value::Null,
        "-inf is not JSON: null"
    );
    let pairs = v["pairs"].as_array().expect("array");
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0]["left"], 1);
    assert_eq!(pairs[0]["right"], 2);
    assert_eq!(pairs[0]["quiet"], "right");
    assert_eq!(pairs[0]["correlation"], serde_json::Value::Null);
    assert_eq!(pairs[0]["window"], "long");
    // The text table uses the meter words.
    let t = ac2(&f, &["levels"]).await?;
    assert_eq!(t.code, 0, "{}", t.stderr);
    for word in ["true peak", "dBTP", "overs", "CLIP", "right quiet", "1 s"] {
        assert!(t.stdout.contains(word), "{word:?} missing in {}", t.stdout);
    }
    // Set: fields given change, the rig comes back; the window shows on the pair row.
    let rig = ok_json(
        &f,
        &[
            "levels",
            "set",
            "--ballistic",
            "type2",
            "--window",
            "0.3s",
            "--over-threshold",
            "-1dbtp",
            "--json",
        ],
    )
    .await?;
    assert_eq!(rig["levels"]["ballistic"], "type2");
    assert_eq!(rig["levels"]["correlation_window"], "short");
    assert_eq!(rig["levels"]["over_threshold"], -1.0);
    assert_eq!(rig["levels"]["correlation_gate"], -60.0);
    let rig = ok_json(&f, &["levels", "set", "--gate", "-40dbfs", "--json"]).await?;
    assert_eq!(rig["levels"]["ballistic"], "type2", "kept");
    assert_eq!(rig["levels"]["correlation_gate"], -40.0);
    let v = ok_json(&f, &["levels", "--json"]).await?;
    assert_eq!(v["pairs"][0]["window"], "short");
    assert_eq!(v["inputs"][0]["ballistic"], "type2");
    let t = ac2(&f, &["levels", "set", "--ballistic", "vu"]).await?;
    assert!(t.stdout.contains("programme meters: VU"), "{}", t.stdout);
    // VU chosen, not offered on a line input without a line calibration: the row says what
    // it reads.
    let v = ok_json(&f, &["levels", "--json"]).await?;
    assert_eq!(v["inputs"][0]["ballistic"], "true_peak");
    // Reset: the holds of the inputs given go, the others stay.
    f.lock().set_input_peak(0, -30.0);
    let v = ok_json(&f, &["levels", "--json"]).await?;
    assert_eq!(v["inputs"][0]["max"], -12.0, "held");
    let r = ok_json(&f, &["levels", "reset", "--input", "1", "--json"]).await?;
    assert_eq!(r["channels"], serde_json::json!([0]));
    let v = ok_json(&f, &["levels", "--json"]).await?;
    assert_eq!(v["inputs"][0]["max"], -30.0);
    assert_eq!(v["inputs"][2]["max"], -12.0);
    let r = ok_json(&f, &["levels", "reset", "--json"]).await?;
    assert_eq!(r["channels"], serde_json::json!([]));
    let t = ac2(&f, &["levels", "reset"]).await?;
    assert!(t.stdout.contains("every input"), "{}", t.stdout);
    Ok(())
}
