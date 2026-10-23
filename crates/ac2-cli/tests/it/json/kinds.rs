//! Input kinds, stereo pairs and line calibration (`programme-meter.md` §1–2): what
//! `session inputs --kind / --pair`, `cal line`, `cal list` and `cal rm --line` send, print
//! and refuse, in the app's words.

use serde_json::{Value, json};

use super::{R, ac2, fake, ok_json};

/// The row of 1-based input `n` in an inputs JSON table.
fn input_row(v: &Value, n: u64) -> Value {
    v.as_array()
        .and_then(|a| a.iter().find(|r| r["input"] == n))
        .cloned()
        .unwrap_or(Value::Null)
}

fn has(text: &str, want: &str) {
    assert!(text.contains(want), "{want:?} in\n{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn kinds_pairs_and_line_calibration_flow() -> R {
    let f = fake()?;
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

    // Kinds and a named pair in one command; the kinds come back typed.
    let i = ok_json(
        &f,
        &[
            "session",
            "inputs",
            "--kind",
            "3=line",
            "--kind",
            "4=line",
            "--kind",
            "2=digital",
            "--mic",
            "1=MM1",
            "--pair",
            "3,4=Main",
            "--json",
        ],
    )
    .await?;
    assert_eq!(input_row(&i, 1)["kind"]["kind"], "mic");
    assert_eq!(input_row(&i, 1)["mic"], "MM1");
    assert_eq!(input_row(&i, 2)["kind"], json!({ "kind": "digital" }));
    assert_eq!(input_row(&i, 3)["kind"], json!({ "kind": "line" }));
    assert_eq!(input_row(&i, 3)["pair"], "left");
    assert_eq!(input_row(&i, 4)["pair"], "right");
    // The text: one line per input, the pair on one line, the app's words.
    let t = ac2(&f, &["session", "inputs"]).await?.stdout;
    has(&t, "in 1 · mic MM1 · ");
    has(&t, "in 2 · digital · dBFS");
    has(&t, "in 3+4 · line · pair Main · dBFS");
    assert!(!t.contains("uncalibrated · dBFS"), "{t}");

    // A mic name or curve on a line or digital input is refused here, naming the way.
    for args in [
        ["session", "inputs", "--mic", "3=M30", "--json"],
        ["session", "inputs", "--curve", "2=off", "--json"],
    ] {
        let r = ac2(&f, &args).await?;
        assert_ne!(r.code, 0, "{args:?}");
        has(&r.stdout, "belongs to a mic");
        has(&r.stdout, "--kind");
    }
    // Pair refusals come from the daemon, as it states them.
    let r = ac2(&f, &["session", "inputs", "--pair", "1,2", "--json"]).await?;
    assert_ne!(r.code, 0);
    assert_eq!(r.json()?["error"]["code"], "invalid");
    has(&r.stdout, "input 1 is a microphone");
    let r = ac2(
        &f,
        &[
            "session", "inputs", "--kind", "5=line", "--kind", "6=line", "--pair", "5,6", "--json",
        ],
    )
    .await?;
    assert_ne!(r.code, 0);
    has(&r.stdout, "input 5 is not captured by the session");
    // A refused command changes nothing: the pair stands.
    let i = ok_json(&f, &["session", "inputs", "--json"]).await?;
    assert_eq!(input_row(&i, 3)["pair"], "left");

    // Line calibration: +4 dBu (1.228 V) read at −9 dBFS puts full scale at +13 dBu.
    let r = ac2(
        &f,
        &[
            "cal", "line", "--input", "3", "--volts", "1.228v", "--read", "-9dbfs",
        ],
    )
    .await?;
    assert_eq!(r.code, 0, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        r.stdout.trim(),
        "in 3: 0 dBFS = +13.0 dBu (1.228 V read at −9.0 dBFS)"
    );
    let j = ok_json(
        &f,
        &[
            "cal", "line", "--input", "3", "--volts", "1.228v", "--read", "-9dbfs", "--json",
        ],
    )
    .await?;
    assert_eq!(j["key"]["channel"], 2);
    assert!(
        (j["dbu_fs"].as_f64().unwrap_or(f64::NAN) - 13.0).abs() < 0.05,
        "{j}"
    );
    assert!((j["line"]["measured"].as_f64().unwrap_or(f64::NAN) + 9.0).abs() < 1e-9);
    // On a digital input: the daemon's refusal, verbatim.
    let r = ac2(
        &f,
        &[
            "cal", "line", "--input", "2", "--volts", "1.228v", "--read", "-9dbfs", "--json",
        ],
    )
    .await?;
    assert_ne!(r.code, 0);
    assert_eq!(r.json()?["error"]["code"], "invalid");
    has(&r.stdout, "input 2 is a digital input");
    has(&r.stdout, "a line calibration is for a line input");

    // `cal list`: its own block; the input line now reads dBu on the calibrated side.
    let t = ac2(&f, &["cal", "list"]).await?.stdout;
    has(&t, "line calibrations\nin 3 · +13.0 dBu at full scale · ");
    has(&t, " · fake:loop");
    has(
        &t,
        "in 3+4 · line · pair Main · L dBu · full scale +13.0 dBu · ",
    );
    has(&t, " · R dBFS");
    let l = ok_json(&f, &["cal", "list", "--json"]).await?;
    assert_eq!(l["line_calibrations"].as_array().map(Vec::len), Some(1));
    assert_eq!(input_row(&l["inputs"], 3)["cal"]["type"], "line");
    // `ac2 status` shows the same lines.
    let st = ac2(&f, &["status"]).await?.stdout;
    has(&st, "in 3+4 · line · pair Main");
    has(&st, "in 2 · digital · dBFS");

    // `cal rm --line` removes it; the mic form still needs a mic.
    let r = ac2(&f, &["cal", "rm", "--input", "3", "--line"]).await?;
    assert_eq!(r.code, 0, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        r.stdout.trim(),
        "line calibration of input 3 of fake:loop deleted"
    );
    let l = ok_json(&f, &["cal", "list", "--json"]).await?;
    assert_eq!(l["line_calibrations"].as_array().map(Vec::len), Some(0));
    let t = ac2(&f, &["cal", "list"]).await?.stdout;
    has(&t, "line calibrations\nnone");
    // A second removal: the device is the session's, so the daemon says what is not there.
    let r = ac2(&f, &["cal", "rm", "--input", "3", "--line", "--json"]).await?;
    assert_ne!(r.code, 0);
    assert_eq!(r.json()?["error"]["code"], "not_found");
    let r = ac2(&f, &["cal", "rm", "--input", "3", "--json"]).await?;
    assert_ne!(r.code, 0);
    has(&r.stdout, "input 3 is a line input");

    // `--no-pairs` clears; a kind change back to mic drops the line input's pair side too.
    let i = ok_json(&f, &["session", "inputs", "--no-pairs", "--json"]).await?;
    assert_eq!(input_row(&i, 3)["pair"], Value::Null);
    assert_eq!(input_row(&i, 4)["pair"], Value::Null);
    let i = ok_json(
        &f,
        &[
            "session", "inputs", "--kind", "3=mic", "--mic", "3=M30", "--json",
        ],
    )
    .await?;
    assert_eq!(input_row(&i, 3)["kind"]["kind"], "mic");
    assert_eq!(input_row(&i, 3)["mic"], "M30");
    let t = ac2(&f, &["session", "inputs"]).await?.stdout;
    has(&t, "in 3 · mic M30 · ");
    has(&t, "in 4 · line · dBFS");
    Ok(())
}
