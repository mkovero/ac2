//! `--json` output of commands run in-process against the fake daemon.

use ac2_cli::{Cli, Out, run_reporting};
use ac2_client::fake::{FakeDaemon, FakeOptions};
use clap::Parser;
use serde_json::{Value, json};

type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct Run {
    code: u8,
    stdout: String,
    stderr: String,
}

impl Run {
    fn json(&self) -> R<Value> {
        serde_json::from_str(&self.stdout)
            .map_err(|e| format!("{e}: stdout {:?} stderr {:?}", self.stdout, self.stderr).into())
    }
}

async fn ac2(f: &FakeDaemon, args: &[&str]) -> R<Run> {
    let ep = f.endpoints();
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
    let cli = Cli::try_parse_from(argv)?;
    let (mut so, mut se) = (Vec::new(), Vec::new());
    let code = {
        let mut out = Out::new(cli.json, &mut so);
        run_reporting(&cli, &mut out, &mut se).await
    };
    Ok(Run {
        code,
        stdout: String::from_utf8(so)?,
        stderr: String::from_utf8(se)?,
    })
}

async fn ok_json(f: &FakeDaemon, args: &[&str]) -> R<Value> {
    let r = ac2(f, args).await?;
    if r.code != 0 {
        return Err(format!("{args:?} exited {}: {} {}", r.code, r.stdout, r.stderr).into());
    }
    r.json()
}

fn fake() -> R<FakeDaemon> {
    Ok(FakeDaemon::start(FakeOptions::default())?)
}

#[tokio::test(flavor = "multi_thread")]
async fn devices_json() -> R {
    let f = fake()?;
    let v = ok_json(&f, &["devices", "--json"]).await?;
    let dir = |ch, names: serde_json::Value| {
        json!({
            "max_channels": ch,
            "rates_hz": [{ "min": 48000, "max": 48000 }],
            "buffer_frames": { "min": 256, "max": 256 },
            "default_rate_hz": 48000,
            "default_buffer_frames": 256,
            "channel_names": names
        })
    };
    assert_eq!(
        v,
        json!([
            {
                "kind": "fake",
                "description": "Simulated rig (no audio): out 1 returns on in 1 (loop) and in 2 (room)",
                "availability": { "type": "available" },
                "devices": [{
                    "backend": "fake",
                    "host": "fake",
                    "id": "fake:loop",
                    "name": "Fake loopback",
                    "input": dir(4, json!(["Loop return", "Room mic", "Line 3", "Line 4"])),
                    "output": dir(2, json!(["Out 1 (speaker + loop)", "Out 2"])),
                    "duplex_clock": "single_callback",
                    "index": "exact",
                    "notes": []
                }]
            },
            {
                "kind": "jack",
                "description": "JACK audio server",
                "availability": { "type": "unavailable", "reason": "JACK server not running" },
                "devices": []
            }
        ])
    );
    Ok(())
}

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
            "frozen": false,
            "delay": null,
            "grid_id": null
        })
    );

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
            "grid": { "ppo": 48, "k_min": -240, "k_max": 239 },
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
            .starts_with("# ac2 trace export v3\n# name: l-pre-eq\n# kind: transfer\n"),
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

#[tokio::test(flavor = "multi_thread")]
async fn status_reports_build_ids() -> R {
    let f = fake()?;
    let v = ok_json(&f, &["status", "--json"]).await?;
    assert_eq!(v["server"], "ac2d 0.0.0 (build fake)");
    assert_eq!(v["build_id"], "fake");
    assert_eq!(v["client_build_id"], ac2_cli::BUILD_ID);
    assert_eq!(v["build_matches"], false);
    // Not a local daemon: "stale" is not judged.
    assert_eq!(v["stale"], Value::Null);
    assert_eq!(v["daemon_incarnation"], "000000005eed0001");
    assert_eq!(
        v["session"],
        json!({ "epoch": 1, "open": null, "stopped": null })
    );
    assert_eq!(
        v["autosave"],
        json!({ "state": { "type": "off" }, "saved_at": null })
    );
    let s = ok_json(&f, &["session", "status", "--json"]).await?;
    assert_eq!(
        s,
        json!({
            "epoch": 1,
            "open": null,
            "recording": null,
            "stopped": null,
            "autosave": { "state": { "type": "off" }, "saved_at": null },
        })
    );
    assert!(
        v["client_id"]
            .as_str()
            .is_some_and(|s| s.starts_with("local-"))
    );

    let same = FakeDaemon::start(FakeOptions {
        server: format!("ac2d 0.0.0 (build {})", ac2_cli::BUILD_ID),
        ..FakeOptions::default()
    })?;
    let v = ok_json(&same, &["daemon", "status", "--json"]).await?;
    assert_eq!(v["build_matches"], true);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn gen_refusals_never_touch_the_lease() -> R {
    let f = fake()?;
    // The fake daemon's ceiling is −6 dBFS.
    let r = ac2(
        &f,
        &["gen", "pink", "--out", "1", "--level", "-3dbfs", "--json"],
    )
    .await?;
    assert_eq!(r.code, 4);
    assert_eq!(r.json()?["error"]["code"], "refused");
    // No session open.
    let r = ac2(
        &f,
        &["gen", "pink", "--out", "1", "--level", "-30dbfs", "--json"],
    )
    .await?;
    assert_eq!(r.code, 1);
    assert_eq!(r.json()?["error"]["code"], "usage");
    // Band limits on a sine.
    let r = ac2(
        &f,
        &[
            "gen", "sine", "--freq", "1khz", "--hp", "30hz", "--out", "1", "--level", "-30dbfs",
        ],
    )
    .await?;
    assert_eq!(r.code, 1);
    assert_eq!(f.executions("gen.acquire"), 0);

    let v = ok_json(&f, &["gen", "stop", "--json"]).await?;
    assert_eq!(v["stopped"], true);
    assert_eq!(f.executions("gen.stop"), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn gen_ceiling_shows_lowers_and_raises_only_with_yes() -> R {
    let f = fake()?;
    // The fake daemon's ceiling and bound are −6 dBFS.
    let v = ok_json(&f, &["gen", "ceiling", "--json"]).await?;
    assert_eq!(
        (v["ceiling"].as_f64(), v["bound"].as_f64()),
        (Some(-6.0), Some(-6.0))
    );
    let v = ok_json(&f, &["gen", "ceiling", "-40dbfs", "--json"]).await?;
    assert_eq!(
        (v["ceiling"].as_f64(), v["previous"].as_f64()),
        (Some(-40.0), Some(-6.0))
    );
    let r = ac2(&f, &["gen", "ceiling", "-30dbfs"]).await?;
    assert_eq!(
        r.code, 4,
        "a raise without --yes is refused before anything is sent"
    );
    assert_eq!(f.executions("gen.ceiling"), 1);
    let v = ok_json(&f, &["gen", "ceiling", "-30dbfs", "--yes", "--json"]).await?;
    assert_eq!(v["ceiling"].as_f64(), Some(-30.0));
    // Above the bound: the daemon refuses it whatever the confirmation.
    let r = ac2(&f, &["gen", "ceiling", "-3dbfs", "--yes", "--json"]).await?;
    assert_ne!(r.code, 0);
    let r = ac2(&f, &["gen", "ceiling"]).await?;
    assert_eq!(r.code, 0);
    assert!(
        r.stdout
            .contains("system max level \u{2212}30.0 dBFS · bound \u{2212}6.0 dBFS")
            && r.stdout.contains("(raised by "),
        "{}",
        r.stdout
    );
    Ok(())
}

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
            "notes": []
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
    let saved = ok_json(&f, &["session", "save", "show", "--json"]).await?;
    assert_eq!(saved["name"], "show");
    assert_eq!(saved["measurements"], 1);
    assert_eq!(saved["traces"], 6);
    let list = ok_json(&f, &["session", "list", "--json"]).await?;
    assert_eq!(list.as_array().map(Vec::len), Some(1));
    assert_eq!(list[0]["name"], "show");
    ok_json(&f, &["trace", "rm", "house", "--json"]).await?;
    let loaded = ok_json(&f, &["session", "load", "show", "--json"]).await?;
    assert_eq!(loaded["traces"], 6);
    let dump = ok_json(&f, &["state", "dump"]).await?;
    assert_eq!(dump["state"]["traces"].as_array().map(Vec::len), Some(6));
    assert_eq!(dump["state"]["generator"]["owner"], json!(null));
    assert_eq!(dump["state"]["generator"]["armed"], false);
    let by_path = dir.path().join("elsewhere");
    let p = ok_json(
        &f,
        &["session", "save", &by_path.to_string_lossy(), "--json"],
    )
    .await?;
    assert_eq!(p["name"], "elsewhere");
    assert!(by_path.join("session.json").exists());
    let r = ac2(&f, &["session", "load", "nope", "--json"]).await?;
    assert_eq!(r.json()?["error"]["code"], "not_found");
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
