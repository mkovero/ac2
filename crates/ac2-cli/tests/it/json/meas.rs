//! Sessions, measurements, delay finding, and naming measurements.

use ac2_audio::FakeConfig;
use ac2_audio::fake::FakePath;
use ac2_proto::model::Signal;
use serde_json::{Value, json};

use super::{R, Rig, ac2, fake, ok_json};

const FS: f64 = 48_000.0;
/// The reference's loopback delay, samples.
const LOOP: u32 = 32;
/// The acoustic path's arrival after the reference, samples.
const ACOUSTIC: u32 = 240;

/// `ok_json` of `args` until `done` holds of the answer: a finding of a measurement whose
/// averages are still filling.
async fn until(f: &Rig, args: &[&str], done: impl Fn(&Value) -> bool) -> R<Value> {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let v = ok_json(f, args).await?;
        if done(&v) || std::time::Instant::now() > end {
            return Ok(v);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

fn delay(v: &Value) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}

/// Two arrivals 200 samples apart, the earlier 12.3 dB under the later: within 2 dB of the
/// first-arrival threshold, so the finder leaves the pick to the operator.
fn two_arrivals() -> R<Rig> {
    let mut fir = vec![0.0; 201];
    fir[0] = 0.5 * 10f32.powf(-12.3 / 20.0);
    fir[200] = 0.5;
    Rig::on(FakeConfig {
        paths: vec![
            FakePath::loopback(0, 0, LOOP),
            // No noise: the finding is the same from one look to the next.
            FakePath::acoustic(0, 1, LOOP + ACOUSTIC, fir, 0.0),
        ],
        ..FakeConfig::default()
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn delay_find_bands_ambiguity_and_refusal() -> R {
    let f = two_arrivals()?;
    ok_json(
        &f,
        &[
            "session",
            "open",
            "--backend",
            "fake",
            "--in",
            "1-2",
            "--json",
        ],
    )
    .await?;
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
            "--start",
            "--json",
        ],
    )
    .await
    .map(|m| assert_eq!(m["config"]["kind"]["config"]["depth"]["type"], "fast_lf"))?;
    let (_c, lease) = f.tone(Signal::Pink, -20.0, vec![0]).await?;

    // Ambiguous once the averages hold both arrivals: the two candidates listed, the second
    // one inserted (1-based on the command line).
    let find = ["delay", "find", "sub", "--observation", "2s", "--json"];
    until(&f, &find, |d| {
        d["finding"]["outcome"]["type"] == "ambiguous"
    })
    .await?;
    let d = ok_json(
        &f,
        &[
            "delay",
            "find",
            "sub",
            "--observation",
            "2s",
            "--insert",
            "2",
            "--json",
        ],
    )
    .await?;
    let outcome = &d["finding"]["outcome"];
    assert_eq!(outcome["type"], "ambiguous", "{d}");
    assert_eq!(d["finding"]["band"]["type"], "full");
    let ranked = outcome["ranked"].as_array().ok_or("ranked")?;
    assert_eq!(ranked.len(), 2, "{d}");
    // The earlier arrival is among the picks.
    let early = f64::from(ACOUSTIC) / FS;
    assert!(
        ranked
            .iter()
            .any(|a| (delay(&a["delay"]) - early).abs() < 1.0 / FS),
        "{d}"
    );
    assert!(
        (delay(&d["inserted"]["delay"]["applied"]) - delay(&ranked[1]["delay"])).abs() < 1e-9,
        "{d}"
    );
    let r = ac2(&f, &["delay", "find", "sub", "--observation", "2s"]).await?;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(
        r.stdout.starts_with("AMBIGUOUS · near the threshold"),
        "{}",
        r.stdout
    );
    assert!(r.stdout.contains("--pick 1|2"), "{}", r.stdout);
    // Bands are sent as given and named in the finding, whatever it finds there.
    let d = ok_json(&f, &["delay", "find", "sub", "--band", "sub", "--json"]).await?;
    assert_eq!(d["finding"]["band"]["type"], "sub", "{d}");
    let r = ac2(&f, &["delay", "find", "sub", "--band", "80hz-800hz"]).await?;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(
        r.stdout.contains("band           custom 80–800 Hz"),
        "{}",
        r.stdout
    );

    // Refused: seconds after the stimulus stopped, the reasons are typed in JSON and spelled
    // out for people; nothing inserted.
    drop(lease);
    f.run_for(5.0).await?;
    let d = ok_json(
        &f,
        &[
            "delay",
            "find",
            "sub",
            "--observation",
            "1s",
            "--insert",
            "--json",
        ],
    )
    .await?;
    assert_eq!(d["finding"]["outcome"]["type"], "no_estimate", "{d}");
    // Silence on both inputs: neither side has anything to correlate.
    assert_eq!(
        d["finding"]["outcome"]["reasons"],
        json!([{ "type": "no_reference" }, { "type": "no_signal" }]),
        "{d}"
    );
    assert_eq!(d["inserted"], Value::Null);
    let r = ac2(&f, &["delay", "find", "sub", "--observation", "1s"]).await?;
    assert_eq!(r.code, 0);
    assert!(r.stdout.starts_with("NO ESTIMATE · "), "{}", r.stdout);
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
    assert_eq!(s["epoch"], 1);
    assert_eq!(s["open"]["input_device"], "fake");
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
                            "duration": d, "weighting": "a", "limit": null, "warn_margin": 3.0,
                            "cue": null
                        }))),
                        "horizon": 60.0,
                        "peaks": { "lcpeak": null, "lafmax": null }
                    },
                    "position": null,
                    "bands": null,
                    "cue": null
                }},
                "scope": "session"
            },
            "config_rev": 3,
            "running": false,
            "delay": null,
            "eq": [],
            "grid_id": null,
            "colour": null,
            "layout": null,
            "offset": 0.0
        })
    );
    // Clearing panes it never had leaves it as it is.
    let m = ok_json(&f, &["meas", "layout", "clear", "foh", "--json"]).await?;
    assert_eq!(m["layout"], serde_json::Value::Null);
    assert_eq!(m["config_rev"], 3);

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

    // Pink noise on out 1: the room mic hears it 240 samples after the loopback.
    let (_c, _lease) = f.tone(Signal::Pink, -20.0, vec![0]).await?;
    let d = until(
        &f,
        &["delay", "find", "main-l", "--observation", "1s", "--json"],
        |d| d["finding"]["outcome"]["type"] == "accepted",
    )
    .await?;
    let arrival = f64::from(ACOUSTIC) / FS;
    let outcome = &d["finding"]["outcome"];
    assert_eq!(outcome["type"], "accepted", "{d}");
    assert!(
        (delay(&outcome["first"]["delay"]) - arrival).abs() < 0.5 / FS,
        "{d}"
    );
    assert_eq!(
        outcome["strongest"]["delay"], outcome["first"]["delay"],
        "{d}"
    );
    assert_eq!(d["finding"]["band"]["type"], "full");
    assert!(
        d["finding"]["confidence"]["psr_db"].as_f64() > Some(10.0),
        "{d}"
    );
    let d = ok_json(&f, &["delay", "insert", "main-l", "--json"]).await?;
    assert!(
        (delay(&d["delay"]["applied"]) - delay(&outcome["first"]["delay"])).abs() < 1e-9,
        "{d}"
    );
    let n = delay(&d["delay"]["applied_samples"]);
    assert!((n - f64::from(ACOUSTIC)).abs() < 0.5, "{d}");

    let d = ok_json(&f, &["delay", "set", "main-l", "480samples", "--json"]).await?;
    assert_eq!(d["delay"]["applied"], 0.01);
    let d = ok_json(&f, &["delay", "nudge", "main-l", "-0.25samples", "--json"]).await?;
    let n = d["delay"]["applied_samples"].as_f64().unwrap_or(f64::NAN);
    assert!((n - 479.75).abs() < 1e-9, "{n}");
    // One delay and its offset from the inserted arrival (5 ms), as the app says it.
    let r = ac2(&f, &["meas", "list"]).await?;
    assert!(
        r.stdout.contains("9.995 ms (+4.995 ms from arrival)"),
        "{}",
        r.stdout
    );
    let r = ac2(&f, &["delay", "nudge", "main-l", "0samples"]).await?;
    assert!(
        r.stdout
            .contains("delay 9.995 ms (+4.995 ms from arrival) · 479.75 samples"),
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
    // One row per point of the trace under its header (an impulse response, when the
    // capture carries one, follows in a table of its own).
    let data = ok_json(&f, &["trace", "show", "l-pre-eq", "--data", "--json"]).await?;
    let points = data["mag_db"].as_array().map_or(0, Vec::len);
    assert_eq!(points, 480, "1/48 octave over the analysis range");
    let rows = csv
        .stdout
        .split_once("\nfreq_hz,mag_db,phase_deg,coherence\n")
        .map_or(0, |(_, t)| {
            t.lines()
                .take_while(|l| l.matches(',').count() == 3)
                .count()
        });
    assert_eq!(rows, points, "{}", csv.stdout);

    let rm = ok_json(&f, &["meas", "rm", "foh", "--json"]).await?;
    assert_eq!(rm["deleted"], 1);
    let missing = ac2(&f, &["meas", "start", "foh", "--json"]).await?;
    assert_eq!(
        missing.json()?,
        json!({ "error": { "code": "usage", "msg": "no measurement named \"foh\"" } })
    );

    let dump = ok_json(&f, &["state", "dump"]).await?;
    let status = ok_json(&f, &["status", "--json"]).await?;
    assert!(
        dump["rev"].as_u64() >= status["rev"].as_u64(),
        "{dump} {status}"
    );
    assert_eq!(
        dump["state"]["measurements"].as_array().map(Vec::len),
        Some(1)
    );

    // How a spectrum is drawn and its display offset: kept with it, not a new config.
    let sp = ok_json(
        &f,
        &[
            "meas", "new", "spectrum", "--input", "1", "--name", "fft", "--json",
        ],
    )
    .await?;
    let m = ok_json(
        &f,
        &[
            "meas",
            "set",
            "fft",
            "--style",
            "fill",
            "--peak",
            "track",
            "--span",
            "60",
            "--colours",
            "magma",
            "--offset",
            "-3",
            "--json",
        ],
    )
    .await?;
    assert_eq!(
        m["config"]["kind"]["config"]["display"],
        json!({"style": "fill", "peak": "track", "spectrograph_span_s": 60,
               "spectrograph_colours": "magma"})
    );
    assert_eq!(m["offset"], -3.0);
    assert_eq!(m["config_rev"], sp["config_rev"]);
    let r = ac2(&f, &["meas", "set", "fft", "--span", "45", "--json"]).await?;
    assert_eq!(r.code, 1);
    let r = ac2(&f, &["meas", "set", "main-l", "--style", "line", "--json"]).await?;
    assert_eq!(r.code, 1);
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
    ok_json(
        &f,
        &[
            "session",
            "open",
            "--backend",
            "fake",
            "--in",
            "1-2",
            "--json",
        ],
    )
    .await?;
    let a = ok_json(&f, &tf("1083")).await?;
    assert_eq!(a["id"], 1);
    let m = ok_json(&f, &["meas", "start", "1083", "--json"]).await?;
    assert_eq!(m["id"], 1);
    let (_c, _lease) = f.tone(Signal::Pink, -20.0, vec![0]).await?;
    let d = until(
        &f,
        &["delay", "find", "1083", "--observation", "1s", "--json"],
        |d| d["finding"]["outcome"]["type"] == "accepted",
    )
    .await?;
    assert_eq!(d["finding"]["outcome"]["type"], "accepted", "{d}");

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
