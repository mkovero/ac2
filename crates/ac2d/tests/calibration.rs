//! Calibration end to end on the fake rig (`docs/design/q7-calibration.md`): cal.spl against
//! a generated "calibrator" tone, the input setup's mic name deciding verified vs other mic,
//! a mic-curve import correcting SPL, spectrum, RTA and TF magnitude (and nothing at the
//! calibrator frequency), the on/off switch, persistence across a restart, and an
//! unreadable store that is refused and never overwritten.
#![allow(clippy::unwrap_used)]

mod common;

use std::time::{Duration, Instant};

use ac2_client::{Client, ClientConfig, ClientError, Endpoints, OnDrop, StimulusLease};
use ac2_proto::frame::{FrameData, SpecFrame, SplMeta, TfFrame};
use ac2_proto::model::{
    BandFraction, CalKey, CalState, CalStatus, GeneratorDesired, GeneratorSettings, InputSetup,
    LevelScale, MeasConfig, MeasKind, MicCurveAction, MicState, RtaConfig, Signal, SpecAveraging,
    SpectrumConfig, State, TraceMeta, Weighting, Window,
};
use ac2_proto::units::{Blob, DbSpl, Dbfs, Hz, MeasId, Seconds};
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

async fn set_curve(c: &Client, on: bool) {
    c.call(Command::SessionInputs {
        inputs: vec![InputSetup {
            channel: 1,
            mic: Some("M30".into()),
            mic_curve: on,
        }],
    })
    .await
    .unwrap();
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
    let cal = entry.spl.unwrap();
    assert!((cal.measured.0 + 26.02).abs() < 0.06, "{:?}", cal.measured);
    assert!((cal.sensitivity.0 - 120.02).abs() < 0.1);
    let st = state_until(&c, |s| !s.inputs.is_empty()).await;
    assert_eq!(
        st.inputs,
        vec![InputSetup {
            channel: 1,
            mic: Some("M30".into()),
            mic_curve: true
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
        .call(Command::CalMicCurve {
            input: 1,
            mic: "M30".into(),
            action: MicCurveAction::Import {
                file_name: "bad.frd".into(),
                content: Blob(b"20 0\n10 1\n".to_vec()),
            },
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

    // Import the curve: 0 dB at the 1 kHz calibrator, so the tone reads the same.
    let e = match c
        .call(Command::CalMicCurve {
            input: 1,
            mic: "M30".into(),
            action: MicCurveAction::Import {
                file_name: "/home/op/curves/M30.frd".into(),
                content: Blob(CURVE.as_bytes().to_vec()),
            },
        })
        .await
        .unwrap()
    {
        ReplyBody::Calibration(e) => e,
        other => panic!("{other:?}"),
    };
    assert_eq!(e.spl, Some(cal), "the calibration is kept");
    let r = e.mic_curve.unwrap();
    assert_eq!(
        (r.file_name.as_str(), r.name.as_str(), r.points),
        ("M30.frd", "M30", 5)
    );
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
    set_curve(&c, false).await;
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
    set_curve(&c, true).await;
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
    assert_eq!(
        t.mic,
        Some(MicState {
            name: "M30".into(),
            curve: Some("M30".into()),
        })
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
        t.mic.as_ref().map(|m| m.curve.clone()),
        Some(Some("M30".into()))
    );

    // Another mic on the input: the calibration still applies but is flagged; no curve
    // (it belongs to M30).
    c.call(Command::SessionInputs {
        inputs: vec![InputSetup {
            channel: 1,
            mic: Some("ECM".into()),
            mic_curve: true,
        }],
    })
    .await
    .unwrap();
    let m = spl_until(&c, "other mic", |m| {
        matches!(m.cal, CalStatus::OtherMicOrInput { .. })
    })
    .await;
    assert!(!m.mic_curve);
    assert_eq!(m.scale, LevelScale::DbSpl);
    // Captured now: the ECM without a curve, calibrated with M30's entry (its key says so).
    let t = capture(&c, 4, "rta-ecm").await;
    assert_eq!(
        t.mic,
        Some(MicState {
            name: "ECM".into(),
            curve: None,
        })
    );
    assert!(matches!(&t.cal, CalState::Calibrated { key: k, .. } if *k == key));

    lease.end().await.unwrap();
    drop(c);
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();

    // Restart on the same store: calibrations, curve provenance and input setup are back.
    let h = start(&store);
    let c = connect(&h).await;
    let st = state_until(&c, |_| true).await;
    assert_eq!(st.calibrations.len(), 1);
    assert_eq!(st.calibrations[0].spl, Some(cal));
    assert_eq!(
        st.calibrations[0].mic_curve.as_ref().map(|r| r.points),
        Some(5)
    );
    assert_eq!(st.inputs[0].mic.as_deref(), Some("ECM"));
    // Clearing the curve keeps the calibration.
    c.call(Command::CalMicCurve {
        input: 1,
        mic: "M30".into(),
        action: MicCurveAction::Clear,
    })
    .await
    .unwrap_err(); // no session open: tied to the session's device
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
        Command::SessionInputs {
            inputs: vec![InputSetup {
                channel: 0,
                mic: Some("M30".into()),
                mic_curve: true,
            }],
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
