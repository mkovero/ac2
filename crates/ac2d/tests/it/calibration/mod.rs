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

mod curves;
mod electrical;
mod end_to_end;

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
    let mut cfg = steady_config(realtime_rig(), local_tcp());
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
