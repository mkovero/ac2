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
    LevelScale, MeasConfig, MeasKind, PeakWeighting, SessionRef, Signal, SplConfig, SplHistory,
    SplLog, SplLogPage, SplLogWhich, State, TimeWeighting, Weighting,
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
            return (**l).clone();
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
            log: SplLogWhich::Current,
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
    assert!(a.level.0 > 90.0);

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
        .find(|a| a.kind == LeqAlarmKind::Recovered && a.subject.duration() == Some(Seconds(5.0)))
        .copied()
        .expect("recovery of the 5 s window");
    assert!(rec.level.0 <= 90.0 + 1e-9, "{rec:?}");
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
            log: SplLogWhich::Current,
            from: 0,
            max: 1,
        })
        .await
        .unwrap_err();
    assert!(matches!(e, ClientError::Daemon(p) if p.code == ErrorCode::Invalid));
    // Deleting the meter deletes its log entity.
    c.call(Command::MeasDelete {
        meas: M,
        traces: ac2_proto::model::OwnedTraces::Keep,
    })
    .await
    .unwrap();
    let deadline = Instant::now() + WAIT;
    while state(&c).await.spl_logs.iter().any(|l| l.meas == M) {
        assert!(Instant::now() < deadline, "entity not deleted");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let e = c
        .call(Command::SplLogGet {
            meas: M,
            log: SplLogWhich::Current,
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

async fn previous_page(c: &Client) -> Result<SplLogPage, ClientError> {
    match c
        .call(Command::SplLogGet {
            meas: M,
            log: SplLogWhich::Previous,
            from: 0,
            max: 100_000,
        })
        .await?
    {
        ReplyBody::SplLogPage(p) => Ok(p),
        other => panic!("{other:?}"),
    }
}

/// The run clock and the total in the `leq` frame, then `spl.log_new`: the windows, their
/// states, the alarms, the clock and the total start over, the windows and limits stay,
/// the ended log stays readable as the previous one, and a saved session carries the new
/// log.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_log_starts_over_and_keeps_the_windows() {
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
    let windows = vec![window(3.0, Some(90.0)), window(30.0, None)];
    c.call(Command::MeasCreate {
        config: meter(windows.clone()),
    })
    .await
    .unwrap();
    // No log ended yet: no previous log.
    let e = previous_page(&c).await.unwrap_err();
    assert!(matches!(e, ClientError::Daemon(p) if p.code == ErrorCode::NotFound));
    c.call(Command::MeasStart { meas: M }).await.unwrap();
    c.subscribe(Subscription::Meas(M)).unwrap();
    let lease = c.acquire_lease(false, OnDrop::Release).await.unwrap();
    set_level(&lease, -20.0).await;
    leq_until(&c, "the tone", |f| (f.leq[0] + 26.02).abs() < 0.1).await;
    calibrate(&c).await;

    // 94 dB SPL: over the 3 s window's limit, an alarm; the run's total is 94 dB.
    log_until(&c, "over alarm", |l| {
        l.alarms.iter().any(|a| a.kind == LeqAlarmKind::Over)
    })
    .await;
    let f = leq_until(&c, "a run of 5 s", |f| {
        f.meta.scale == LevelScale::DbSpl
            && f.meta
                .run
                .is_some_and(|r| r.until.0 - r.started_at.0 >= 5_000_000_000)
    })
    .await;
    let run = f.meta.run.unwrap();
    assert!((run.laeq - 94.0).abs() < 0.3, "{run:?}");
    assert!(run.lzeq.is_finite() && run.lceq.is_finite());
    assert!(!run.trimmed);
    let first_start = page(&c, 0).await.rows[0].start;
    assert_eq!(run.started_at, first_start);
    let elapsed = (run.until.0 - run.started_at.0) as f64 / 1e9;
    assert!(
        (run.measured.0 + run.gaps.0 - elapsed).abs() < 1.5,
        "{run:?}"
    );

    // Turned down to 74 dB: the window recovers; then a new log.
    set_level(&lease, -40.0).await;
    log_until(&c, "recovered", |l| {
        l.alarms.iter().any(|a| a.kind == LeqAlarmKind::Recovered)
    })
    .await;
    leq_until(&c, "3 s at 74 dB", |f| (f.leq[0] - 74.0).abs() < 0.3).await;
    let ended_total = page(&c, 0).await.total;
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let r = c.call(Command::SplLogNew { meas: M }).await.unwrap();
    assert!(matches!(r, ReplyBody::Ack { .. }), "{r:?}");
    let l = log_until(&c, "entity reset", |l| l.alarms.is_empty()).await;
    assert!(l.windows.iter().all(|w| w.since.0 >= at));
    assert_eq!(l.windows[0].judgement, LeqJudgement::Ok);
    assert_eq!(l.windows[1].judgement, LeqJudgement::NoLimit);
    // The new log: a clock from about now, a total of the new seconds only, windows
    // filling again.
    let f = leq_until(&c, "the new run", |f| {
        f.meta
            .run
            .is_some_and(|r| r.started_at.0 + 1_000_000_000 >= at)
            && f.elapsed[1] >= 2.0
    })
    .await;
    let run = f.meta.run.unwrap();
    assert!((run.laeq - 74.0).abs() < 0.3, "{run:?}");
    assert!(f.elapsed[1] < 15.0, "{:?}", f.elapsed);
    assert!(f.meta.logged < ended_total, "{} rows", f.meta.logged);
    let new = page(&c, 0).await;
    assert_eq!(new.from, 0);
    assert!(new.rows[0].start.0 + 1_000_000_000 >= at);
    // The entity names the new log's start once it has one.
    let l = log_until(&c, "new start", |l| l.started_at.is_some()).await;
    assert_eq!(l.started_at, Some(new.rows[0].start));
    // The windows and limits are the meter's still.
    let st = state(&c).await;
    let MeasKind::Spl { config } = &st.measurements[0].config.kind else {
        panic!()
    };
    assert_eq!(config.leq.windows, windows);
    // The ended log, whole, as the previous one.
    let prev = previous_page(&c).await.unwrap();
    assert!(prev.total >= ended_total);
    assert_eq!(prev.rows[0].start, first_start);
    let spl = |r: &ac2_proto::model::SplLogRow| r.laeq.0 + r.sensitivity.map_or(0.0, |s| s.0);
    assert!(prev.rows.iter().any(|r| (spl(r) - 94.0).abs() < 0.3));
    // No alarm carried over: 74 dB stays under the limit.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(state(&c).await.spl_logs.iter().all(|l| l.alarms.is_empty()));

    // Saved and loaded: the session holds the new log only.
    drop(lease);
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
    assert_eq!(p.rows[0].start, new.rows[0].start);

    // Only SPL meters have a log to renew.
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
    let e = c.call(Command::SplLogNew { meas: t }).await.unwrap_err();
    assert!(matches!(e, ClientError::Daemon(p) if p.code == ErrorCode::Invalid));
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();
}

/// The newest `spl` frame (with its stamp's rev) once `ok` holds.
async fn spl_until(
    c: &Client,
    what: &str,
    ok: impl Fn(&ac2_proto::frame::SplFrame, ac2_proto::units::Rev) -> bool,
) -> ac2_proto::frame::SplFrame {
    let topic = Topic::Data {
        meas: M,
        stream: Stream::Spl,
    };
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(f) = c.latest().unwrap().get(&topic)
            && let FrameData::Spl(s) = &f.frame.data
            && ok(s, f.frame.stamp.config_rev)
        {
            return s.clone();
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Weightings change on a running meter in place: the next frames read the new weighting
/// at once at its settled level (Slow too: it ran all along), over the same interval; the
/// Leq windows, their states and the per-second log carry on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn weightings_change_in_place_and_keep_the_log() {
    init_log();
    let h = Daemon::start(config(realtime_rig(), local_tcp())).unwrap();
    let c = connect(&h).await;
    c.call(Command::SessionOpen {
        config: session(false),
    })
    .await
    .unwrap();
    c.call(Command::MeasCreate {
        config: meter(vec![window(3.0, None), window(30.0, None)]),
    })
    .await
    .unwrap();
    c.call(Command::MeasStart { meas: M }).await.unwrap();
    c.subscribe(Subscription::Meas(M)).unwrap();
    let lease = c.acquire_lease(false, OnDrop::Release).await.unwrap();
    // −26.02 dBFS of 1 kHz at the input.
    set_level(&lease, -20.0).await;
    let before = spl_until(&c, "LAF of the tone over 4 s", |f, _| {
        f.meta.duration.0 >= 4.0 && (f.meta.level + 26.02).abs() < 0.1
    })
    .await;
    assert_eq!(
        (before.meta.weighting, before.meta.time_weighting),
        (Weighting::A, TimeWeighting::Fast)
    );
    let leq_before = leq_until(&c, "4 s in the 30 s window", |f| f.elapsed[1] >= 4.0).await;
    let rows_before = page(&c, 0).await.total;
    let entity = state(&c).await.spl_logs[0].clone();

    let mut cfg = meter(vec![window(3.0, None), window(30.0, None)]);
    let MeasKind::Spl { config } = &mut cfg.kind else {
        unreachable!()
    };
    config.weighting = Weighting::C;
    config.time_weighting = TimeWeighting::Slow;
    config.peak_weighting = PeakWeighting::Z;
    let ReplyBody::Measurement(m) = c
        .call(Command::MeasUpdate {
            meas: M,
            config: cfg,
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(m.running);
    let after = spl_until(&c, "a frame under the new rev", |_, rev| {
        rev >= m.config_rev
    })
    .await;
    assert_eq!(
        (
            after.meta.weighting,
            after.meta.time_weighting,
            after.meta.peak_weighting
        ),
        (Weighting::C, TimeWeighting::Slow, PeakWeighting::Z)
    );
    // C at 1 kHz is −0.06 dB; LCS is settled on the first frame, and the interval did not
    // restart.
    assert!((after.meta.level + 26.08).abs() < 0.1, "{:?}", after.meta);
    assert!(
        after.meta.duration.0 >= before.meta.duration.0,
        "{:?}",
        after.meta
    );
    assert!((after.meta.leq + 26.08).abs() < 0.2, "{:?}", after.meta);
    // The windows keep filling from where they were, the log goes on, the entity is as it was.
    let f = leq_until(&c, "the 30 s window filling on", |f| {
        f.elapsed[1] >= leq_before.elapsed[1] + 2.0
    })
    .await;
    assert_eq!(f.leq.len(), 2);
    assert!(page(&c, 0).await.total > rows_before);
    let now = state(&c).await.spl_logs[0].clone();
    assert_eq!(now.started_at, entity.started_at);
    assert_eq!(now.windows, entity.windows);
    drop(lease);
}

/// A new log at 94 dB SPL with 90 dB limits on a 4 s and a 30 s window: a filling window is
/// judged on its budget (`docs/design/leq.md`, *Judging a filling window*). 2.5 times the
/// limit's power spends the 4 s budget in 1.6 s and the 30 s one in 12 s: both are on course
/// at once (near, `ON_COURSE`, the time to the budget counting down), the 4 s window goes
/// over after 2 s and the 30 s one only after about 13 s (its least level above 90.0 at
/// 0.1 dB), each an alarm then and not before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_filling_window_goes_over_when_its_budget_is_spent() {
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
    c.call(Command::MeasCreate {
        config: meter(vec![window(4.0, Some(90.0)), window(30.0, Some(90.0))]),
    })
    .await
    .unwrap();
    c.call(Command::MeasStart { meas: M }).await.unwrap();
    c.subscribe(Subscription::Meas(M)).unwrap();
    let lease = c.acquire_lease(false, OnDrop::Release).await.unwrap();
    set_level(&lease, -20.0).await;
    leq_until(&c, "the tone", |f| (f.leq[0] + 26.02).abs() < 0.1).await;
    calibrate(&c).await;
    leq_until(&c, "94 dB SPL", |f| {
        f.meta.scale == LevelScale::DbSpl && (f.leq[0] - 94.0).abs() < 0.2
    })
    .await;

    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    c.call(Command::SplLogNew { meas: M }).await.unwrap();
    log_until(&c, "entity reset", |l| l.alarms.is_empty()).await;
    let fresh = |f: &LeqFrame| {
        f.meta
            .run
            .is_some_and(|r| r.started_at.0 + 1_000_000_000 >= at)
    };
    // The first seconds: the 30 s window on course, the Leq so far 94 dB, its least level
    // (the rest silent) under it, its headroom the level that spends what is left of the
    // budget by the end of the fill.
    let f = leq_until(&c, "the 30 s window on course", |f| {
        fresh(f) && (2.0..=8.0).contains(&f.elapsed[1])
    })
    .await;
    let fl = f.flags[1];
    assert!(
        fl.contains(LeqFlags::NEAR) && fl.contains(LeqFlags::ON_COURSE),
        "{fl:?}"
    );
    assert!(!fl.contains(LeqFlags::OVER));
    assert_eq!(fl.judgement(), LeqJudgement::Near);
    assert!((f.leq[1] - 94.0).abs() < 0.3, "{:?}", f.leq);
    let (e, m) = (f64::from(f.elapsed[1]), f64::from(f.measured[1]));
    let r = 30.0 - e;
    let least = f64::from(f.leq[1]) + 10.0 * (m / (m + r)).log10();
    assert!(
        (f64::from(f.least[1]) - least).abs() < 0.05,
        "{:?}",
        f.least
    );
    // Budget 10^9·(m + r), spent at 10^(L/10) a second.
    let pace = 10f64.powf(f64::from(f.leq[1]) / 10.0);
    let over_in = (1e9 * (m + r) - pace * m) / pace;
    assert!(
        (f64::from(f.over_in[1]) - over_in).abs() < 0.2,
        "{:?} vs {over_in}",
        f.over_in
    );
    let allowed = 10.0 * ((1e9 * (m + r) - pace * m) / r).log10();
    assert!(
        (f64::from(f.allowed[1]) - allowed).abs() < 0.05,
        "{:?} vs {allowed}",
        f.allowed
    );
    // The 4 s window spent its budget long ago: over, and its alarm the only one.
    assert!(f.flags[0].contains(LeqFlags::OVER), "{:?}", f.flags);
    let l = log_until(&c, "the 4 s window's alarm", |l| !l.alarms.is_empty()).await;
    assert!(
        l.alarms
            .iter()
            .all(|a| a.subject.duration() == Some(Seconds(4.0)) && a.kind == LeqAlarmKind::Over),
        "{:?}",
        l.alarms
    );
    assert_eq!(l.windows[1].judgement, LeqJudgement::Near);

    // The 30 s window over once the budget is spent, about 13 s into the log.
    let l = log_until(&c, "the 30 s window over", |l| {
        l.alarms
            .iter()
            .any(|a| a.subject.duration() == Some(Seconds(30.0)))
    })
    .await;
    let start = page(&c, 0).await.rows[0].start.0;
    let over_at = |d: f64| {
        let a = l
            .alarms
            .iter()
            .find(|a| a.subject.duration() == Some(Seconds(d)))
            .unwrap();
        assert_eq!(a.kind, LeqAlarmKind::Over);
        (a.at.0 - start) as f64 / 1e9
    };
    let (t4, t30) = (over_at(4.0), over_at(30.0));
    assert!((1.5..=3.5).contains(&t4), "4 s window over after {t4} s");
    assert!(
        (11.5..=14.5).contains(&t30),
        "30 s window over after {t30} s"
    );
    let f = leq_until(&c, "the 30 s window over in the frame", |f| {
        f.flags[1].contains(LeqFlags::OVER)
    })
    .await;
    assert!(!f.flags[1].contains(LeqFlags::ON_COURSE));
    assert!(f.least[1] > 90.0 && f.elapsed[1] < 30.0, "{f:?}");
    assert!(f.over_in[1].is_nan());
    drop(lease);
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();
}

/// A log longer than a page (a loaded session's 45000 seconds): `spl.log_get` pages it,
/// every row once; `spl.history_get` gives its windows second by second over the last
/// 4 h, the first of those computed over the rows before them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_long_log_in_pages_and_its_history() {
    use ac2_proto::model::SplLogRow;
    use ac2_proto::units::WallNs;
    use ac2_traces::session::{SavedMeasurement, SavedSplLog, Session};
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let h = Daemon::start(config(realtime_rig(), local_tcp())).unwrap();
    let c = connect(&h).await;
    let n = SplLogPage::MAX_ROWS as u64 * 2 + 5000;
    let t0 = 1_790_000_000_000_000_000;
    let rows: Vec<SplLogRow> = (0..n)
        .map(|k| SplLogRow {
            start: WallNs(t0 + k * 1_000_000_000),
            measured: Seconds(1.0),
            laeq: Dbfs(-30.0 - (k % 10) as f64),
            lceq: Dbfs(-28.0),
            lzeq: Dbfs(-27.0),
            lcpeak: Dbfs(-12.0),
            lafmax: Dbfs(-25.0),
            sensitivity: Some(Db(120.0)),
            position: None,
        })
        .collect();
    let path = dir.path().join("long");
    ac2_traces::session::save(
        &path,
        &Session {
            saved_at: WallNs(t0),
            measurements: vec![SavedMeasurement {
                id: M,
                config: meter(vec![window(60.0, Some(95.0))]),
                running: false,
                delay: None,
            }],
            spl_logs: vec![SavedSplLog {
                info: ac2_traces::spl_log::SplLogInfo {
                    meas: M,
                    name: "FOH SPL".into(),
                    input: 1,
                    mic: None,
                },
                rows: rows.clone(),
                bands: Vec::new(),
            }],
            traces: Vec::new(),
        },
    )
    .unwrap();
    c.call(Command::FileLoad {
        session: SessionRef::Path {
            path: path.to_string_lossy().into_owned(),
        },
    })
    .await
    .unwrap();
    let starts = |r: &[SplLogRow]| r.iter().map(|r| r.start).collect::<Vec<_>>();
    // Paged from the start: every row once, a page at most MAX_ROWS.
    let mut got = Vec::new();
    let mut from = 0;
    loop {
        let p = page(&c, from).await;
        assert!(p.rows.len() <= SplLogPage::MAX_ROWS as usize);
        assert_eq!((p.from, p.total), (from, n));
        from += p.rows.len() as u64;
        got.extend(p.rows);
        if from >= n {
            break;
        }
    }
    assert_eq!(starts(&got), starts(&rows));
    assert_eq!(got[17].laeq, rows[17].laeq);

    // The history of its last 4 h, one point a second, replayed from the rows before too.
    let r = c
        .call(Command::SplHistoryGet {
            meas: M,
            seconds: 100_000,
        })
        .await
        .unwrap();
    let ReplyBody::SplHistory(hist) = r else {
        panic!("{r:?}")
    };
    assert_eq!(hist.scale, LevelScale::DbSpl);
    assert_eq!(hist.windows, vec![window(60.0, Some(95.0))]);
    assert_eq!(hist.at.len(), SplHistory::MAX_SECONDS as usize + 1);
    assert_eq!(
        hist.at.last().unwrap().0,
        rows[rows.len() - 1].start.0 + 1_000_000_000
    );
    // A full minute of -30 … -39 dBFS at 120 dB SPL: about 85 dB, under its 95 dB limit.
    let leq = &hist.leq[0];
    assert!(
        leq.iter().all(|l| (84.0..87.0).contains(l)),
        "{:?}",
        &leq[..5]
    );
    assert!(hist.over[0].iter().all(|o| !o));
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();
}

/// Peak limits and the measuring-position correction (`docs/design/leq.md`, *Peak limits*,
/// *Measuring-position correction*). A calibrated 1 kHz tone at 94 dB SPL has LCpeak 97.0
/// and LAFmax 94.0 dB: near a 99 dB LCpeak limit and a 96 dB LAFmax limit. 6 dB louder the
/// LCpeak limit goes over (an alarm naming LCpeak) and stays over for the 10 s hold after
/// the tone is turned back down, then recovers. A position correction of +5 dB (energy)
/// and +1 dB (peak) moves every level the meter reports, the LAFmax limit goes over on the
/// corrected level, and the log keeps what was measured with the correction beside it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn peak_limits_and_the_position_correction() {
    use ac2_proto::model::{AlarmSubject, PeakLimit, PeakLimits, PeakQuantity, PositionCorrection};
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
    let limit = |l: f64| {
        Some(PeakLimit {
            limit: DbSpl(l),
            warn_margin: Db(3.0),
        })
    };
    let with = |position: Option<PositionCorrection>| {
        let mut m = meter(vec![window(5.0, None)]);
        let MeasKind::Spl { config } = &mut m.kind else {
            unreachable!()
        };
        config.leq.peaks = PeakLimits {
            lcpeak: limit(99.0),
            lafmax: limit(96.0),
        };
        config.position = position;
        m
    };
    c.call(Command::MeasCreate { config: with(None) })
        .await
        .unwrap();
    let l = log_until(&c, "entity", |_| true).await;
    assert_eq!(l.peaks.lcpeak.judgement, LeqJudgement::NotCalibrated);
    // A correction beyond ±30 dB is refused.
    let e = c
        .call(Command::MeasUpdate {
            meas: M,
            config: with(Some(PositionCorrection::both(31.0))),
        })
        .await
        .unwrap_err();
    assert!(matches!(e, ClientError::Daemon(p) if p.code == ErrorCode::Invalid));
    c.call(Command::MeasStart { meas: M }).await.unwrap();
    c.subscribe(Subscription::Meas(M)).unwrap();
    let lease = c.acquire_lease(false, OnDrop::Release).await.unwrap();
    set_level(&lease, -20.0).await;
    leq_until(&c, "the tone at the input", |f| {
        (f.leq[0] + 26.02).abs() < 0.2
    })
    .await;
    calibrate(&c).await;

    let f = leq_until(&c, "peaks judged near", |f| {
        f.meta.scale == LevelScale::DbSpl
            && f.meta
                .lcpeak
                .is_some_and(|p| p.judgement == LeqJudgement::Near)
            && f.meta
                .lafmax
                .is_some_and(|p| p.judgement == LeqJudgement::Near)
    })
    .await;
    let (lc, laf) = (f.meta.lcpeak.unwrap(), f.meta.lafmax.unwrap());
    assert!((lc.level - 97.0).abs() < 0.2, "{lc:?}");
    assert!((laf.level - 94.0).abs() < 0.2, "{laf:?}");
    assert_eq!(f.meta.position, None);

    // 6 dB louder: LCpeak 103 over its 99 dB limit (LAFmax 100 over 96 too).
    set_level(&lease, -14.0).await;
    let l = log_until(&c, "LCpeak over", |l| {
        l.peaks.lcpeak.judgement == LeqJudgement::Over
    })
    .await;
    let a = l
        .alarms
        .iter()
        .find(|a| {
            a.subject
                == AlarmSubject::Peak {
                    quantity: PeakQuantity::LcPeak,
                }
        })
        .copied()
        .expect("an LCpeak alarm");
    assert_eq!(
        (a.kind, a.limit, a.position),
        (LeqAlarmKind::Over, DbSpl(99.0), None)
    );
    assert!((a.level.0 - 103.0).abs() < 0.3, "{a:?}");
    // Back down: over for the hold (the loud seconds are still in it), then recovered.
    set_level(&lease, -20.0).await;
    let down = Instant::now();
    let l = log_until(&c, "LCpeak recovered", |l| {
        l.peaks.lcpeak.judgement == LeqJudgement::Near
    })
    .await;
    let held = down.elapsed().as_secs_f64();
    assert!((8.0..=14.0).contains(&held), "held {held:.1} s");
    assert!(l.alarms.iter().any(|a| a.kind == LeqAlarmKind::Recovered
        && a.subject
            == AlarmSubject::Peak {
                quantity: PeakQuantity::LcPeak
            }));

    // The correction: +5 dB on the energy levels, +1 dB on the peaks.
    let pos = PositionCorrection {
        level: Db(5.0),
        peak: Db(1.0),
    };
    let ReplyBody::Measurement(m) = c
        .call(Command::MeasUpdate {
            meas: M,
            config: with(Some(pos)),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    let s = spl_until(&c, "a corrected spl frame", |_, rev| rev >= m.config_rev).await;
    assert_eq!(s.meta.position, Some(pos));
    assert!((s.meta.level - 99.0).abs() < 0.2, "{:?}", s.meta);
    // The meter's peak since it started: the loud stretch's 103 dB with the peak's +1 dB.
    assert!((s.meta.lpeak - 104.0).abs() < 0.3, "{:?}", s.meta);
    let f = leq_until(&c, "corrected windows and peaks", |f| {
        f.meta.position == Some(pos)
            && f.meta
                .lafmax
                .is_some_and(|p| p.judgement == LeqJudgement::Over)
    })
    .await;
    assert!((f.leq[0] - 99.0).abs() < 0.3, "{:?}", f.leq);
    assert!(
        (f.meta.lcpeak.unwrap().level - 98.0).abs() < 0.2,
        "{:?}",
        f.meta
    );
    assert!(
        (f.meta.lafmax.unwrap().level - 99.0).abs() < 0.2,
        "{:?}",
        f.meta
    );
    let l = log_until(&c, "LAFmax over, corrected", |l| {
        l.alarms.iter().any(|a| {
            a.subject
                == AlarmSubject::Peak {
                    quantity: PeakQuantity::LafMax,
                }
                && a.position == Some(Db(5.0))
        })
    })
    .await;
    assert_eq!(l.peaks.lafmax.judgement, LeqJudgement::Over);
    // The log: what was measured (94 dB SPL), the correction beside it, from the first
    // second logged after the change.
    let deadline = Instant::now() + WAIT;
    let last = loop {
        let last = page(&c, 0).await.rows.last().copied().unwrap();
        if last.position.is_some() {
            break last;
        }
        assert!(Instant::now() < deadline, "no row with the correction");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert_eq!(last.position, Some(pos));
    let sens = last.sensitivity.unwrap().0;
    assert!((last.laeq.0 + sens - 94.0).abs() < 0.2, "{last:?}");
    assert!((last.lcpeak.0 + sens - 97.0).abs() < 0.2, "{last:?}");
    assert!((last.lafmax.0 + sens - 94.0).abs() < 0.2, "{last:?}");
    drop(lease);
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();
}
