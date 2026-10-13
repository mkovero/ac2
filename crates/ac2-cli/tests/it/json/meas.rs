//! Sessions, measurements, delay finding, and naming measurements.

use serde_json::{Value, json};

use super::{R, ac2, fake, ok_json};

#[tokio::test(flavor = "multi_thread")]
async fn delay_find_bands_ambiguity_and_refusal() -> R {
    use ac2_client::fake::FakeFinding;
    let f = fake()?;
    ok_json(
        &f,
        &[
            "meas",
            "new",
            "tf",
            "--ref",
            "1",
            "--meas",
            "2",
            "--name",
            "sub",
            "--fast-lf",
            "--json",
        ],
    )
    .await
    .map(|m| assert_eq!(m["config"]["kind"]["config"]["depth"]["type"], "fast_lf"))?;

    // Ambiguous: candidates listed, the third one inserted (1-based on the command line).
    f.lock().finding = FakeFinding::Ambiguous;
    let d = ok_json(
        &f,
        &[
            "delay",
            "find",
            "sub",
            "--band",
            "sub",
            "--observation",
            "8s",
            "--insert",
            "3",
            "--json",
        ],
    )
    .await?;
    assert_eq!(d["finding"]["outcome"]["type"], "ambiguous");
    assert_eq!(d["finding"]["band"]["type"], "sub");
    assert_eq!(
        d["finding"]["outcome"]["ranked"].as_array().map(Vec::len),
        Some(3)
    );
    assert_eq!(d["inserted"]["delay"]["applied"], 0.0134);
    let r = ac2(&f, &["delay", "find", "sub", "--band", "80hz-800hz"]).await?;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(
        r.stdout.starts_with("AMBIGUOUS · near the threshold"),
        "{}",
        r.stdout
    );
    assert!(r.stdout.contains("12.50 ms  −11.5 dB") || r.stdout.contains("12.50 ms"));
    assert!(r.stdout.contains("--pick 1|2|3"), "{}", r.stdout);
    assert!(
        r.stdout.contains("band           custom 80–800 Hz"),
        "{}",
        r.stdout
    );

    // Refused: the reasons are typed in JSON and spelled out for people; nothing inserted.
    f.lock().finding = FakeFinding::NoEstimate;
    let d = ok_json(&f, &["delay", "find", "sub", "--insert", "--json"]).await?;
    assert_eq!(d["finding"]["outcome"]["type"], "no_estimate");
    assert_eq!(
        d["finding"]["outcome"]["reasons"],
        json!([{ "type": "low_psr" }, { "type": "low_band_snr" }])
    );
    assert_eq!(d["inserted"], Value::Null);
    let r = ac2(&f, &["delay", "find", "sub"]).await?;
    assert_eq!(r.code, 0);
    assert!(
        r.stdout
            .starts_with("NO ESTIMATE · no clear peak, too noisy in band"),
        "{}",
        r.stdout
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn session_meas_delay_trace_flow() -> R {
    let f = fake()?;
    let s = ok_json(
        &f,
        &[
            "session",
            "open",
            "--backend",
            "fake",
            "--in",
            "1-4",
            "--rate",
            "48khz",
            "--json",
        ],
    )
    .await?;
    assert_eq!(s["epoch"], 2);
    assert_eq!(s["open"]["input_device"], "fake:loop");
    assert_eq!(s["open"]["config"]["input_channels"], json!([0, 1, 2, 3]));
    assert_eq!(s["open"]["config"]["sample_rate_hz"], 48000);

    // A backend without devices is refused, never silently replaced.
    let r = ac2(
        &f,
        &[
            "session",
            "open",
            "--backend",
            "jack",
            "--in",
            "1",
            "--json",
        ],
    )
    .await?;
    assert_eq!(r.code, 1);
    assert_eq!(r.json()?["error"]["code"], "usage");

    let m = ok_json(
        &f,
        &[
            "meas", "new", "spl", "--input", "3", "--weight", "a", "--name", "foh", "--json",
        ],
    )
    .await?;
    assert_eq!(
        m,
        json!({
            "id": 1,
            "config": {
                "name": "foh",
                "kind": { "type": "spl", "config": {
                    "input": 2, "weighting": "a", "time_weighting": "fast", "peak_weighting": "c",
                    "leq": {
                        "windows": ([60.0, 300.0, 600.0, 1800.0, 3600.0].map(|d| json!({
                            "duration": d, "weighting": "a", "limit": null, "warn_margin": 3.0
                        }))),
                        "horizon": 60.0,
                        "peaks": { "lcpeak": null, "lafmax": null }
                    },
                    "position": null,
                    "bands": null
                }}
            },
            "config_rev": 2,
            "running": false,
            "delay": null,
            "grid_id": null,
            "colour": null,
            "layout": null
        })
    );
    // Clearing panes it never had leaves it as it is.
    let m = ok_json(&f, &["meas", "layout", "clear", "foh", "--json"]).await?;
    assert_eq!(m["layout"], serde_json::Value::Null);
    assert_eq!(m["config_rev"], 2);

    let tf = ok_json(
        &f,
        &[
            "meas", "new", "tf", "--ref", "1", "--meas", "2", "--name", "main-l", "--smooth", "6",
            "--start", "--json",
        ],
    )
    .await?;
    assert_eq!(tf["id"], 2);
    assert_eq!(tf["running"], true);
    assert_eq!(
        tf["config"]["kind"],
        json!({ "type": "transfer", "config": {
            "reference_input": 0,
            "measurement_input": 1,
            "averaging": { "type": "fifo", "blocks": 8 },
            "resolution": "forty_eighth",
            "smoothing": { "fraction": "sixth", "mode": "magnitude_phase" },
            "depth": { "type": "equal_confidence" }
        }})
    );

    let list = ok_json(&f, &["meas", "list", "--json"]).await?;
    let names: Vec<&str> = list
        .as_array()
        .ok_or("not an array")?
        .iter()
        .filter_map(|m| m["config"]["name"].as_str())
        .collect();
    assert_eq!(names, ["foh", "main-l"]);

    let d = ok_json(&f, &["delay", "find", "main-l", "--insert", "--json"]).await?;
    assert_eq!(d["finding"]["outcome"]["type"], "accepted");
    assert_eq!(d["finding"]["outcome"]["first"]["delay"], 0.0125);
    assert_eq!(d["finding"]["outcome"]["strongest"]["delay"], 0.0127);
    assert_eq!(d["finding"]["band"]["type"], "full");
    assert_eq!(d["finding"]["confidence"]["psr_db"], 24.0);
    assert_eq!(d["finding"]["candidates"].as_array().map(Vec::len), Some(2));
    assert_eq!(d["inserted"]["delay"]["applied"], 0.0125);
    assert_eq!(d["inserted"]["delay"]["applied_samples"], 600.0);

    let d = ok_json(&f, &["delay", "set", "main-l", "480samples", "--json"]).await?;
    assert_eq!(d["delay"]["applied"], 0.01);
    let d = ok_json(&f, &["delay", "nudge", "main-l", "-0.25samples", "--json"]).await?;
    let n = d["delay"]["applied_samples"].as_f64().unwrap_or(f64::NAN);
    assert!((n - 479.75).abs() < 1e-9, "{n}");
    // One delay and its offset from the inserted arrival (12.5 ms), as the app says it.
    let r = ac2(&f, &["meas", "list"]).await?;
    assert!(
        r.stdout.contains("9.995 ms (−2.505 ms from arrival)"),
        "{}",
        r.stdout
    );
    let r = ac2(&f, &["delay", "nudge", "main-l", "0samples"]).await?;
    assert!(
        r.stdout
            .contains("delay 9.995 ms (−2.505 ms from arrival) · 479.75 samples"),
        "{}",
        r.stdout
    );
    let d = ok_json(&f, &["delay", "track", "2", "on", "--json"]).await?;
    assert_eq!(d["delay"]["tracking"], true);
    // Delay commands refuse non-transfer measurements.
    let r = ac2(&f, &["delay", "find", "foh", "--json"]).await?;
    assert_eq!(r.code, 1);
    assert_eq!(r.json()?["error"]["code"], "usage");

    let t = ok_json(
        &f,
        &["trace", "capture", "main-l", "--name", "l-pre-eq", "--json"],
    )
    .await?;
    assert_eq!(t["edit"]["name"], "l-pre-eq");
    assert_eq!(t["source"]["type"], "captured");
    assert_eq!(t["source"]["meas"], 2);
    // The trace records the exact applied delay, fraction included (480 − 0.25 samples).
    let td = t["delay"].as_f64().unwrap_or(f64::NAN);
    assert!((td - 479.75 / 48_000.0).abs() < 1e-12, "{td}");
    let traces = ok_json(&f, &["trace", "list", "--json"]).await?;
    assert_eq!(traces.as_array().map(Vec::len), Some(1));
    let csv = ac2(&f, &["trace", "export", "l-pre-eq", "--csv", "-"]).await?;
    assert_eq!(csv.code, 0);
    assert!(
        csv.stdout
            .starts_with("# ac2 trace export v6\n# name: l-pre-eq\n# kind: transfer\n"),
        "{}",
        csv.stdout
    );
    assert!(
        csv.stdout
            .contains("\nfreq_hz,mag_db,phase_deg,coherence\n")
    );
    assert_eq!(csv.stdout.lines().count(), 17 + 480);

    let rm = ok_json(&f, &["meas", "rm", "foh", "--json"]).await?;
    assert_eq!(rm["deleted"], 1);
    let missing = ac2(&f, &["meas", "start", "foh", "--json"]).await?;
    assert_eq!(
        missing.json()?,
        json!({ "error": { "code": "usage", "msg": "no measurement named \"foh\"" } })
    );

    let dump = ok_json(&f, &["state", "dump"]).await?;
    assert_eq!(dump["rev"], json!(f.lock().rev.0));
    assert_eq!(
        dump["state"]["measurements"].as_array().map(Vec::len),
        Some(1)
    );
    Ok(())
}

/// A measurement named with digits is found by its name; a reference that is one
/// measurement's id and another's name is refused, naming both.
#[tokio::test(flavor = "multi_thread")]
async fn numeric_names_resolve_and_ambiguity_is_refused() -> R {
    let f = fake()?;
    let tf = |name: &'static str| {
        [
            "meas", "new", "tf", "--ref", "1", "--meas", "2", "--name", name, "--json",
        ]
    };
    let a = ok_json(&f, &tf("1083")).await?;
    assert_eq!(a["id"], 1);
    let d = ok_json(&f, &["delay", "find", "1083", "--json"]).await?;
    assert_eq!(d["finding"]["outcome"]["type"], "accepted");
    let m = ok_json(&f, &["meas", "start", "1083", "--json"]).await?;
    assert_eq!(m["id"], 1);

    let b = ok_json(&f, &tf("1")).await?;
    assert_eq!(b["id"], 2);
    // "2" is only measurement 2's id.
    let m = ok_json(&f, &["meas", "start", "2", "--json"]).await?;
    assert_eq!(m["id"], 2);
    // "1" is measurement 1's id and measurement 2's name.
    let r = ac2(&f, &["delay", "find", "1", "--json"]).await?;
    assert_eq!(r.code, 1);
    let e = r.json()?;
    assert_eq!(e["error"]["code"], "usage");
    let msg = e["error"]["msg"].as_str().ok_or("msg")?;
    assert!(
        msg.contains("ambiguous")
            && msg.contains("measurement 1 (\"1083\") by id")
            && msg.contains("measurement 2 (\"1\") by name"),
        "{msg}"
    );
    let r = ac2(&f, &["delay", "find", "999", "--json"]).await?;
    assert_eq!(r.code, 1);
    assert_eq!(
        r.json()?["error"]["msg"],
        "no measurement with id or name 999"
    );
    Ok(())
}
