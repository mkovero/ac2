//! Calibration end to end on the fake rig (`docs/design/q7-calibration.md`): cal.spl against
//! a generated "calibrator" tone, the input setup's mic name deciding verified vs other mic,
//! a mic-curve import correcting SPL, spectrum, RTA and TF magnitude (and nothing at the
//! calibrator frequency), switching it off, two curves of one mic (0° / 90°) switched on an
//! input, persistence across a restart, and an unreadable store that is refused and never
//! overwritten.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::{Duration, Instant};

use ac2_client::{Client, ClientError, OnDrop, StimulusLease};
use ac2_proto::frame::{FrameData, SpecFrame, SplMeta, TfFrame};
use ac2_proto::model::{
    BandFraction, CalBasis, CalKey, CalMethod, CalRef, CalState, CalStatus, CurveChoice,
    ElectricalConnection, GeneratorDesired, GeneratorSettings, InputKind, InputSetup, LevelScale,
    MeasConfig, MeasKind, Mic, MicCurveId, MicLoading, RtaConfig, SensitivitySource, Signal,
    SpecAveraging, SpectrumConfig, State, TraceMeta, Weighting, Window,
};
use ac2_proto::units::{Blob, Db, DbSpl, Dbfs, Hz, MeasId, MvPerPa, Ohms, Seconds, Volts};
use ac2_proto::{Command, ErrorCode, ErrorDetail, ReplyBody, Stream, Subscription, Topic};
use ac2d::{Daemon, Handle};
use common::*;

mod curves;
mod electrical;
mod end_to_end;

const CURVE: &str = "* test mic\n20 0\n1000 0\n4000 0\n8000 6\n24000 6\n";

async fn connect(h: &Handle) -> Client {
    mirror(h, "ac2d-cal-test").await
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
    let poll = Poll::new(Duration::from_secs(15), 40);
    frame_until(c, (MeasId(meas), stream), poll, what, |f| {
        if f.stale { None } else { pick(&f.frame.data) }
    })
    .await
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
    c.call(Command::TraceCapture {
        meas: MeasId(meas),
        name: name.into(),
        slot: None,
    })
    .await
    .unwrap()
    .trace()
}

async fn set_curve(c: &Client, mic: &str, curve: CurveChoice) {
    c.call(Command::SessionInputs {
        inputs: vec![InputSetup {
            channel: 1,
            kind: InputKind::Mic {
                mic: Some(mic.into()),
                curve,
            },
            pair: None,
        }],
        pairs: Vec::new(),
    })
    .await
    .unwrap();
}

fn label(l: &str) -> CurveChoice {
    CurveChoice::Curve { label: l.into() }
}

async fn import(c: &Client, mic: &str, file_name: &str, content: &[u8]) -> Mic {
    c.call(Command::CalCurveImport {
        mic: mic.into(),
        label: None,
        file_name: file_name.into(),
        content: Blob(content.to_vec()),
        input: Some(1),
    })
    .await
    .unwrap()
    .mic()
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
