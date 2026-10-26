//! `ac2 session salvage` of sessions saved by earlier format versions (fixtures under
//! `tests/fixtures/salvage`, in the shapes of formats 20, 22, 23 and 24) into a real
//! daemon: measurements with their settings, colours and delays, the trace under its
//! measurement, the math channel on the restored ids, and the running ones started only
//! where the audio I/O captures their inputs. The fixtures were saved from the fake
//! backend; the format 20 and 22 manifests are the format-23 one less the fields those versions
//! lacked (`layout`; `colour` too in 20, which also has a nudged, tracking delay), and the
//! trace's magnitude columns were filled in by hand because the fake input measured silence,
//! which an import refuses.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use ac2_cli::{Cli, Out, run_reporting};
use ac2_client::{Client, ClientConfig, Endpoints};
use ac2_proto::Command;
use ac2_proto::model::{
    ColourFamily, DeviceSelector, MathExpr, MeasKind, Measurement, Operand, SessionConfig,
    SpecLevel, SpecResolution, TfAveraging, TraceOwner, Weighting, Window,
};
use ac2d::{BackendChoice, Daemon, DaemonConfig, Listen};
use clap::Parser;
use serde_json::Value;

async fn run(ep: &Endpoints, args: &[&str]) -> (i32, String) {
    let argv: Vec<&str> = [
        "ac2",
        "--json",
        "--ctrl-endpoint",
        &ep.ctrl,
        "--data-endpoint",
        &ep.data,
    ]
    .into_iter()
    .chain(args.iter().copied())
    .collect();
    let cli = Cli::try_parse_from(argv).unwrap();
    let (mut so, mut se) = (Vec::new(), Vec::new());
    let code = {
        let mut out = Out::new(true, &mut so);
        run_reporting(&cli, &mut out, &mut se).await
    };
    let text = String::from_utf8(so).unwrap();
    (
        i32::from(code),
        format!("{text}{}", String::from_utf8_lossy(&se)),
    )
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/salvage")
}

/// The fixture session of `version` as a session folder: its manifest beside the trace
/// and SPL log files the three versions share.
fn session_dir(version: u32, into: &Path) -> PathBuf {
    let dir = into.join(format!("autosave.v{version}"));
    for sub in ["traces", "spl"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
        for e in std::fs::read_dir(fixtures().join("files").join(sub)).unwrap() {
            let e = e.unwrap();
            std::fs::copy(e.path(), dir.join(sub).join(e.file_name())).unwrap();
        }
    }
    std::fs::copy(
        fixtures().join(format!("v{version}/session.json")),
        dir.join("session.json"),
    )
    .unwrap();
    dir
}

struct Rig {
    _h: ac2d::Handle,
    ep: Endpoints,
    c: Client,
    dir: tempfile::TempDir,
}

async fn rig(open_inputs: Option<Vec<u16>>) -> Rig {
    let backend = ac2d::backend(BackendChoice::Fake).unwrap();
    let listen = Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    };
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = DaemonConfig::new(backend, listen, -10.0);
    cfg.cal_store = Some(dir.path().join("calibrations.json"));
    let h = Daemon::start(cfg).unwrap();
    let ep = Endpoints {
        ctrl: h.ctrl_endpoint().to_owned(),
        data: h.data_endpoint().to_owned(),
    };
    let c = Client::connect(ClientConfig::new(ep.clone(), "salvage-rig-test"))
        .await
        .unwrap();
    c.wait_synced(Duration::from_secs(5)).await.unwrap();
    if let Some(inputs) = open_inputs {
        c.call(Command::SessionOpen {
            config: SessionConfig {
                backend: None,
                input_device: DeviceSelector::Default,
                output_device: DeviceSelector::Default,
                input_channels: inputs,
                spare_inputs: 0,
                output_channels: 2,
                sample_rate_hz: None,
                buffer_frames: None,
                loopback: None,
            },
        })
        .await
        .unwrap();
    }
    Rig { _h: h, ep, c, dir }
}

fn document(text: &str) -> Value {
    let mut de = serde_json::Deserializer::from_str(text).into_iter::<Value>();
    de.next().expect("a document").expect("json")
}

async fn salvage(r: &Rig, version: u32) -> Value {
    let dir = session_dir(version, r.dir.path());
    let (code, text) = run(&r.ep, &["session", "salvage", dir.to_str().unwrap()]).await;
    assert_eq!(code, 0, "{text}");
    let report = document(&text);
    assert_eq!(report["version"], version, "{report:#}");
    report
}

fn item<'r>(report: &'r Value, name: &str) -> &'r Value {
    report["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == name)
        .unwrap_or_else(|| panic!("no item {name}: {report:#}"))
}

fn notes(item: &Value) -> String {
    item["notes"].to_string()
}

async fn meas(c: &Client, name: &str) -> Measurement {
    let s = c.snapshot().await.unwrap().state;
    s.measurements
        .into_iter()
        .find(|m| m.config.name == name)
        .unwrap_or_else(|| panic!("no measurement {name}"))
}

/// What every version's fixture holds, restored the same.
async fn check_common(r: &Rig, report: &Value) {
    for name in ["FOH SPL", "TF 1", "Spectrum 1", "TF 1 vs S1", "TF 1 S1"] {
        assert_eq!(item(report, name)["restored"], true, "{report:#}");
    }

    let spl = meas(&r.c, "FOH SPL").await;
    let MeasKind::Spl { config } = &spl.config.kind else {
        panic!("{spl:?}")
    };
    assert_eq!(config.input, 0);
    assert_eq!(config.weighting, Weighting::C);
    let windows: Vec<_> = config
        .leq
        .windows
        .iter()
        .map(|w| (w.duration.0, w.limit.map(|l| l.0)))
        .collect();
    assert_eq!(windows, [(60.0, None), (3600.0, Some(80.0))]);
    assert_eq!(config.leq.horizon.0, 600.0);
    let peaks = &config.leq.peaks;
    assert_eq!(peaks.lcpeak.as_ref().map(|p| p.limit.0), Some(140.0));
    assert_eq!(peaks.lafmax.as_ref().map(|p| p.limit.0), Some(115.0));
    let bands = config.bands.as_ref().expect("band meter");
    assert_eq!(bands.windows.len(), 11);
    assert_eq!(
        bands.predicted.as_ref().map(|p| p.night.map(|n| n.0)),
        Some(Some(25.0))
    );
    assert!(
        !notes(item(report, "FOH SPL")).contains("does not fit"),
        "{report:#}"
    );

    let tf = meas(&r.c, "TF 1").await;
    let MeasKind::Transfer { config } = &tf.config.kind else {
        panic!("{tf:?}")
    };
    assert_eq!((config.reference_input, config.measurement_input), (1, 0));
    assert_eq!(config.averaging, TfAveraging::Fifo { blocks: 16 });
    assert!(config.smoothing.is_some());
    assert!(!tf.running, "TF 1 was saved stopped");

    // Format 23's single FFT length is this version's fixed resolution.
    let spec = meas(&r.c, "Spectrum 1").await;
    let MeasKind::Spectrum { config } = &spec.config.kind else {
        panic!("{spec:?}")
    };
    assert_eq!(config.resolution, SpecResolution::Fixed { fft_len: 65_536 });
    assert_eq!(config.window, Window::Hann);
    assert!(config.smoothing.is_some());
    assert!(
        !notes(item(report, "Spectrum 1")).contains("fft_len"),
        "{report:#}"
    );

    // The math channel computes from the restored TF 1 and its trace.
    let s = r.c.snapshot().await.unwrap().state;
    let trace = s
        .traces
        .iter()
        .find(|t| t.edit.name == "TF 1 S1")
        .expect("trace");
    assert_eq!(trace.edit.owner, TraceOwner::Meas { meas: tf.id });
    assert_eq!(trace.edit.slot, Some(2));
    assert!(trace.edit.smoothing.is_some());
    let math = meas(&r.c, "TF 1 vs S1").await;
    let MeasKind::Math { config } = &math.config.kind else {
        panic!("{math:?}")
    };
    assert_eq!(config.owner, TraceOwner::Meas { meas: tf.id });
    let MathExpr::Binary { a, b, .. } = config.expr else {
        panic!("{config:?}")
    };
    assert_eq!(a, Operand::Meas { meas: tf.id });
    assert_eq!(b, Operand::Trace { trace: trace.id });

    // The Leq history is not taken back, and the summary says so.
    let log = report["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["what"] == "spl_log")
        .expect("spl log row");
    assert_eq!(log["restored"], false);
    assert_eq!(log["name"], "FOH SPL");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn format_22_comes_back_with_colours_and_running_state() {
    let r = rig(Some(vec![0, 1])).await;
    let report = salvage(&r, 22).await;
    check_common(&r, &report).await;
    let tf = meas(&r.c, "TF 1").await;
    assert_eq!(tf.colour, Some(ColourFamily::Hue { index: 2 }));
    let d = tf.delay.expect("delay");
    assert!((d.applied.0 - 0.0025).abs() < 1e-9, "{d:?}");
    // Running when saved and its input captured: started.
    assert!(meas(&r.c, "FOH SPL").await.running);
    assert!(meas(&r.c, "Spectrum 1").await.running);
}

/// Format 24 had no level mode: a spectrum read tone power at every resolution, so both
/// the fixed and the multi-resolution one come back as tone (what was on screen, and
/// what their stored traces hold) and the report names the setting it filled in.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn format_24_spectra_come_back_as_tone_and_say_so() {
    let r = rig(Some(vec![0, 1])).await;
    let report = salvage(&r, 24).await;
    check_common(&r, &report).await;
    for (name, resolution) in [
        ("Spectrum 1", SpecResolution::Fixed { fft_len: 65_536 }),
        ("Spectrum MR", SpecResolution::MULTI),
    ] {
        let m = meas(&r.c, name).await;
        let MeasKind::Spectrum { config } = &m.config.kind else {
            panic!("{m:?}")
        };
        assert_eq!(config.resolution, resolution);
        assert_eq!(config.level, SpecLevel::Tone, "{name}");
        let n = notes(item(&report, name));
        assert!(
            n.contains("level") && n.contains("tone"),
            "{name}: {report:#}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn format_20_keeps_the_delay_and_tracking_and_names_the_arrival_it_cannot() {
    let r = rig(Some(vec![0, 1])).await;
    let report = salvage(&r, 20).await;
    check_common(&r, &report).await;
    let tf = meas(&r.c, "TF 1").await;
    assert_eq!(tf.colour, None, "format 20 had no colour choice");
    let d = tf.delay.expect("delay");
    assert!((d.applied.0 - 0.0031).abs() < 1e-9, "{d:?}");
    assert!(d.tracking);
    assert!(
        notes(item(&report, "TF 1")).contains("is not kept"),
        "the arrival offset is named: {report:#}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn format_23_without_audio_reports_what_it_could_not_set() {
    let r = rig(None).await;
    let report = salvage(&r, 23).await;
    check_common(&r, &report).await;
    for name in ["FOH SPL", "Spectrum 1"] {
        assert!(
            notes(item(&report, name)).contains("no audio I/O is open"),
            "{report:#}"
        );
        assert!(!meas(&r.c, name).await.running);
    }
    assert!(
        notes(item(&report, "TF 1")).contains("not applied"),
        "{report:#}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_running_measurement_on_an_input_not_captured_is_left_stopped() {
    // Only input 2 is captured: the SPL meter and spectrum on input 1 stay stopped.
    let r = rig(Some(vec![1])).await;
    let report = salvage(&r, 22).await;
    check_common(&r, &report).await;
    assert!(
        notes(item(&report, "FOH SPL")).contains("input 1 is not captured"),
        "{report:#}"
    );
    assert!(!meas(&r.c, "FOH SPL").await.running);
    assert!(!meas(&r.c, "Spectrum 1").await.running);
}
