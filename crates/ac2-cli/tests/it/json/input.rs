//! Input setup, mic curves and calibration.

use serde_json::{Value, json};

use super::{R, ac2, fake, ok_json};

/// The row of 1-based input `n` in an inputs JSON table.
fn input_row(v: &Value, n: u64) -> Value {
    v.as_array()
        .and_then(|a| a.iter().find(|r| r["input"] == n))
        .cloned()
        .unwrap_or(Value::Null)
}

fn mic_curve_file(name: &str) -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/mic_curves")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn input_setup_mic_curve_and_cal_flow() -> R {
    let f = fake()?;
    // A session: a calibration of this device's input is verified, not just matched by mic.
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
    // No mic name on input 2 yet: cal commands ask for one instead of guessing.
    let r = ac2(
        &f,
        &["cal", "spl", "--input", "2", "--ref", "94db", "--json"],
    )
    .await?;
    assert_ne!(r.code, 0);
    assert!(
        r.stdout.contains("no mic name"),
        "{} {}",
        r.stdout,
        r.stderr
    );

    let i = ok_json(
        &f,
        &[
            "session",
            "inputs",
            "--mic",
            "2=MM1 34804",
            "--mic",
            "3=M30",
            "--curve",
            "3=off",
            "--json",
        ],
    )
    .await?;
    let two = input_row(&i, 2);
    assert_eq!(two["mic"], "MM1 34804");
    assert_eq!(two["curve"], json!({ "type": "not_chosen" }));
    // Nothing stored for the mic: said so, never just "on".
    assert_eq!(two["curve_text"], "no curve stored for MM1 34804");
    assert_eq!(input_row(&i, 3)["curve"], json!({ "type": "off" }));
    // A curve the mic does not have is refused.
    let r = ac2(&f, &["cal", "use", "3", "90°", "--json"]).await?;
    assert_ne!(r.code, 0);
    assert_eq!(r.json()?["error"]["code"], "invalid");
    // Only the named rows change; `IN=` clears a name.
    let i = ok_json(&f, &["session", "inputs", "--mic", "3=", "--json"]).await?;
    assert_eq!(input_row(&i, 3)["mic"], Value::Null);
    assert_eq!(input_row(&i, 2)["mic"], "MM1 34804");

    // The input's mic name is the default for --mic.
    let c = ok_json(
        &f,
        &["cal", "spl", "--input", "2", "--ref", "114db", "--json"],
    )
    .await?;
    assert_eq!(c["key"]["mic"], "MM1 34804");

    // Both curves of the capsule: labels from the files, the first one chosen (the mic's
    // only curve then), the second one not.
    let zero = mic_curve_file("449350_34804_0Grad.txt");
    let ninety = mic_curve_file("449350_34804_90Grad.txt");
    let m = ok_json(
        &f,
        &["cal", "curve", "import", &zero, "--input", "2", "--json"],
    )
    .await?;
    assert_eq!(m["curves"][0]["label"], "0°");
    assert_eq!(m["curves"][0]["file_name"], "449350_34804_0Grad.txt");
    assert_eq!(m["curves"][0]["stated_sensitivity"], 15.0);
    let m = ok_json(
        &f,
        &["cal", "curve", "import", &ninety, "--input", "2", "--json"],
    )
    .await?;
    assert_eq!(m["curves"][1]["label"], "90°");
    let l = ok_json(&f, &["cal", "list", "--json"]).await?;
    assert_eq!(l["mics"][0]["name"], "MM1 34804");
    assert_eq!(l["mics"][0]["curves"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        input_row(&l["inputs"], 2)["curve"],
        json!({ "type": "curve", "label": "0°" })
    );
    assert_eq!(input_row(&l["inputs"], 2)["cal"]["type"], "verified");
    // The text lists the curves per mic and what each input uses.
    let text = ac2(&f, &["cal", "list"]).await?;
    for want in [
        "MM1 34804",
        "449350_34804_90Grad.txt",
        "15.0 mV/Pa",
        "verified · 114.0 dB SPL at 1.00 kHz",
    ] {
        assert!(text.stdout.contains(want), "{want:?} in\n{}", text.stdout);
    }

    // Switching: one command, the input table says what is in use.
    let i = ok_json(&f, &["cal", "use", "2", "90°", "--json"]).await?;
    assert_eq!(input_row(&i, 2)["curve_text"], "90°");
    assert_eq!(input_row(&i, 2)["curve_applied"]["label"], "90°");
    let i = ok_json(&f, &["cal", "use", "2", "off", "--json"]).await?;
    assert_eq!(input_row(&i, 2)["curve_text"], "off");
    assert_eq!(input_row(&i, 2)["curve_applied"], Value::Null);
    let r = ac2(&f, &["cal", "use", "2", "45°", "--json"]).await?;
    assert_ne!(r.code, 0);
    ok_json(&f, &["cal", "use", "2", "90°", "--json"]).await?;
    // `ac2 status` shows it too.
    let st = ac2(&f, &["status"]).await?;
    assert!(st.stdout.contains("MM1 34804"), "{}", st.stdout);

    // Deleting the chosen curve: the input says the curve is not stored.
    ok_json(
        &f,
        &["cal", "curve", "rm", "--mic", "MM1 34804", "90°", "--json"],
    )
    .await?;
    let l = ok_json(&f, &["cal", "list", "--json"]).await?;
    assert_eq!(l["mics"][0]["curves"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        input_row(&l["inputs"], 2)["curve_text"],
        "90° — not stored for MM1 34804"
    );
    // Renaming: the inputs that use the curve follow.
    ok_json(&f, &["cal", "use", "2", "0°", "--json"]).await?;
    ok_json(
        &f,
        &[
            "cal",
            "curve",
            "rename",
            "--mic",
            "MM1 34804",
            "0°",
            "on axis",
            "--json",
        ],
    )
    .await?;
    let l = ok_json(&f, &["cal", "list", "--json"]).await?;
    assert_eq!(input_row(&l["inputs"], 2)["curve_text"], "on axis");

    // A refused file: typed error, nothing stored.
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("bad.txt");
    std::fs::write(&file, "only text\n")?;
    let path = file.to_string_lossy().into_owned();
    let r = ac2(
        &f,
        &["cal", "curve", "import", &path, "--input", "2", "--json"],
    )
    .await?;
    assert_ne!(r.code, 0);
    let err = r.json()?;
    assert_eq!(err["error"]["code"], "invalid", "{err}");

    // `cal electrical`: the acoustic calibration of input 2 is not replaced by accident.
    let r = ac2(
        &f,
        &[
            "cal",
            "electrical",
            "--input",
            "2",
            "--volts",
            "15mv",
            "--json",
        ],
    )
    .await?;
    assert_ne!(r.code, 0);
    assert_eq!(r.json()?["error"]["code"], "refused");
    // Replaced on purpose, the sensitivity from the data sheet of the mic's curve (the fake
    // input reads −20 dBFS: 0 dBFS = 150 mV, 150 mV / 15 mV/Pa = 10 Pa = 114.0 dB SPL).
    let e = ok_json(
        &f,
        &[
            "cal",
            "electrical",
            "--input",
            "2",
            "--volts",
            "15mv",
            "--replace-acoustic",
            "--json",
        ],
    )
    .await?;
    let m = &e["spl"]["method"];
    assert_eq!(m["type"], "electrical");
    assert_eq!(m["connection"], "in_line");
    assert_eq!(m["mic_sensitivity"], 15.0);
    assert_eq!(m["mic_sensitivity_from"]["type"], "data_sheet");
    assert_eq!(m["uncertainty"], 1.0);
    let s = e["spl"]["sensitivity"].as_f64().unwrap_or_default();
    assert!((s - 113.979_400_086_720_4).abs() < 1e-9, "{s}");
    // The text names the numbers it rests on and what to do with the phantom power.
    let text = ac2(
        &f,
        &[
            "cal",
            "electrical",
            "--input",
            "2",
            "--volts",
            "0.1v",
            "--freq",
            "400hz",
            "--sensitivity",
            "-40dbv/pa",
            "--method",
            "injected",
            "--uncertainty",
            "0.5db",
        ],
    )
    .await?;
    assert_eq!(text.code, 0, "{}", text.stderr);
    for want in [
        "electrical injected, 100.0 mV at 400 Hz, 0 dBFS = 1.000 V, 10.0 mV/Pa",
        "±0.5 dB",
        "0 dBFS = 134.0 dB SPL on input 2 (MM1 34804)",
        "note: the tone was 400 Hz",
        "switch phantom power back ON",
    ] {
        assert!(text.stdout.contains(want), "{want:?} in\n{}", text.stdout);
    }
    let text = ac2(&f, &["cal", "list"]).await?;
    for want in [
        "electrical injected",
        "±0.5 dB",
        "verified · electrical (injected, 10.0 mV/Pa) ±0.5 dB",
    ] {
        assert!(text.stdout.contains(want), "{want:?} in\n{}", text.stdout);
    }
    // The MM1's 330 Ω loaded by a 2 kΩ input: 15 mV/Pa open circuit is 12.88 mV/Pa at the
    // input, so 0 dBFS (150 mV) is 1.327 dB more SPL than unloaded.
    let e = ok_json(
        &f,
        &[
            "cal",
            "electrical",
            "--input",
            "2",
            "--volts",
            "15mv",
            "--mic-impedance",
            "330ohm",
            "--input-impedance",
            "2kohm",
            "--json",
        ],
    )
    .await?;
    let m = &e["spl"]["method"];
    assert_eq!(m["mic_sensitivity"], 15.0);
    assert_eq!(
        m["loading"],
        json!({ "mic_impedance": 330.0, "input_impedance": 2000.0 })
    );
    let s = e["spl"]["sensitivity"].as_f64().unwrap_or_default();
    let want = 113.979_400_086_720_4 + 20.0 * (2330.0f64 / 2000.0).log10();
    assert!((s - want).abs() < 1e-9, "{s}");
    let text = ac2(&f, &["cal", "list"]).await?;
    for want in [
        "data sheet 15.0 mV/Pa, loaded 330 Ω → 2.0 kΩ −1.3 dB",
        "verified · electrical (in-line, data sheet 15.0 mV/Pa, loaded 330 Ω → 2.0 kΩ −1.3 dB) \
         ±1 dB",
    ] {
        assert!(text.stdout.contains(want), "{want:?} in\n{}", text.stdout);
    }

    // `cal rm`: the sensitivity calibration of the session's device, the input's mic by
    // default.
    let d = ok_json(&f, &["cal", "rm", "--input", "2", "--json"]).await?;
    assert_eq!(
        d["key"],
        json!({ "device": "fake:loop", "channel": 1, "mic": "MM1 34804" })
    );
    let l = ok_json(&f, &["cal", "list", "--json"]).await?;
    assert_eq!(l["calibrations"], json!([]));
    let text = ac2(&f, &["cal", "rm", "--input", "2", "--mic", "M30"]).await?;
    assert_ne!(text.code, 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn cal_and_unreachable_daemon() -> R {
    let f = fake()?;
    let c = ok_json(
        &f,
        &[
            "cal", "spl", "--input", "3", "--ref", "94db", "--mic", "M30", "--json",
        ],
    )
    .await?;
    assert_eq!(
        c["key"],
        json!({ "device": "fake:loop", "channel": 2, "mic": "M30" })
    );
    assert_eq!(
        c["spl"]["method"],
        json!({ "type": "acoustic", "calibrator_level": 94.0 })
    );
    assert_eq!(c["spl"]["freq"], 1000.0);
    let l = ok_json(&f, &["cal", "list", "--json"]).await?;
    assert_eq!(l["calibrations"].as_array().map(Vec::len), Some(1));
    // Calibrating bound the input's mic name.
    assert_eq!(input_row(&l["inputs"], 3)["mic"], "M30");
    assert_eq!(
        input_row(&l["inputs"], 3)["curve"],
        json!({ "type": "not_chosen" })
    );

    f.lock().mute = true;
    let r = ac2(&f, &["--timeout", "100ms", "devices", "--json"]).await?;
    assert_eq!(r.code, 3);
    assert_eq!(r.json()?["error"]["code"], "not_running");
    Ok(())
}
