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
    // True peak chosen: the reading and its max are the true peak.
    let near = |v: &serde_json::Value, want: f64| (v.as_f64().expect("number") - want).abs() < 1e-6;
    assert!(near(&rows[0]["ppm"], -11.8), "{}", rows[0]["ppm"]);
    assert!(
        near(&rows[0]["true_peak"], -11.8),
        "{}",
        rows[0]["true_peak"]
    );
    assert!(near(&rows[0]["max"], -11.8), "{}", rows[0]["max"]);
    assert_eq!(rows[0]["overs"], 0);
    assert_eq!(rows[0]["clip"], false);
    assert_eq!(
        rows[1]["ppm"], "-inf",
        "JSON has no infinity; null means something else"
    );
    assert_eq!(rows[1]["max"], "-inf");
    // Loudness: the line pair 1+2, then input 3 alone; input 4 (undeclared, so a mic) none.
    let loud = v["loudness"].as_array().expect("array");
    assert_eq!(loud.len(), 2, "{loud:?}");
    assert_eq!(
        loud[0]["source"],
        serde_json::json!({"type": "pair", "left": 1, "right": 2})
    );
    assert_eq!(
        loud[1]["source"],
        serde_json::json!({"type": "input", "input": 3})
    );
    assert!(near(&loud[0]["max_true_peak"], -11.8), "{}", loud[0]);
    for l in loud {
        // Before its first gating block passes, integrated is null, never "-inf".
        let int = &l["integrated"];
        assert!(int.is_null() || int.is_f64(), "{l}");
        let mom = &l["momentary"];
        assert!(mom == "-inf" || mom.is_f64(), "{l}");
        assert!(l["duration"].as_f64().expect("seconds") > 0.0, "{l}");
        assert!(l["since"].as_u64().expect("daemon wall ns") > 0, "{l}");
    }
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
    for word in [
        "true peak",
        "dBTP",
        "overs",
        "CLIP",
        "right quiet",
        "1 s",
        "TP max",
        "max Short",
        "1+2",
        "3 mono",
    ] {
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
    assert!(near(&v["inputs"][0]["max"], -11.8), "held");
    let r = ok_json(&f, &["levels", "reset", "--input", "1", "--json"]).await?;
    assert_eq!(r["channels"], serde_json::json!([0]));
    let v = ok_json(&f, &["levels", "--json"]).await?;
    assert!(
        near(&v["inputs"][0]["max"], -29.8),
        "the VU fallback's true peak"
    );
    assert!(near(&v["inputs"][2]["max"], -11.8));
    let r = ok_json(&f, &["levels", "reset", "--json"]).await?;
    assert_eq!(r["channels"], serde_json::json!([]));
    let t = ac2(&f, &["levels", "reset"]).await?;
    assert!(t.stdout.contains("every input"), "{}", t.stdout);
    Ok(())
}
