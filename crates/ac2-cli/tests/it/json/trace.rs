//! Stored traces.

use ac2_cli::Cli;
use ac2_client::fake::{FakeDaemon, FakeOptions};
use clap::Parser;
use serde_json::json;

use super::{R, ac2, ok_json};

fn fixture(name: &str) -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ac2-traces/tests/fixtures")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn trace_commands_json() -> R {
    let dir = tempfile::tempdir()?;
    let f = FakeDaemon::start(FakeOptions {
        session_dir: Some(dir.path().join("sessions")),
        ..FakeOptions::default()
    })?;
    ok_json(
        &f,
        &[
            "meas", "new", "tf", "--ref", "1", "--meas", "2", "--name", "main", "--start", "--json",
        ],
    )
    .await?;
    ok_json(&f, &["delay", "set", "main", "10ms", "--json"]).await?;
    let a = ok_json(
        &f,
        &[
            "trace", "capture", "main", "--name", "a", "--slot", "1", "--json",
        ],
    )
    .await?;
    assert_eq!(
        a,
        json!({
            "id": 2,
            "edit": {
                "name": "a",
                "color": { "r": 86, "g": 180, "b": 233 },
                "visible": true,
                "locked": false,
                "order": 2,
                "offset": 0.0,
                "polarity": "normal",
                // The typed 10 ms moved the live curve from its arrival (0): the capture
                // carries that move as its display nudge.
                "delay_nudge": 0.01,
                "slot": 1,
                "smoothing": null,
                "owner": { "type": "meas", "meas": 1 }
            },
            "kind": { "type": "transfer" },
            "source": {
                "type": "captured",
                "meas": 1,
                "meas_name": "main",
                "epoch": 1,
                "at_sample": 48000
            },
            "grid_id": 0x79ec_3d16_ae0e_94d0u64,
            "delay": 0.01,
            "depth": { "type": "equal_confidence" },
            "cal": { "type": "uncalibrated" },
            "mic": null,
            "mic_curve": null,
            "created_at": 1790000000000000000u64
        })
    );
    // The slot range is checked by the parser.
    assert!(
        Cli::try_parse_from([
            "ac2", "trace", "capture", "main", "--name", "x", "--slot", "10"
        ])
        .is_err()
    );
    ok_json(
        &f,
        &[
            "trace", "capture", "main", "--name", "b", "--slot", "2", "--json",
        ],
    )
    .await?;

    let l = ok_json(&f, &["trace", "list", "--json"]).await?;
    assert_eq!(l.as_array().map(Vec::len), Some(2));
    let s = ok_json(&f, &["trace", "show", "a", "--json"]).await?;
    assert_eq!(s, a);
    let d = ok_json(&f, &["trace", "show", "2", "--data", "--json"]).await?;
    assert_eq!(d["meta"], a);
    assert_eq!(d["mag_db"].as_array().map(Vec::len), Some(480));
    assert!(d["coherence"].is_array());
    // Both captured at the same applied delay: no difference.
    let dd = ok_json(&f, &["trace", "delay-diff", "a", "b", "--json"]).await?;
    assert_eq!(dd["difference_s"], 0.0);
    assert_eq!(dd["a_delay_s"], 0.01);
    assert!(
        dd["text"]
            .as_str()
            .is_some_and(|t| t.ends_with("mm @ 20 °C")),
        "{dd}"
    );

    let avg = ok_json(
        &f,
        &[
            "trace", "average", "a", "b", "--name", "avg", "--method", "complex", "--ref", "b",
            "--json",
        ],
    )
    .await?;
    assert_eq!(
        avg["source"],
        json!({
            "type": "average",
            "traces": [2, 3],
            "method": "complex",
            "reference": { "type": "trace", "trace": 3 }
        })
    );
    assert_eq!(avg["delay"], 0.01);
    let fixed = ok_json(
        &f,
        &[
            "trace",
            "average",
            "a",
            "b",
            "--name",
            "avg2",
            "--ref-delay",
            "12.5ms",
            "--json",
        ],
    )
    .await?;
    assert_eq!(
        fixed["source"]["reference"],
        json!({ "type": "fixed", "delay": 0.0125 })
    );
    // A math channel of the two stored traces, by name.
    let m = ok_json(&f, &["math", "new", "a / b", "--json"]).await?;
    assert_eq!(m["config"]["name"], "a ÷ b");
    assert_eq!(
        m["config"]["kind"]["config"]["expr"],
        json!({
            "type": "binary",
            "a": { "type": "trace", "trace": 2 },
            "op": "divide",
            "b": { "type": "trace", "trace": 3 }
        })
    );

    // Display smoothing from the command line: 1/N or N, the mode kept unless asked.
    let sm = ok_json(&f, &["trace", "smooth", "a", "1/12", "--json"]).await?;
    assert_eq!(
        sm["edit"]["smoothing"],
        json!({ "fraction": "twelfth", "mode": "magnitude_phase" })
    );
    let sm = ok_json(
        &f,
        &["trace", "smooth", "a", "6", "--magnitude-only", "--json"],
    )
    .await?;
    assert_eq!(
        sm["edit"]["smoothing"],
        json!({ "fraction": "sixth", "mode": "magnitude" })
    );
    let sm = ok_json(&f, &["trace", "smooth", "a", "1/3", "--json"]).await?;
    assert_eq!(sm["edit"]["smoothing"]["mode"], "magnitude");
    let sm = ok_json(&f, &["trace", "smooth", "a", "1/3", "--phase", "--json"]).await?;
    assert_eq!(sm["edit"]["smoothing"]["mode"], "magnitude_phase");
    let sm = ok_json(&f, &["trace", "smooth", "a", "none", "--json"]).await?;
    assert_eq!(sm["edit"]["smoothing"], json!(null));
    assert!(Cli::try_parse_from(["ac2", "trace", "smooth", "a", "1/5"]).is_err());
    let human = ac2(&f, &["trace", "smooth", "b", "1/24"]).await?;
    assert!(
        human.stdout.contains("1/24 octave, magnitude and phase"),
        "{}",
        human.stdout
    );

    // A mic curve on a stored trace: none in the mic library yet, then the imported one.
    let r = ac2(&f, &["trace", "mic", "a", "M30", "--json"]).await?;
    assert_eq!(r.json()?["error"]["code"], "usage");
    let curve = dir.path().join("M30.txt");
    std::fs::write(
        &curve,
        "20 -1.5
1000 0
20000 2
",
    )?;
    ok_json(
        &f,
        &[
            "cal",
            "curve",
            "import",
            &curve.to_string_lossy(),
            "--mic",
            "M30",
            "--json",
        ],
    )
    .await?;
    let mc = ok_json(&f, &["trace", "mic", "a", "M30", "--json"]).await?;
    assert_eq!(mc["mic_curve"]["mic"], "M30");
    assert_eq!(mc["mic_curve"]["curve"]["label"], "M30");
    assert_eq!(mc["mic_curve"]["f_norm"], 1000.0);
    let human = ac2(&f, &["trace", "show", "a"]).await?;
    assert!(
        human
            .stdout
            .contains("M30 (curve M30 applied after capture, 0 dB at 1000 Hz, file M30.txt)"),
        "{}",
        human.stdout
    );
    let off = ok_json(&f, &["trace", "mic", "a", "none", "--json"]).await?;
    assert_eq!(off["mic_curve"], json!(null));

    // Shown or hidden, and slots, for any trace: the average has none until given one.
    let hidden = ok_json(&f, &["trace", "display", "avg", "off", "--json"]).await?;
    assert_eq!(hidden["edit"]["visible"], false);
    assert_eq!(hidden["edit"]["slot"], json!(null));
    let shown = ok_json(&f, &["trace", "display", "avg", "on", "--json"]).await?;
    assert_eq!(shown["edit"]["visible"], true);
    // Slot 1 moves from "a" to the average.
    let slotted = ok_json(&f, &["trace", "slot", "avg", "1", "--json"]).await?;
    assert_eq!(slotted["edit"]["slot"], 1);
    let l = ok_json(&f, &["trace", "list", "--json"]).await?;
    let slot_of = |name: &str| {
        l.as_array()
            .and_then(|a| a.iter().find(|t| t["edit"]["name"] == name))
            .map(|t| t["edit"]["slot"].clone())
    };
    assert_eq!(slot_of("a"), Some(json!(null)));
    assert_eq!(slot_of("avg"), Some(json!(1)));
    let freed = ok_json(&f, &["trace", "slot", "avg", "none", "--json"]).await?;
    assert_eq!(freed["edit"]["slot"], json!(null));
    let human = ac2(&f, &["trace", "slot", "b", "3"]).await?;
    assert!(human.stdout.contains(" 3 "), "{}", human.stdout);
    assert!(Cli::try_parse_from(["ac2", "trace", "slot", "a", "10"]).is_err());
    assert!(Cli::try_parse_from(["ac2", "trace", "display", "a", "maybe"]).is_err());

    let rew = fixture("rew_export.txt");
    let i = ok_json(&f, &["trace", "import", &rew, "--json"]).await?;
    assert_eq!(i["edit"]["name"], "rew_export");
    assert_eq!(
        i["source"],
        json!({
            "type": "imported",
            "file_name": "rew_export.txt",
            "format": "analyzer_text",
            "notes": [],
            "ir_file": null
        })
    );
    let house = fixture("house_curve.txt");
    let t = ok_json(
        &f,
        &[
            "trace", "import", &house, "--target", "--name", "house", "--json",
        ],
    )
    .await?;
    assert_eq!(t["kind"], json!({ "type": "target" }));
    assert_eq!(t["edit"]["name"], "house");
    let bad = dir.path().join("bad.txt");
    std::fs::write(&bad, "20 1\n30 x\n")?;
    let r = ac2(&f, &["trace", "import", &bad.to_string_lossy(), "--json"]).await?;
    assert_eq!(r.code, 1);
    assert_eq!(
        r.json()?["error"]["detail"],
        json!({ "type": "import", "line": 2, "problem": "bad_number" })
    );
    assert_eq!(r.json()?["error"]["code"], "invalid");

    ok_json(&f, &["meas", "rm", "a ÷ b", "--json"]).await?;

    // Sessions.
    let saved = ok_json(&f, &["session", "save-as", "show", "--json"]).await?;
    assert_eq!(saved["name"], "show");
    assert_eq!(saved["measurements"], 1);
    assert_eq!(saved["traces"], 6);
    let list = ok_json(&f, &["session", "list", "--json"]).await?;
    assert_eq!(list.as_array().map(Vec::len), Some(1));
    assert_eq!(list[0]["name"], "show");
    ok_json(&f, &["trace", "rm", "house", "--json"]).await?;
    // Unsaved changes are not dropped unless told.
    let r = ac2(&f, &["session", "load", "show", "--json"]).await?;
    assert_eq!(r.code, 4, "refused");
    let e = r.json()?;
    let msg = e["error"]["msg"].as_str().ok_or("msg")?;
    assert!(msg.contains("show has unsaved changes"), "{msg}");
    let status = ok_json(&f, &["session", "status", "--json"]).await?;
    assert_eq!(status["working"]["dirty"], true);
    assert_eq!(
        status["working"]["origin"],
        json!({ "type": "name", "name": "show" })
    );
    let loaded = ok_json(&f, &["session", "load", "show", "--discard", "--json"]).await?;
    assert_eq!(loaded["traces"], 6);
    let dump = ok_json(&f, &["state", "dump"]).await?;
    assert_eq!(dump["state"]["traces"].as_array().map(Vec::len), Some(6));
    assert_eq!(dump["state"]["generator"]["owner"], json!(null));
    assert_eq!(dump["state"]["generator"]["armed"], false);
    let by_path = dir.path().join("elsewhere");
    let p = ok_json(
        &f,
        &["session", "save-as", &by_path.to_string_lossy(), "--json"],
    )
    .await?;
    assert_eq!(p["name"], "elsewhere");
    assert!(by_path.join("session.json").exists());
    let r = ac2(&f, &["session", "load", "nope", "--json"]).await?;
    assert_eq!(r.json()?["error"]["code"], "not_found");
    Ok(())
}
