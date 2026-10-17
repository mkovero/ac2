//! The per-second log: a new log starts over, and a long log in pages with its history.

use super::*;

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
async fn slow_a_new_log_starts_over_and_keeps_the_windows() {
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
    let h = Daemon::start(steady_config(realtime_rig(), local_tcp())).unwrap();
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
                colour: None,
                layout: None,
                compared: Vec::new(),
                offset: ac2_proto::units::Db(0.0),
            }],
            spl_logs: vec![SavedSplLog {
                info: ac2_proto::spl_log::SplLogInfo {
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
        discard: true,
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
            span: HistorySpan::Newest { seconds: 100_000 },
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
    let sec = 1_000_000_000u64;
    let (first, until) = (rows[0].start, WallNs(rows[rows.len() - 1].start.0 + sec));
    assert_eq!((hist.log_start, hist.log_until), (Some(first), Some(until)));

    let range = |from: WallNs, to: WallNs, step_s: u32| {
        let c = &c;
        async move {
            c.call(Command::SplHistoryGet {
                meas: M,
                span: HistorySpan::Range { from, to, step_s },
            })
            .await
            .map(|r| match r {
                ReplyBody::SplHistory(h) => h,
                r => panic!("{r:?}"),
            })
        }
    };
    // The log's first seconds, one by one: the range reaches the first row.
    let start = range(WallNs(first.0 - sec), WallNs(first.0 + 600 * sec), 1)
        .await
        .unwrap();
    assert_eq!(start.at.len(), 600);
    assert_eq!(start.at[0].0, first.0 + sec);
    assert_eq!(
        (start.log_start, start.log_until),
        (Some(first), Some(until))
    );
    // The whole log at a minute a point: the highest second of each bucket.
    let whole = range(WallNs(first.0 - sec), until, 60).await.unwrap();
    assert_eq!(whole.step_s, 60);
    assert!(whole.at.len() >= n as usize / 60, "{}", whole.at.len());
    assert!(whole.at.iter().all(|t| t.0 % (60 * sec) == 0));
    for (k, end) in whole.at.iter().enumerate() {
        let inside: Vec<f32> = hist
            .at
            .iter()
            .zip(&hist.leq[0])
            .filter(|(t, _)| t.0 > end.0 - 60 * sec && t.0 <= end.0)
            .map(|(_, l)| *l)
            .collect();
        // Buckets wholly within the 4 h the newest-seconds history covers.
        if inside.len() == 60 {
            let max = inside.iter().copied().fold(f32::NAN, f32::max);
            assert!((whole.leq[0][k] - max).abs() < 1e-3, "bucket {k}");
        }
    }
    // More points than a reply carries: refused, naming the limit.
    let e = range(first, until, 1).await.unwrap_err();
    assert!(e.to_string().contains("at most 4000"), "{e}");
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();
}
