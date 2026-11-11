//! Rolling Leq windows end to end on the fake rig (`docs/design/leq.md`): a calibrated
//! 1 kHz tone at a known level drives a 5 s and a 10 s window over their limit and, turned
//! down, back under it; the alarms, states, `leq` frames and the per-second log are
//! checked, the windows change in place, survive a stopped meter and a session save/load.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::{Duration, Instant};

use ac2_client::{Client, ClientError, OnDrop, StimulusLease};
use ac2_proto::frame::{FrameData, LeqFlags, LeqFrame};
use ac2_proto::model::{
    GeneratorDesired, GeneratorSettings, HistorySpan, LeqAlarmKind, LeqConfig, LeqJudgement,
    LeqWindow, LevelScale, MeasConfig, MeasKind, PeakWeighting, SessionRef, Signal, SplConfig,
    SplHistory, SplLog, SplLogPage, SplLogWhich, State, TimeWeighting, Weighting,
};
use ac2_proto::units::{Db, DbSpl, Dbfs, Hz, MeasId, Seconds};
use ac2_proto::{Command, ErrorCode, ReplyBody, Stream, Subscription};
use ac2d::{Daemon, Handle};
use common::*;

mod levels;
mod log;
mod windows;

const M: MeasId = MeasId(1);

async fn connect(h: &Handle) -> Client {
    mirror(h, "ac2d-leq-test").await
}

fn tone(level: f64) -> GeneratorDesired {
    GeneratorDesired {
        settings: GeneratorSettings {
            signal: Signal::Sine { freq: Hz(1000.0) },
            level: Dbfs(level),
            band: None,
            outputs: vec![0],
        },
        armed: true,
        firing: true,
    }
}

fn window(seconds: f64, limit: Option<f64>) -> LeqWindow {
    LeqWindow {
        cue: None,
        duration: Seconds(seconds),
        weighting: Weighting::A,
        limit: limit.map(DbSpl),
        warn_margin: Db(3.0),
    }
}

fn meter(windows: Vec<LeqWindow>) -> MeasConfig {
    MeasConfig {
        scope: ac2_proto::model::MeasScope::Session,
        name: "FOH SPL".into(),
        kind: MeasKind::Spl {
            config: SplConfig {
                cue: None,
                bands: None,
                input: 1,
                weighting: Weighting::A,
                time_weighting: TimeWeighting::Fast,
                peak_weighting: PeakWeighting::C,
                leq: LeqConfig {
                    windows,
                    horizon: Seconds(2.0),
                    peaks: Default::default(),
                },
                position: None,
            },
        },
    }
}

const WAIT: Duration = Duration::from_secs(40);
/// [`WAIT`], looking every 50 ms.
const POLL: Poll = Poll::new(WAIT, 50);

/// The newest `leq` frame once `ok` holds.
async fn leq_until(c: &Client, what: &str, ok: impl Fn(&LeqFrame) -> bool) -> LeqFrame {
    frame_until(c, (M, Stream::Leq), POLL, what, |f| match &f.frame.data {
        FrameData::Leq(l) if ok(l) => Some((**l).clone()),
        _ => None,
    })
    .await
}

/// The meter's `spl_log` entity once `ok` holds.
async fn log_until(c: &Client, what: &str, ok: impl Fn(&SplLog) -> bool) -> SplLog {
    mirror_until(c, POLL, what, |s| {
        let log = s.spl_logs.iter().find(|l| l.meas == M);
        log.filter(|l| ok(l)).cloned()
    })
    .await
}

async fn state(c: &Client) -> std::sync::Arc<State> {
    c.view().state.clone().unwrap()
}

async fn page(c: &Client, from: u64) -> SplLogPage {
    c.call(Command::SplLogGet {
        meas: M,
        log: SplLogWhich::Current,
        from,
        max: 100_000,
    })
    .await
    .unwrap()
    .spl_log_page()
}

async fn calibrate(c: &Client) {
    let deadline = Instant::now() + WAIT;
    loop {
        match c
            .call(Command::CalSpl {
                input: 1,
                mic: "M30".into(),
                calibrator_level: DbSpl(94.0),
                calibrator_freq: Hz(1000.0),
            })
            .await
        {
            Ok(ReplyBody::Calibration(_)) => return,
            Err(ClientError::Daemon(p)) if p.msg.contains("not steady") => {}
            other => panic!("{other:?}"),
        }
        assert!(Instant::now() < deadline, "calibrator never steady");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

async fn set_level(lease: &StimulusLease, level: f64) {
    lease.set(tone(level)).await.unwrap();
}

/// The newest `spl` frame (with its stamp's rev) once `ok` holds.
async fn spl_until(
    c: &Client,
    what: &str,
    ok: impl Fn(&ac2_proto::frame::SplFrame, ac2_proto::units::Rev) -> bool,
) -> ac2_proto::frame::SplFrame {
    let poll = Poll::new(WAIT, 20);
    frame_until(c, (M, Stream::Spl), poll, what, |f| match &f.frame.data {
        FrameData::Spl(s) if ok(s, f.frame.stamp.config_rev) => Some(s.clone()),
        _ => None,
    })
    .await
}
