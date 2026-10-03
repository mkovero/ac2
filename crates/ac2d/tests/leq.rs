//! Rolling Leq windows end to end on the fake rig (`docs/design/leq.md`): a calibrated
//! 1 kHz tone at a known level drives a 5 s and a 10 s window over their limit and, turned
//! down, back under it; the alarms, states, `leq` frames and the per-second log are
//! checked, the windows change in place, survive a stopped meter and a session save/load.
#![allow(clippy::unwrap_used)]

mod common;

use std::time::{Duration, Instant};

use ac2_client::{Client, ClientConfig, ClientError, Endpoints, OnDrop, StimulusLease};
use ac2_proto::frame::{FrameData, LeqFlags, LeqFrame};
use ac2_proto::model::{
    GeneratorDesired, GeneratorSettings, LeqAlarmKind, LeqConfig, LeqJudgement, LeqWindow,
    LevelScale, MeasConfig, MeasKind, PeakWeighting, SessionRef, Signal, SplConfig, SplLog,
    SplLogPage, State, TimeWeighting, Weighting,
};
use ac2_proto::units::{Db, DbSpl, Dbfs, Hz, MeasId, Seconds};
use ac2_proto::{Command, ErrorCode, ReplyBody, Stream, Subscription, Topic};
use ac2d::{Daemon, Handle};
use common::*;

const M: MeasId = MeasId(1);

async fn connect(h: &Handle) -> Client {
    let ep = Endpoints {
        ctrl: h.ctrl_endpoint().to_owned(),
        data: h.data_endpoint().to_owned(),
    };
    let c = Client::connect(ClientConfig::new(ep, "ac2d-leq-test"))
        .await
        .unwrap();
    c.wait_synced(Duration::from_secs(5)).await.unwrap();
    c
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
        duration: Seconds(seconds),
        weighting: Weighting::A,
        limit: limit.map(DbSpl),
        warn_margin: Db(3.0),
    }
}

fn meter(windows: Vec<LeqWindow>) -> MeasConfig {
    MeasConfig {
        name: "FOH SPL".into(),
        kind: MeasKind::Spl {
            config: SplConfig {
                input: 1,
                weighting: Weighting::A,
                time_weighting: TimeWeighting::Fast,
                peak_weighting: PeakWeighting::C,
                leq: LeqConfig {
                    windows,
                    horizon: Seconds(2.0),
                },
            },
        },
    }
}

const WAIT: Duration = Duration::from_secs(40);

/// The newest `leq` frame once `ok` holds.
async fn leq_until(c: &Client, what: &str, ok: impl Fn(&LeqFrame) -> bool) -> LeqFrame {
    let topic = Topic::Data {
        meas: M,
        stream: Stream::Leq,
    };
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(f) = c.latest().unwrap().get(&topic)
            && let FrameData::Leq(l) = &f.frame.data
            && ok(l)
        {
            return l.clone();
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The meter's `spl_log` entity once `ok` holds.
async fn log_until(c: &Client, what: &str, ok: impl Fn(&SplLog) -> bool) -> SplLog {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(s) = c.view().state.clone()
            && let Some(l) = s.spl_logs.iter().find(|l| l.meas == M)
            && ok(l)
        {
            return l.clone();
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn state(c: &Client) -> std::sync::Arc<State> {
    c.view().state.clone().unwrap()
}

async fn page(c: &Client, from: u64) -> SplLogPage {
    match c
        .call(Command::SplLogGet {
            meas: M,
            from,
            max: 100_000,
        })
        .await
        .unwrap()
    {
        ReplyBody::SplLogPage(p) => p,
        other => panic!("{other:?}"),
    }
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn windows_go_over_and_recover_and_survive() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(realtime_rig(), local_tcp());
    cfg.cal_store = Some(dir.path().join("calibrations.json"));
    let h = Daemon::start(cfg).unwrap();
    let c = connect(&h).await;
    c.call(Command::SessionOpen {
        config: session(false),
    })
    .await
    .unwrap();

    // Invalid windows are refused at creation.
    let e = c
        .call(Command::MeasCreate {
            config: meter(vec![window(2.5, None)]),
        })
        .await
        .unwrap_err();
    assert!(matches!(e, ClientError::Daemon(p) if p.code == ErrorCode::Invalid));

    let m = match c
        .call(Command::MeasCreate {
            config: meter(vec![window(5.0, Some(90.0)), window(10.0, Some(90.0))]),
        })
        .await
        .unwrap()
    {
        ReplyBody::Measurement(m) => m,
        other => panic!("{other:?}"),
    };
    assert_eq!(m.id, M);
    // The entity exists before the meter runs: limits set, nothing calibrated.
    let l = log_until(&c, "entity", |_| true).await;
    assert_eq!(l.windows.len(), 2);
    assert!(
        l.windows
            .iter()
            .all(|w| w.judgement == LeqJudgement::NotCalibrated)
    );
    c.call(Command::MeasStart { meas: M }).await.unwrap();
    c.subscribe(Subscription::Meas(M)).unwrap();

    // Uncalibrated: dBFS values, limits not judged. The tone reaches input 1 at
    // −20 dBFS × 0.5 = −26 dBFS.
    let lease = c.acquire_lease(false, OnDrop::Release).await.unwrap();
    set_level(&lease, -20.0).await;
    let f = leq_until(&c, "uncalibrated tone over 5 s", |f| {
        f.elapsed[0] >= 5.0 && (f.leq[0] + 26.02).abs() < 0.1
    })
    .await;
    assert_eq!(f.meta.scale, LevelScale::Dbfs);
    assert_eq!(f.flags[0].judgement(), LeqJudgement::NotCalibrated);
    assert!(f.allowed[0].is_nan());

    // Calibrated against the tone itself (94 dB): both windows read 94 dB SPL, over the
    // 90 dB limit; the 5 s window cannot recover within the 2 s horizon (3 s at 94 dB stay
    // in it), the 10 s window cannot either.
    calibrate(&c).await;
    let f = leq_until(&c, "over", |f| {
        f.meta.scale == LevelScale::DbSpl
            && f.flags.iter().all(|x| x.contains(LeqFlags::OVER))
            && f.elapsed[1] >= 10.0
    })
    .await;
    assert!((f.leq[0] - 94.0).abs() < 0.2, "{:?}", f.leq);
    assert!((f.leq[1] - 94.0).abs() < 0.2, "{:?}", f.leq);
    assert!(f.flags[0].contains(LeqFlags::CANNOT_RECOVER));
    // Playing at the limit, the 5 s window is back at it once the 94 dB seconds have left:
    // 5 s from now.
    assert!((f.recover[0] - 5.0).abs() <= 1.0, "{:?}", f.recover);
    let l = log_until(&c, "over alarms", |l| {
        l.alarms
            .iter()
            .filter(|a| a.kind == LeqAlarmKind::Over)
            .count()
            == 2
    })
    .await;
    assert!(l.windows.iter().all(|w| w.judgement == LeqJudgement::Over));
    let a = l.alarms[0];
    assert_eq!(a.limit, DbSpl(90.0));
    assert!(a.leq.0 > 90.0);

    // Turned down 20 dB (74 dB SPL): the 5 s window recovers first, then the 10 s one;
    // each recovery is an alarm entry with the window's value then.
    set_level(&lease, -40.0).await;
    let l = log_until(&c, "5 s window recovered", |l| {
        l.windows[0].judgement == LeqJudgement::Ok
    })
    .await;
    let rec = l
        .alarms
        .iter()
        .find(|a| a.kind == LeqAlarmKind::Recovered && a.duration == Seconds(5.0))
        .copied()
        .expect("recovery of the 5 s window");
    assert!(rec.leq.0 <= 90.0 + 1e-9, "{rec:?}");
    let l = log_until(&c, "10 s window recovered", |l| {
        l.windows.iter().all(|w| w.judgement == LeqJudgement::Ok)
    })
    .await;
    assert_eq!(
        l.alarms
            .iter()
            .filter(|a| a.kind == LeqAlarmKind::Recovered)
            .count(),
        2
    );
    let f = leq_until(&c, "both at 74 dB", |f| {
        (f.leq[0] - 74.0).abs() < 0.3 && (f.leq[1] - 74.0).abs() < 0.3
    })
    .await;
    assert!(f.flags.iter().all(|x| x.judgement() == LeqJudgement::Ok));
    // Headroom at 74 dB in a 10 s window with a 2 s horizon: 8 s at 74 dB stay, so the
    // next 2 s may be 10·lg((10^9·10 − 10^7.4·8) / 2) = 96.9 dB.
    let want = 10.0 * ((10f64.powf(9.0) * 10.0 - 10f64.powf(7.4) * 8.0) / 2.0).log10();
    assert!(
        (f64::from(f.allowed[1]) - want).abs() < 0.3,
        "{:?} vs {want}",
        f.allowed
    );

    // The per-second log: rows at −26 dBFS before the calibration, 94 and 74 dB SPL after.
    let p = page(&c, 0).await;
    assert_eq!(p.from, 0);
    assert_eq!(p.total as usize, p.rows.len());
    assert!(p.rows.iter().all(|r| r.measured == Seconds(1.0)));
    assert!(
        p.rows
            .iter()
            .any(|r| r.sensitivity.is_none() && (r.laeq.0 + 26.02).abs() < 0.1)
    );
    let spl = |r: &ac2_proto::model::SplLogRow| r.laeq.0 + r.sensitivity.map_or(0.0, |s| s.0);
    assert!(
        p.rows
            .iter()
            .any(|r| r.sensitivity.is_some() && (spl(r) - 94.0).abs() < 0.2)
    );
    assert!((spl(p.rows.last().unwrap()) - 74.0).abs() < 0.2);
    assert!(p.rows.windows(2).all(|w| w[1].start.0 > w[0].start.0));
    let later = page(&c, p.total - 2).await;
    assert_eq!(
        (later.from, later.rows.len()),
        (p.total - 2, later.rows.len())
    );

    // Windows changed in place: the meter is not restarted and its log carries on.
    let before = state(&c).await;
    let rev_before = before.measurements[0].config_rev;
    let rows_before = page(&c, 0).await.total;
    let r = c
        .call(Command::MeasUpdate {
            meas: M,
            config: meter(vec![window(5.0, Some(70.0)), window(20.0, None)]),
        })
        .await
        .unwrap();
    let ReplyBody::Measurement(m) = r else {
        panic!()
    };
    assert!(m.config_rev > rev_before);
    let f = leq_until(&c, "frames of the new windows", |f| {
        f.leq.len() == 2 && f.elapsed[1] > 10.0
    })
    .await;
    // The 20 s window is rebuilt from the log: more than the seconds since the update.
    assert!(f.elapsed[1] >= 15.0, "{:?}", f.elapsed);
    assert_eq!(f.flags[1].judgement(), LeqJudgement::NoLimit);
    let l = log_until(&c, "5 s window over the new 70 dB limit", |l| {
        l.windows.len() == 2 && l.windows[0].judgement == LeqJudgement::Over
    })
    .await;
    assert_eq!(l.windows[1].judgement, LeqJudgement::NoLimit);
    // The log goes on past the change.
    let deadline = Instant::now() + WAIT;
    while page(&c, 0).await.total <= rows_before {
        assert!(Instant::now() < deadline, "the log stopped at the change");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Stopped for a while and started again: the windows are rebuilt from the log with
    // the stop as a gap (incomplete, not silence).
    leq_until(&c, "20 s at 74 dB", |f| {
        f.elapsed[1] >= 20.0 && (f.leq[1] - 74.0).abs() < 0.1
    })
    .await;
    c.call(Command::MeasStop { meas: M }).await.unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
    c.call(Command::MeasStart { meas: M }).await.unwrap();
    let f = leq_until(&c, "incomplete 20 s window after the gap", |f| {
        f.flags[1].contains(LeqFlags::INCOMPLETE) && f.elapsed[1] >= 20.0
    })
    .await;
    assert!(f.measured[1] < 19.0, "{:?}", f.measured);
    assert!(
        (f.leq[1] - 74.0).abs() < 0.3,
        "the gap is not silence: {:?}",
        f.leq
    );

    // Saved and loaded: the log comes back with the meter.
    drop(lease);
    let total = page(&c, 0).await.total;
    let path = dir.path().join("show").to_string_lossy().into_owned();
    c.call(Command::FileSave {
        session: SessionRef::Path { path: path.clone() },
    })
    .await
    .unwrap();
    c.call(Command::FileLoad {
        session: SessionRef::Path { path },
    })
    .await
    .unwrap();
    let p = page(&c, 0).await;
    assert!(p.total >= total, "{} < {total}", p.total);
    assert!((spl(&p.rows[p.rows.len() - 1]) - 74.0).abs() < 0.3);
    log_until(&c, "entity after the load", |l| l.windows.len() == 2).await;

    // Not an SPL meter: invalid; no such measurement: not found.
    let t = match c
        .call(Command::MeasCreate {
            config: transfer("tf"),
        })
        .await
        .unwrap()
    {
        ReplyBody::Measurement(m) => m.id,
        other => panic!("{other:?}"),
    };
    let e = c
        .call(Command::SplLogGet {
            meas: t,
            from: 0,
            max: 1,
        })
        .await
        .unwrap_err();
    assert!(matches!(e, ClientError::Daemon(p) if p.code == ErrorCode::Invalid));
    // Deleting the meter deletes its log entity.
    c.call(Command::MeasDelete { meas: M }).await.unwrap();
    let deadline = Instant::now() + WAIT;
    while state(&c).await.spl_logs.iter().any(|l| l.meas == M) {
        assert!(Instant::now() < deadline, "entity not deleted");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let e = c
        .call(Command::SplLogGet {
            meas: M,
            from: 0,
            max: 1,
        })
        .await
        .unwrap_err();
    assert!(matches!(e, ClientError::Daemon(p) if p.code == ErrorCode::NotFound));
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();
}
