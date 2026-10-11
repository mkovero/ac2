//! Leq windows over their limit and back, and a filling window judged on its budget.

use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_windows_go_over_and_recover_and_survive() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = steady_config(realtime_rig(), local_tcp());
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
        overwrite: false,
    })
    .await
    .unwrap();
    c.call(Command::FileLoad {
        session: SessionRef::Path { path },
        discard: true,
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

/// A new log at 94 dB SPL with 90 dB limits on a 4 s and a 30 s window: a filling window is
/// judged on its budget (`docs/design/leq.md`, *Judging a filling window*). 2.5 times the
/// limit's power spends the 4 s budget in 1.6 s and the 30 s one in 12 s: both are on course
/// at once (near, `ON_COURSE`, the time to the budget counting down), the 4 s window goes
/// over after 2 s and the 30 s one only after about 13 s (its least level above 90.0 at
/// 0.1 dB), each an alarm then and not before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_a_filling_window_goes_over_when_its_budget_is_spent() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = steady_config(realtime_rig(), local_tcp());
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
