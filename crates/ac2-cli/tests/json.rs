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
    let dir = |ch| {
        json!({
            "max_channels": ch,
            "rates_hz": [{ "min": 48000, "max": 48000 }],
            "buffer_frames": { "min": 256, "max": 256 },
            "default_rate_hz": 48000
        })
    };
    assert_eq!(
        v,
        json!([{
            "backend": "fake",
            "host": "fake",
            "id": "fake:loop",
            "name": "Fake loopback",
            "input": dir(4),
            "output": dir(2),
            "duplex_clock": "single_callback",
            "index": "exact",
            "notes": []
        }])
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
                    "input": 2, "weighting": "a", "time_weighting": "fast", "peak_weighting": "c"
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
            "smoothing": { "fraction": "sixth", "mode": "power" },
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
    assert_eq!(d["inserted"]["delay"]["applied_samples"], 600);

    let d = ok_json(&f, &["delay", "set", "main-l", "480samples", "--json"]).await?;
    assert_eq!(d["delay"]["applied"], 0.01);
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
    assert_eq!(t["delay"], 0.01);
    let traces = ok_json(&f, &["trace", "list", "--json"]).await?;
    assert_eq!(traces.as_array().map(Vec::len), Some(1));
    let csv = ac2(&f, &["trace", "export", "l-pre-eq", "--csv", "-"]).await?;
    assert_eq!(csv.code, 0);
    assert_eq!(
        csv.stdout,
        "freq_hz,mag_db,phase_deg,coherence\n1000,-3,45,0.98\n"
    );

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
    assert_eq!(v["session"], json!({ "epoch": 1, "open": null }));
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
    assert_eq!(c["calibrator_level"], 94.0);
    assert_eq!(c["calibrator_freq"], 1000.0);
    let l = ok_json(&f, &["cal", "list", "--json"]).await?;
    assert_eq!(l.as_array().map(Vec::len), Some(1));

    f.lock().mute = true;
    let r = ac2(&f, &["--timeout", "100ms", "devices", "--json"]).await?;
    assert_eq!(r.code, 3);
    assert_eq!(r.json()?["error"]["code"], "not_running");
    Ok(())
}
