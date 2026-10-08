//! Calibration end to end on the fake rig (`docs/design/q7-calibration.md`): cal.spl against
//! a generated "calibrator" tone, the input setup's mic name deciding verified vs other mic,
//! a mic-curve import correcting SPL, spectrum, RTA and TF magnitude (and nothing at the
//! calibrator frequency), switching it off, two curves of one mic (0° / 90°) switched on an
//! input, persistence across a restart, and an unreadable store that is refused and never
//! overwritten.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::{Duration, Instant};

use ac2_client::{Client, ClientConfig, ClientError, Endpoints, OnDrop, StimulusLease};
use ac2_proto::frame::{FrameData, SpecFrame, SplMeta, TfFrame};
use ac2_proto::model::{
    BandFraction, CalBasis, CalKey, CalMethod, CalState, CalStatus, CurveChoice,
    ElectricalConnection, GeneratorDesired, GeneratorSettings, InputSetup, LevelScale, MeasConfig,
    MeasKind, Mic, MicCurveId, RtaConfig, SensitivitySource, Signal, SpecAveraging, SpectrumConfig,
    State, TraceMeta, Weighting, Window,
};
use ac2_proto::units::{Blob, Db, DbSpl, Dbfs, Hz, MeasId, MvPerPa, Seconds, Volts};
use ac2_proto::{Command, ErrorCode, ErrorDetail, ReplyBody, Stream, Subscription, Topic};
use ac2d::{Daemon, Handle};
use common::*;

const CURVE: &str = "* test mic\n20 0\n1000 0\n4000 0\n8000 6\n24000 6\n";

async fn connect(h: &Handle) -> Client {
    let ep = Endpoints {
        ctrl: h.ctrl_endpoint().to_owned(),
        data: h.data_endpoint().to_owned(),
    };
    let c = Client::connect(ClientConfig::new(ep, "ac2d-cal-test"))
        .await
        .unwrap();
    c.wait_synced(Duration::from_secs(5)).await.unwrap();
    c
}

fn start(store: &std::path::Path) -> Handle {
    let mut cfg = config(realtime_rig(), local_tcp());
    cfg.cal_store = Some(store.to_owned());
    Daemon::start(cfg).unwrap()
}

fn sine(freq: f64, signal: Signal) -> GeneratorDesired {
    let _ = freq;
    GeneratorDesired {
        settings: GeneratorSettings {
            signal,
            level: Dbfs(-20.0),
            band: None,
            outputs: vec![0],
        },
        armed: true,
        firing: true,
    }
}

/// Polls the newest frame of `stream` of `meas` until `pick` returns a value.
async fn wait<T>(
    c: &Client,
    meas: u32,
    stream: Stream,
    what: &str,
    mut pick: impl FnMut(&FrameData) -> Option<T>,
) -> T {
    let topic = Topic::Data {
        meas: MeasId(meas),
        stream,
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(f) = c.latest().unwrap().get(&topic)
            && !f.stale
            && let Some(v) = pick(&f.frame.data)
        {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

/// The mirrored state once `ok` holds.
async fn state_until(c: &Client, ok: impl Fn(&State) -> bool) -> std::sync::Arc<State> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(s) = c.view().state.clone()
            && ok(&s)
        {
            return s;
        }
        assert!(Instant::now() < deadline, "mirror never caught up");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn spl_meta(d: &FrameData) -> Option<SplMeta> {
    match d {
        FrameData::Spl(f) => Some(f.meta),
        _ => None,
    }
}

/// Settled SPL reading satisfying `ok`.
async fn spl_until(c: &Client, what: &str, ok: impl Fn(&SplMeta) -> bool) -> SplMeta {
    wait(c, 1, Stream::Spl, what, |d| spl_meta(d).filter(|m| ok(m))).await
}

fn tf(d: &FrameData) -> Option<TfFrame> {
    match d {
        FrameData::Tf(f) => Some(f.clone()),
        _ => None,
    }
}

fn spec(d: &FrameData) -> Option<SpecFrame> {
    match d {
        FrameData::Spec(f) => Some(f.clone()),
        _ => None,
    }
}

fn spec_peak(f: &SpecFrame) -> (usize, f32) {
    f.level
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, v)| v.is_finite())
        .fold((0, f32::NEG_INFINITY), |a, b| if b.1 > a.1 { b } else { a })
}

async fn capture(c: &Client, meas: u32, name: &str) -> TraceMeta {
    match c
        .call(Command::TraceCapture {
            meas: MeasId(meas),
            name: name.into(),
            slot: None,
        })
        .await
        .unwrap()
    {
        ReplyBody::Trace(t) => t,
        other => panic!("{other:?}"),
    }
}

async fn set_curve(c: &Client, mic: &str, curve: CurveChoice) {
    c.call(Command::SessionInputs {
        inputs: vec![InputSetup {
            channel: 1,
            mic: Some(mic.into()),
            curve,
        }],
    })
    .await
    .unwrap();
}

fn label(l: &str) -> CurveChoice {
    CurveChoice::Curve { label: l.into() }
}

async fn import(c: &Client, mic: &str, file_name: &str, content: &[u8]) -> Mic {
    match c
        .call(Command::CalCurveImport {
            mic: mic.into(),
            label: None,
            file_name: file_name.into(),
            content: Blob(content.to_vec()),
            input: Some(1),
        })
        .await
        .unwrap()
    {
        ReplyBody::Mic(m) => m,
        other => panic!("{other:?}"),
    }
}

fn fixture(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/mic_curves")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

async fn fire(lease: &StimulusLease, signal: Signal) {
    lease.set(sine(0.0, signal)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn calibration_mic_curve_and_matching_end_to_end() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("calibrations.json");
    let h = start(&store);
    let c = connect(&h).await;

    // No session: a calibration needs the capture device it is tied to.
    let e = c
        .call(Command::CalSpl {
            input: 1,
            mic: "M30".into(),
            calibrator_level: DbSpl(94.0),
            calibrator_freq: Hz(1000.0),
        })
        .await
        .unwrap_err();
    assert!(matches!(e, ClientError::Daemon(p) if p.code == ErrorCode::Invalid));

    c.call(Command::SessionOpen {
        config: session(false),
    })
    .await
    .unwrap();
    for cfg in [
        spl("meter", 1),
        transfer("tf"),
        MeasConfig {
            name: "spec".into(),
            kind: MeasKind::Spectrum {
                config: SpectrumConfig {
                    input: 1,
                    fft_len: 8192,
                    window: Window::Hann,
                    averaging: SpecAveraging::Off,
                    smoothing: None,
                },
            },
        },
        MeasConfig {
            name: "rta".into(),
            kind: MeasKind::Rta {
                config: RtaConfig {
                    input: 1,
                    fraction: BandFraction::Third,
                    f_lo: Hz(20.0),
                    f_hi: Hz(20_000.0),
                    weighting: Weighting::Z,
                    averaging: SpecAveraging::Exponential {
                        time_constant: Seconds(1.0),
                    },
                },
            },
        },
    ] {
        let m = match c.call(Command::MeasCreate { config: cfg }).await.unwrap() {
            ReplyBody::Measurement(m) => m,
            other => panic!("{other:?}"),
        };
        c.call(Command::MeasStart { meas: m.id }).await.unwrap();
        c.subscribe(Subscription::Meas(m.id)).unwrap();
    }
    // Measurement ids: 1 spl, 2 tf, 3 spec, 4 rta.

    // The "calibrator": a 1 kHz tone through the acoustic path (−20 dBFS × 0.5 = −26 dBFS).
    let lease = c.acquire_lease(false, OnDrop::Release).await.unwrap();
    fire(&lease, Signal::Sine { freq: Hz(1000.0) }).await;
    let m = spl_until(&c, "uncalibrated tone", |m| (m.level + 26.02).abs() < 0.2).await;
    assert_eq!(m.scale, LevelScale::Dbfs);
    assert_eq!(m.cal, CalStatus::Uncalibrated);

    // Right after the tone starts the 1 s reading has not settled: refused, then accepted
    // once it is steady.
    let mut refused = 0;
    let deadline = Instant::now() + Duration::from_secs(15);
    let entry = loop {
        match c
            .call(Command::CalSpl {
                input: 1,
                mic: "M30".into(),
                calibrator_level: DbSpl(94.0),
                calibrator_freq: Hz(1000.0),
            })
            .await
        {
            Ok(ReplyBody::Calibration(e)) => break e,
            Err(ClientError::Daemon(p)) if p.msg.contains("not steady") => refused += 1,
            other => panic!("{other:?}"),
        }
        assert!(Instant::now() < deadline, "calibrator never steady");
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    assert!(refused > 0, "an unsettled reading must be refused");
    let cal = entry.spl;
    assert!((cal.measured.0 + 26.02).abs() < 0.06, "{:?}", cal.measured);
    assert!((cal.sensitivity.0 - 120.02).abs() < 0.1);
    let st = state_until(&c, |s| !s.inputs.is_empty()).await;
    assert_eq!(
        st.inputs,
        vec![InputSetup {
            channel: 1,
            mic: Some("M30".into()),
            curve: CurveChoice::NotChosen,
        }],
        "calibrating binds the input's mic name"
    );
    let m = spl_until(&c, "verified", |m| {
        matches!(m.cal, CalStatus::Verified { .. }) && (m.level - 94.0).abs() < 0.2
    })
    .await;
    assert_eq!(m.scale, LevelScale::DbSpl);
    assert!(!m.mic_curve);

    // A refused curve file: typed error, nothing stored.
    let e = c
        .call(Command::CalCurveImport {
            mic: "M30".into(),
            label: None,
            file_name: "bad.frd".into(),
            content: Blob(b"20 0\n10 1\n".to_vec()),
            input: Some(1),
        })
        .await
        .unwrap_err();
    let ClientError::Daemon(p) = e else {
        panic!("{e:?}")
    };
    assert_eq!(p.code, ErrorCode::Invalid);
    assert!(matches!(
        p.detail,
        Some(ErrorDetail::MicCurveFile { line: Some(2), .. })
    ));

    // Import the curve: 0 dB at the 1 kHz calibrator, so the tone reads the same. The mic's
    // only curve becomes the input's active one; its label is the file stem (no angle).
    let m = import(&c, "M30", "/home/op/curves/M30.frd", CURVE.as_bytes()).await;
    let r = m.curves[0].clone();
    assert_eq!(
        (r.file_name.as_str(), r.label.as_str(), r.points),
        ("M30.frd", "M30", 5)
    );
    let st = state_until(&c, |s| {
        s.inputs.first().is_some_and(|i| i.curve == label("M30"))
    })
    .await;
    assert_eq!(st.calibrations.len(), 1, "the calibration is kept");
    let m = spl_until(&c, "corrected at 1 kHz", |m| m.mic_curve).await;
    assert!((m.level - 94.0).abs() < 0.2, "{}", m.level);

    // 10 kHz: the curve's +6 dB comes off SPL and spectrum; switching it off restores them.
    fire(&lease, Signal::Sine { freq: Hz(10_000.0) }).await;
    let on = spl_until(&c, "10 kHz corrected", |m| {
        m.mic_curve && (m.level - 88.0).abs() < 0.3
    })
    .await;
    assert!(
        (on.lpeak - (94.0 + 3.01)).abs() < 0.3,
        "Lpeak stays uncorrected: {}",
        on.lpeak
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    let s_on = wait(&c, 3, Stream::Spec, "spec on", |d| {
        spec(d).filter(|f| f.meta.mic_curve)
    })
    .await;
    set_curve(&c, "M30", CurveChoice::Off).await;
    spl_until(&c, "10 kHz uncorrected", |m| {
        !m.mic_curve && (m.level - 94.0).abs() < 0.3
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let s_off = wait(&c, 3, Stream::Spec, "spec off", |d| {
        spec(d).filter(|f| !f.meta.mic_curve)
    })
    .await;
    let ((k_on, p_on), (k_off, p_off)) = (spec_peak(&s_on), spec_peak(&s_off));
    assert_eq!(k_on, k_off);
    assert!(
        (p_off - p_on - 6.0).abs() < 0.3,
        "spectrum {p_off} vs {p_on}"
    );
    assert!(matches!(s_on.meta.cal, CalStatus::Verified { .. }));

    // Pink noise: TF magnitude at ~10 kHz drops by the curve's 6 dB; phase is untouched.
    c.call(Command::DelaySet {
        meas: MeasId(2),
        delay: Seconds(f64::from(ACOUSTIC_DELAY) / f64::from(FS)),
    })
    .await
    .unwrap();
    fire(&lease, Signal::Pink).await;
    let col = 399; // k = 159: 1000·2^(159/48) ≈ 9.9 kHz
    // The RTA averages exponentially (1 s): let the 10 kHz tone decay out of it.
    tokio::time::sleep(Duration::from_millis(5000)).await;
    let off = wait(&c, 2, Stream::Tf, "tf off", |d| {
        tf(d).filter(|f| !f.meta.mic_curve && f.mag[col].is_finite())
    })
    .await;
    let rta_off = wait(&c, 4, Stream::Rta, "rta off", |d| match d {
        FrameData::Rta(f) if !f.meta.mic_curve => Some(f.level.clone()),
        _ => None,
    })
    .await;
    set_curve(&c, "M30", label("M30")).await;
    let on = wait(&c, 2, Stream::Tf, "tf on", |d| {
        tf(d).filter(|f| f.meta.mic_curve && f.mag[col].is_finite())
    })
    .await;
    assert!(
        (off.mag[col] + 6.02).abs() < 0.3,
        "acoustic gain 0.5: {}",
        off.mag[col]
    );
    assert!(
        (off.mag[col] - on.mag[col] - 6.0).abs() < 0.3,
        "{} vs {}",
        off.mag[col],
        on.mag[col]
    );
    assert!((off.phase[col] - on.phase[col]).abs() < 5.0);
    let rta_on = wait(&c, 4, Stream::Rta, "rta on", |d| match d {
        FrameData::Rta(f) if f.meta.mic_curve => Some(f.level.clone()),
        _ => None,
    })
    .await;
    // Band 10 kHz is index 27 of 20 Hz … 20 kHz; 1 kHz (index 17) is not corrected.
    let d10 = rta_off[27] - rta_on[27];
    let d1 = rta_off[17] - rta_on[17];
    assert!((d10 - 6.0).abs() < 1.0, "10 kHz band {d10}");
    assert!(d1.abs() < 1.0, "1 kHz band {d1}");

    // A captured RTA trace records the input's mic, its curve and the verified calibration.
    let t = capture(&c, 4, "rta-m30").await;
    let key = CalKey {
        device: st.session.open.as_ref().unwrap().input_device.clone(),
        channel: 1,
        mic: "M30".into(),
    };
    let mic = t.mic.clone().unwrap();
    assert_eq!(mic.name, "M30");
    assert_eq!(mic.curve.as_ref().map(|c| c.label.as_str()), Some("M30"));
    assert_eq!(
        mic.curve.as_ref().map(|c| c.content_hash.as_str()),
        Some(r.content_hash.as_str())
    );
    assert_eq!(
        t.cal,
        CalState::Calibrated {
            key: key.clone(),
            sensitivity: cal.sensitivity,
            calibrated_at: cal.calibrated_at,
        }
    );
    // A transfer function is a ratio: no calibration, but the mic and its curve.
    let t = capture(&c, 2, "tf-m30").await;
    assert_eq!(t.cal, CalState::Uncalibrated);
    assert_eq!(
        t.mic
            .as_ref()
            .and_then(|m| m.curve.as_ref())
            .map(|c| c.label.as_str()),
        Some("M30")
    );

    // Another mic on the input: the calibration still applies but is flagged; no curve
    // (M30's curve belongs to M30, and none is stored for the ECM). Choosing M30's label for
    // the ECM is refused.
    let e = c
        .call(Command::SessionInputs {
            inputs: vec![InputSetup {
                channel: 1,
                mic: Some("ECM".into()),
                curve: label("M30"),
            }],
        })
        .await
        .unwrap_err();
    assert!(
        matches!(&e, ClientError::Daemon(p) if p.code == ErrorCode::Invalid && p.msg.contains("no curve")),
        "{e:?}"
    );
    set_curve(&c, "ECM", CurveChoice::NotChosen).await;
    let st = state_until(&c, |s| s.inputs[0].mic.as_deref() == Some("ECM")).await;
    assert!(matches!(
        ac2_proto::cal::state_input_use(&st, 1).curve,
        ac2_proto::cal::CurveUse::NoneStored { mic: "ECM" }
    ));
    let m = spl_until(&c, "other mic", |m| {
        matches!(m.cal, CalStatus::OtherMicOrInput { .. })
    })
    .await;
    assert!(!m.mic_curve);
    assert_eq!(m.scale, LevelScale::DbSpl);
    // Captured now: the ECM without a curve, calibrated with M30's entry (its key says so).
    let t = capture(&c, 4, "rta-ecm").await;
    assert_eq!(
        t.mic.as_ref().map(|m| (m.name.as_str(), m.curve.is_none())),
        Some(("ECM", true))
    );
    assert!(matches!(&t.cal, CalState::Calibrated { key: k, .. } if *k == key));

    lease.end().await.unwrap();
    drop(c);
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();

    // Restart on the same store: calibrations, the mic library and input setup are back.
    let h = start(&store);
    let c = connect(&h).await;
    let st = state_until(&c, |_| true).await;
    assert_eq!(st.calibrations.len(), 1);
    assert_eq!(st.calibrations[0].spl, cal);
    assert_eq!(st.mics.len(), 1);
    assert_eq!(st.mics[0].curves[0].points, 5);
    assert_eq!(st.inputs[0].mic.as_deref(), Some("ECM"));

    // Library and sensitivity changes need no session. The curve first (the calibration
    // stays), then the sensitivity.
    let missing =
        |e: ClientError| matches!(e, ClientError::Daemon(p) if p.code == ErrorCode::NotFound);
    let id = MicCurveId {
        mic: "M30".into(),
        label: "M30".into(),
    };
    let r = c
        .call(Command::CalCurveDelete { curve: id.clone() })
        .await
        .unwrap();
    assert!(matches!(r, ReplyBody::Ack { .. }), "{r:?}");
    let st = state_until(&c, |s| s.mics.is_empty()).await;
    assert_eq!(st.calibrations.len(), 1);
    assert!(missing(
        c.call(Command::CalCurveDelete { curve: id })
            .await
            .unwrap_err()
    ));
    let del = || Command::CalDelete { key: key.clone() };
    c.call(del()).await.unwrap();
    let st = state_until(&c, |s| s.calibrations.is_empty()).await;
    assert_eq!(
        st.inputs[0].mic.as_deref(),
        Some("ECM"),
        "input setup untouched"
    );
    assert!(missing(c.call(del()).await.unwrap_err()));
    drop(c);
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();

    // The deletion is in the file.
    let h = start(&store);
    let c = connect(&h).await;
    let st = state_until(&c, |_| true).await;
    assert!(st.calibrations.is_empty(), "{:?}", st.calibrations);
    drop(c);
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unreadable_store_is_refused_and_untouched() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("calibrations.json");
    let garbage = b"{\"format\": \"ac2-calibrations\", \"version\": 1, \"entries\": [";
    std::fs::write(&store, garbage).unwrap();
    let h = start(&store);
    let c = connect(&h).await;
    for cmd in [
        Command::CalList,
        Command::CalDelete {
            key: CalKey {
                device: ac2_proto::model::DeviceId("fake:loop".into()),
                channel: 1,
                mic: "M30".into(),
            },
        },
        Command::SessionInputs {
            inputs: vec![InputSetup {
                channel: 0,
                mic: Some("M30".into()),
                curve: CurveChoice::NotChosen,
            }],
        },
        Command::CalCurveImport {
            mic: "M30".into(),
            label: None,
            file_name: "c.txt".into(),
            content: Blob(CURVE.as_bytes().to_vec()),
            input: None,
        },
    ] {
        let e = c.call(cmd).await.unwrap_err();
        let ClientError::Daemon(p) = e else {
            panic!("{e:?}")
        };
        assert_eq!(p.code, ErrorCode::Refused);
        assert!(
            matches!(p.detail, Some(ErrorDetail::CalStore { .. })),
            "{p:?}"
        );
    }
    assert_eq!(std::fs::read(&store).unwrap(), garbage);
    drop(c);
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();
}

/// Two curves of one capsule (beyerdynamic MM1, 0° and 90° incidence, the manufacturer's
/// files): labels from the files, the first import chosen on the input, the second not;
/// switching 0° → 90° → off moves the served transfer-function magnitude by exactly the
/// curves' difference (both normalised at 1 kHz, the input being uncalibrated); captures
/// record the curve in use; a chosen curve that is deleted says so instead of silently
/// applying nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_curves_of_one_mic_switched_on_an_input() {
    use ac2_core::mic_curve::MicCurve;
    use ac2_proto::cal::{CurveUse, state_input_use};
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let h = start(&dir.path().join("calibrations.json"));
    let c = connect(&h).await;
    c.call(Command::SessionOpen {
        config: session(false),
    })
    .await
    .unwrap();
    let m = match c
        .call(Command::MeasCreate {
            config: transfer("tf"),
        })
        .await
        .unwrap()
    {
        ReplyBody::Measurement(m) => m,
        other => panic!("{other:?}"),
    };
    c.call(Command::MeasStart { meas: m.id }).await.unwrap();
    c.subscribe(Subscription::Meas(m.id)).unwrap();
    c.call(Command::DelaySet {
        meas: m.id,
        delay: Seconds(f64::from(ACOUSTIC_DELAY) / f64::from(FS)),
    })
    .await
    .unwrap();
    const MIC: &str = "MM1 34804";
    set_curve(&c, MIC, CurveChoice::NotChosen).await;

    let zero = fixture("449350_34804_0Grad.txt");
    let ninety = fixture("449350_34804_90Grad.txt");
    let m0 = import(&c, MIC, "449350_34804_0Grad.txt", &zero).await;
    assert_eq!(m0.curves.len(), 1);
    assert_eq!(m0.curves[0].label, "0°");
    assert_eq!(m0.curves[0].stated_sensitivity, Some(15.0));
    let m2 = import(&c, MIC, "449350_34804_90Grad.txt", &ninety).await;
    let labels: Vec<&str> = m2.curves.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, ["0°", "90°"]);
    // The first import was the mic's only curve, so the input chose it; the second leaves
    // the choice alone.
    let st = state_until(&c, |s| s.mics.first().is_some_and(|m| m.curves.len() == 2)).await;
    assert_eq!(st.inputs[0].curve, label("0°"));

    let lease = c.acquire_lease(false, OnDrop::Release).await.unwrap();
    fire(&lease, Signal::Pink).await;
    let grid_f = |k: i32| 1000.0 * 2f64.powf(f64::from(k) / 48.0);
    // Columns at 5, 8, 12.5 and 16 kHz (k_min = −240).
    let cols: Vec<usize> = [5000.0f64, 8000.0, 12_500.0, 16_000.0]
        .iter()
        .map(|f| ((48.0 * (f / 1000.0).log2()).round() as i32 + 240) as usize)
        .collect();
    let tf_with = |want: bool| {
        let c = &c;
        let cols = cols.clone();
        async move {
            // Two frames past the switch, so the frame is from after it.
            let mut seen = 0;
            wait(c, m.id.0, Stream::Tf, "tf", |d| {
                tf(d).filter(|f| {
                    let ok = f.meta.mic_curve == want && cols.iter().all(|&k| f.mag[k].is_finite());
                    if ok {
                        seen += 1;
                    }
                    ok && seen > 2
                })
            })
            .await
        }
    };
    tokio::time::sleep(Duration::from_millis(3000)).await;
    let at0 = tf_with(true).await;
    set_curve(&c, MIC, label("90°")).await;
    let at90 = tf_with(true).await;
    set_curve(&c, MIC, CurveChoice::Off).await;
    let off = tf_with(false).await;
    let c0 = MicCurve::parse(&zero).unwrap().normalised(1000.0);
    let c90 = MicCurve::parse(&ninety).unwrap().normalised(1000.0);
    for &k in &cols {
        let f = grid_f(k as i32 - 240);
        // Displayed = raw − cₙ(f): 0° → 90° moves it by c0ₙ − c90ₙ, off by +c0ₙ.
        let want = c0.db(f) - c90.db(f);
        let got = f64::from(at90.mag[k] - at0.mag[k]);
        assert!(
            (got - want).abs() < 0.25,
            "{f:.0} Hz: 0°→90° {got:+.2} dB, curves {want:+.2}"
        );
        let got = f64::from(off.mag[k] - at0.mag[k]);
        assert!(
            (got - c0.db(f)).abs() < 0.25,
            "{f:.0} Hz: off {got:+.2} dB, 0° curve {:+.2}",
            c0.db(f)
        );
        assert!(
            (at90.phase[k] - at0.phase[k]).abs() < 5.0,
            "phase untouched"
        );
    }

    // A capture keeps exactly which curve it carries: label and content hash.
    set_curve(&c, MIC, label("90°")).await;
    tf_with(true).await;
    let t = capture(&c, m.id.0, "tf-90").await;
    let used = t.mic.as_ref().and_then(|m| m.curve.clone()).unwrap();
    assert_eq!(used, m2.curves[1]);
    let export = match c
        .call(Command::TraceExport {
            trace: t.id,
            format: ac2_proto::model::ExportFormat::Ac2Csv,
        })
        .await
        .unwrap()
    {
        ReplyBody::Export { content, .. } => String::from_utf8(content.0).unwrap(),
        other => panic!("{other:?}"),
    };
    assert!(
        export.contains("MM1 34804 (curve: 90°, in the columns; file \"449350_34804_90Grad.txt\""),
        "{}",
        export.lines().take(20).collect::<Vec<_>>().join("\n")
    );

    // Deleting the chosen curve: the input keeps the choice and says the curve is missing;
    // the frames say no curve ran.
    c.call(Command::CalCurveDelete {
        curve: MicCurveId {
            mic: MIC.into(),
            label: "90°".into(),
        },
    })
    .await
    .unwrap();
    let st = state_until(&c, |s| s.mics[0].curves.len() == 1).await;
    assert_eq!(
        state_input_use(&st, 1).curve,
        CurveUse::Missing {
            mic: MIC,
            label: "90°"
        }
    );
    tf_with(false).await;
    lease.end().await.unwrap();
    drop(c);
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();
}

fn electrical(
    mic: &str,
    volts: f64,
    mic_sensitivity: Option<f64>,
    replace_acoustic: bool,
) -> Command {
    Command::CalSplElectrical {
        input: 1,
        mic: mic.into(),
        connection: ElectricalConnection::InLine,
        volts: Volts(volts),
        freq: Hz(1000.0),
        mic_sensitivity: mic_sensitivity.map(MvPerPa),
        uncertainty: None,
        replace_acoustic,
    }
}

/// Retries `cmd` while the tone the generator just started is not there or not steady yet.
async fn steady(c: &Client, cmd: Command) -> Result<ReplyBody, ClientError> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match c.call(cmd.clone()).await {
            // The tone just went on: nothing, or not settled, yet.
            Err(ClientError::Daemon(p))
                if p.msg.contains("not steady") || p.msg.contains("no tone") => {}
            other => return other,
        }
        assert!(Instant::now() < deadline, "level never steady");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

fn refused(r: Result<ReplyBody, ClientError>, what: &str) -> String {
    match r {
        Err(ClientError::Daemon(p)) if p.code == ErrorCode::Refused => p.msg,
        other => panic!("{what}: expected a refusal, got {other:?}"),
    }
}

/// Electrical calibration on the fake rig (Q7 §11): a 1 kHz tone at a known level on input
/// 2, a "measured" voltage and the data-sheet sensitivity of the mic's curve file; the SPL
/// meter then reads what the hand calculation says, with the method in its status.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn electrical_calibration_end_to_end() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("calibrations.json");
    let h = start(&store);
    let c = connect(&h).await;

    // No session: refused like cal.spl.
    let e = c
        .call(electrical("MM1 34804", 0.015, None, false))
        .await
        .unwrap_err();
    assert!(matches!(e, ClientError::Daemon(p) if p.code == ErrorCode::Invalid));

    c.call(Command::SessionOpen {
        config: session(false),
    })
    .await
    .unwrap();
    let m = match c
        .call(Command::MeasCreate {
            config: spl("meter", 1),
        })
        .await
        .unwrap()
    {
        ReplyBody::Measurement(m) => m,
        other => panic!("{other:?}"),
    };
    c.call(Command::MeasStart { meas: m.id }).await.unwrap();
    c.subscribe(Subscription::Meas(m.id)).unwrap();

    // The mic's curve file states 15.0 mV/Pa; importing it on input 2 names the mic there.
    import(
        &c,
        "MM1 34804",
        "449350_34804_0Grad.txt",
        &fixture("449350_34804_0Grad.txt"),
    )
    .await;

    // Unit slips and a mic without a data-sheet value are invalid before anything is read.
    for (cmd, want) in [
        (electrical("MM1 34804", 0.0, None, false), "outside 0.1 mV"),
        (
            electrical("MM1 34804", 0.015, Some(15_000.0), false),
            "outside 0.1",
        ),
        (electrical("ECM", 0.015, None, false), "no curve file"),
    ] {
        match c.call(cmd).await {
            Err(ClientError::Daemon(p)) if p.code == ErrorCode::Invalid => {
                assert!(p.msg.contains(want), "{want:?} in {}", p.msg);
            }
            other => panic!("{want}: {other:?}"),
        }
    }
    // Silence: no tone.
    let msg = refused(
        c.call(electrical("MM1 34804", 0.015, None, false)).await,
        "silence",
    );
    assert!(msg.contains("no tone signal"), "{msg}");

    // A tone too low for a good reading: −70 dBFS out, −76 dBFS in.
    let lease = c.acquire_lease(false, OnDrop::Release).await.unwrap();
    let tone = |level: f64| GeneratorDesired {
        settings: GeneratorSettings {
            signal: Signal::Sine { freq: Hz(1000.0) },
            level: Dbfs(level),
            band: None,
            outputs: vec![0],
        },
        armed: true,
        firing: true,
    };
    lease.set(tone(-70.0)).await.unwrap();
    let msg = refused(
        steady(&c, electrical("MM1 34804", 0.015, None, false)).await,
        "too low",
    );
    assert!(msg.contains("too low"), "{msg}");

    // The tone at −20 dBFS out reads −26.02 dBFS on the input. "Measured" 15 mV there with
    // 15 mV/Pa: 0 dBFS = 15 mV · 10^(26.02/20) ≈ 0.300 V, sensitivity
    // 20·lg(15 mV / (15 mV/Pa · 20 µPa)) − L = 93.98 + 26.02 = 120.0 dB.
    lease.set(tone(-20.0)).await.unwrap();
    spl_until(&c, "the louder tone", |m| (m.level + 26.02).abs() < 0.3).await;
    let e = match steady(&c, electrical("MM1 34804", 0.015, None, false))
        .await
        .unwrap()
    {
        ReplyBody::Calibration(e) => e,
        other => panic!("{other:?}"),
    };
    let l = e.spl.measured.0;
    assert!((l + 26.02).abs() < 0.06, "{l}");
    let CalMethod::Electrical {
        connection,
        volts,
        full_scale,
        mic_sensitivity,
        mic_sensitivity_from,
        uncertainty,
    } = &e.spl.method
    else {
        panic!("{:?}", e.spl.method)
    };
    assert_eq!(*connection, ElectricalConnection::InLine);
    assert_eq!(*volts, Volts(0.015));
    assert!((full_scale.0 - 0.015 / 10f64.powf(l / 20.0)).abs() < 1e-12);
    assert_eq!(*mic_sensitivity, MvPerPa(15.0));
    assert_eq!(
        *mic_sensitivity_from,
        SensitivitySource::DataSheet {
            label: "0°".into(),
            file_name: "449350_34804_0Grad.txt".into()
        }
    );
    assert_eq!(*uncertainty, Db(1.0));
    // By hand: 15 mV / 15 mV/Pa = 1 Pa = 93.98 dB SPL, read at L dBFS.
    let want = 20.0 * (1.0 / 20e-6f64).log10() - l;
    assert!((e.spl.sensitivity.0 - want).abs() < 1e-9);
    assert!((e.spl.sensitivity.0 - 120.0).abs() < 0.07);

    // The SPL meter reads 93.98 dB SPL (1 Pa at the mic gives 15 mV), verified, electrical.
    let m = spl_until(&c, "electrically calibrated", |m| {
        matches!(
            m.cal,
            CalStatus::Verified {
                basis: CalBasis::Electrical {
                    data_sheet: true,
                    ..
                },
                ..
            }
        ) && m.scale == LevelScale::DbSpl
    })
    .await;
    assert!((m.level - 93.98).abs() < 0.2, "{}", m.level);

    // A calibrator replaces it without asking; an electrical one replaces a calibrator's
    // only on purpose.
    let r = steady(
        &c,
        Command::CalSpl {
            input: 1,
            mic: "MM1 34804".into(),
            calibrator_level: DbSpl(94.0),
            calibrator_freq: Hz(1000.0),
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(&r, ReplyBody::Calibration(e) if matches!(e.spl.method, CalMethod::Acoustic { .. })),
        "{r:?}"
    );
    let msg = refused(
        c.call(electrical("MM1 34804", 0.015, None, false)).await,
        "acoustic",
    );
    assert!(msg.contains("acoustic calibration"), "{msg}");
    let r = steady(&c, electrical("MM1 34804", 0.0151, Some(15.0), true))
        .await
        .unwrap();
    let ReplyBody::Calibration(e) = r else {
        panic!("{r:?}")
    };
    assert!(matches!(
        e.spl.method,
        CalMethod::Electrical {
            mic_sensitivity_from: SensitivitySource::Typed,
            ..
        }
    ));
    let st = state_until(&c, |s| {
        s.calibrations
            .iter()
            .any(|x| matches!(x.spl.method, CalMethod::Electrical { .. }))
    })
    .await;
    assert_eq!(st.calibrations.len(), 1);

    lease.end().await.unwrap();
    drop(c);
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();

    // The method is in the store file.
    let h = start(&store);
    let c = connect(&h).await;
    let st = state_until(&c, |_| true).await;
    assert_eq!(st.calibrations.len(), 1);
    assert_eq!(st.calibrations[0].spl, e.spl);
    drop(c);
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();
}
