//! Levels on a running meter: weightings changed in place, peak limits and the
//! measuring-position correction.

use super::*;

/// Weightings change on a running meter in place: the next frames read the new weighting
/// at once at its settled level (Slow too: it ran all along), over the same interval; the
/// Leq windows, their states and the per-second log carry on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_weightings_change_in_place_and_keep_the_log() {
    init_log();
    let h = Daemon::start(steady_config(realtime_rig(), local_tcp())).unwrap();
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

/// Peak limits and the measuring-position correction (`docs/design/leq.md`, *Peak limits*,
/// *Measuring-position correction*). A calibrated 1 kHz tone at 94 dB SPL has LCpeak 97.0
/// and LAFmax 94.0 dB: near a 99 dB LCpeak limit and a 96 dB LAFmax limit. 6 dB louder the
/// LCpeak limit goes over (an alarm naming LCpeak) and stays over for the 10 s hold after
/// the tone is turned back down, then recovers. A position correction of +5 dB (energy)
/// and +1 dB (peak) moves every level the meter reports, the LAFmax limit goes over on the
/// corrected level, and the log keeps what was measured with the correction beside it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_peak_limits_and_the_position_correction() {
    use ac2_proto::model::{AlarmSubject, PeakLimit, PeakLimits, PeakQuantity, PositionCorrection};
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
