//! `ac2 levels`, `levels set` and `levels reset` against the daemon.

use std::time::{Duration, Instant};

use ac2_audio::FakeConfig;
use ac2_audio::fake::FakePath;
use ac2_proto::model::Signal;
use ac2_proto::units::Hz;

use super::{R, Rig, ac2, ok_json, set_tone};

/// A sine's true peak is its level in dBFS (0 dBFS: a full-scale sine).
const TONE: f64 = -12.0;
const QUIET: f64 = -30.0;

#[tokio::test(flavor = "multi_thread")]
async fn levels_set_shows_the_rig_and_reset_clears_the_holds() -> R {
    // Out 1 reaches inputs 1 and 3; inputs 2 and 4 are silent.
    let f = Rig::on(FakeConfig {
        paths: vec![FakePath::loopback(0, 0, 32), FakePath::loopback(0, 2, 32)],
        ..FakeConfig::default()
    })?;
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
    let (_c, lease) = f
        .tone(Signal::Sine { freq: Hz(1000.0) }, TONE, vec![0])
        .await?;
    let near =
        |v: &serde_json::Value, want: f64| v.as_f64().is_some_and(|v| (v - want).abs() < 0.1);
    // The tone at its level on input 1 (the meters have seen it once its max is there).
    let v = levels_until(&f, |v| near(&v["inputs"][0]["true_peak"], TONE)).await?;
    // Input 2 silent: the pair reads right quiet.
    assert_eq!(v["settings"]["ballistic"], "true_peak");
    let rows = v["inputs"].as_array().expect("array");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["input"], 1);
    assert_eq!(rows[0]["kind"], "line");
    assert_eq!(rows[0]["ballistic"], "true_peak");
    // True peak chosen: the reading and its max are the true peak.
    assert!(near(&rows[0]["ppm"], TONE), "{}", rows[0]["ppm"]);
    assert!(
        near(&rows[0]["true_peak"], TONE),
        "{}",
        rows[0]["true_peak"]
    );
    assert!(near(&rows[0]["max"], TONE), "{}", rows[0]["max"]);
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
    assert!(near(&loud[0]["max_true_peak"], TONE), "{}", loud[0]);
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
    // A loudness ballistic: the target is kept with the rest, each input's own column reads
    // its true peak (a mic's too), the numbers unchanged, and the loudness table marks the
    // window the bars read.
    let rig = ok_json(
        &f,
        &[
            "levels",
            "set",
            "--ballistic",
            "short-term",
            "--loudness-target",
            "-16lufs",
            "--json",
        ],
    )
    .await?;
    assert_eq!(rig["levels"]["ballistic"], "short_term");
    assert_eq!(rig["levels"]["loudness_target"], -16.0);
    assert_eq!(rig["levels"]["correlation_gate"], -40.0, "kept");
    let v = ok_json(&f, &["levels", "--json"]).await?;
    assert_eq!(v["settings"]["loudness_target"], -16.0);
    for row in v["inputs"].as_array().expect("array") {
        assert_eq!(row["ballistic"], "true_peak", "{row}");
        assert_eq!(row["ppm"], row["true_peak"], "{row}");
    }
    let t = ac2(&f, &["levels"]).await?;
    assert_eq!(t.code, 0, "{}", t.stderr);
    assert!(t.stdout.contains("Short (bars)"), "{}", t.stdout);
    assert!(!t.stdout.contains("Short loudness"), "{}", t.stdout);
    let t = ac2(&f, &["levels", "set", "--ballistic", "momentary"]).await?;
    assert!(
        t.stdout.contains("programme meters: Mom loudness;")
            && t.stdout.contains("loudness target -16 LUFS"),
        "{}",
        t.stdout
    );
    let t = ac2(&f, &["levels"]).await?;
    assert!(t.stdout.contains("Mom (bars)"), "{}", t.stdout);
    ok_json(&f, &["levels", "set", "--ballistic", "vu", "--json"]).await?;
    // Reset: the holds of the inputs given go, the others stay.
    set_tone(&lease, Signal::Sine { freq: Hz(1000.0) }, QUIET, vec![0]).await?;
    let v = levels_until(&f, |v| near(&v["inputs"][0]["true_peak"], QUIET)).await?;
    assert!(near(&v["inputs"][0]["max"], TONE), "held: {v}");
    let r = ok_json(&f, &["levels", "reset", "--input", "1", "--json"]).await?;
    assert_eq!(r["channels"], serde_json::json!([0]));
    // The hold starts again from the next block metered.
    let v = levels_until(&f, |v| v["inputs"][0]["max"].is_f64()).await?;
    assert!(
        near(&v["inputs"][0]["max"], QUIET),
        "the VU fallback's true peak: {v}"
    );
    assert!(near(&v["inputs"][2]["max"], TONE));
    let r = ok_json(&f, &["levels", "reset", "--json"]).await?;
    assert_eq!(r["channels"], serde_json::json!([]));
    let t = ac2(&f, &["levels", "reset"]).await?;
    assert!(t.stdout.contains("every input"), "{}", t.stdout);
    Ok(())
}

/// `levels --json` until `done` holds of it: the meters are read from the running stream.
async fn levels_until(f: &Rig, done: impl Fn(&serde_json::Value) -> bool) -> R<serde_json::Value> {
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        let v = ok_json(f, &["levels", "--json"]).await?;
        if done(&v) || Instant::now() > end {
            return Ok(v);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
