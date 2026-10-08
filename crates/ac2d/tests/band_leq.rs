//! The band meter end to end on the fake rig (`docs/design/band-leq.md`), from an empty
//! daemon: the STM 545/2015 low-frequency preset judged on a calibrated 63 Hz tone, the
//! day/night flip at 22:00 local time on an injected clock, the band log kept through a
//! stopped meter and a saved session, and a FOH → dwelling transfer from typed levels and
//! from a span of the log.
#![allow(clippy::unwrap_used)]

mod common;

use std::time::{Duration, Instant};

use ac2_client::{Client, ClientConfig, ClientError, Endpoints, OnDrop, StimulusLease};
use ac2_proto::frame::{BandLeqMeta, FrameData};
use ac2_proto::model::{
    AlarmSubject, BAND_COUNT, BandLeqConfig, BandLeqPreset, BandLevelSource, BandLimitPlace,
    BandPeriod, BandTransferBand, BandTransferSet, GeneratorDesired, GeneratorSettings,
    LeqAlarmKind, LeqConfig, LeqJudgement, LevelScale, MeasConfig, MeasKind, PeakWeighting,
    SessionRef, Signal, SplConfig, SplLog, TimeWeighting, Weighting,
};
use ac2_proto::units::{DbSpl, Dbfs, Hz, MeasId, Seconds, WallNs};
use ac2_proto::{Command, ErrorCode, ReplyBody, Stream, Subscription, Topic};
use ac2d::{Daemon, Handle, LocalClock};
use common::*;

const M: MeasId = MeasId(1);
/// Index of the 63 Hz band.
const B63: usize = 5;
const WAIT: Duration = Duration::from_secs(40);

async fn connect(h: &Handle) -> Client {
    let ep = Endpoints {
        ctrl: h.ctrl_endpoint().to_owned(),
        data: h.data_endpoint().to_owned(),
    };
    let c = Client::connect(ClientConfig::new(ep, "ac2d-band-leq-test"))
        .await
        .unwrap();
    c.wait_synced(Duration::from_secs(5)).await.unwrap();
    c
}

/// A clock whose local time of day is `at_s` (s after midnight) now.
fn clock_at(at_s: u32) -> LocalClock {
    let now = (wall_now_ns() / 1_000_000_000 % 86_400) as i64;
    let east = (i64::from(at_s) - now).rem_euclid(86_400);
    LocalClock::FixedOffset {
        east_s: east as i32,
    }
}

fn tone(freq: f64, level: f64) -> GeneratorDesired {
    GeneratorDesired {
        settings: GeneratorSettings {
            signal: Signal::Sine { freq: Hz(freq) },
            level: Dbfs(level),
            band: None,
            outputs: vec![0],
        },
        armed: true,
        firing: true,
    }
}

/// The STM 545/2015 low-frequency preset on 5 s windows.
fn bands() -> BandLeqConfig {
    BandLeqConfig {
        duration: Seconds(5.0),
        ..BandLeqPreset::Finland545Lf.config(None)
    }
}

fn meter(bands: Option<BandLeqConfig>) -> MeasConfig {
    MeasConfig {
        name: "FOH SPL".into(),
        kind: MeasKind::Spl {
            config: SplConfig {
                input: 1,
                weighting: Weighting::A,
                time_weighting: TimeWeighting::Fast,
                peak_weighting: PeakWeighting::C,
                leq: LeqConfig {
                    windows: Vec::new(),
                    horizon: Seconds(2.0),
                    peaks: Default::default(),
                },
                position: None,
                bands: bands.map(Box::new),
            },
        },
    }
}

async fn start(clock: LocalClock) -> (Handle, Client, tempfile::TempDir) {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(realtime_rig(), local_tcp());
    cfg.cal_store = Some(dir.path().join("calibrations.json"));
    cfg.local_clock = clock;
    let h = Daemon::start(cfg).unwrap();
    let c = connect(&h).await;
    c.call(Command::SessionOpen {
        config: session(false),
    })
    .await
    .unwrap();
    (h, c, dir)
}

/// The newest `band_leq` frame once `ok` holds.
async fn band_until(c: &Client, what: &str, ok: impl Fn(&BandLeqMeta) -> bool) -> BandLeqMeta {
    let topic = Topic::Data {
        meas: M,
        stream: Stream::BandLeq,
    };
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(f) = c.latest().unwrap().get(&topic)
            && let FrameData::BandLeq(b) = &f.frame.data
            && ok(&b.meta)
        {
            return b.meta.clone();
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

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

async fn calibrate(c: &Client, lease: &StimulusLease) {
    lease.set(tone(1000.0, -20.0)).await.unwrap();
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
            // The tone takes a moment to reach the input and settle.
            Err(ClientError::Daemon(p))
                if p.msg.contains("not steady") || p.msg.contains("no calibrator") => {}
            other => panic!("{other:?}"),
        }
        assert!(Instant::now() < deadline, "calibrator never steady");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

/// The meter's stored transfer band at 63 Hz once `ok` holds.
async fn transfer_until(
    c: &Client,
    what: &str,
    ok: impl Fn(&BandTransferBand) -> bool,
) -> BandTransferSet {
    let deadline = Instant::now() + WAIT;
    loop {
        let s = c.view().state.clone().unwrap();
        let m = s.measurements.iter().find(|m| m.id == M).unwrap();
        if let MeasKind::Spl { config } = &m.config.kind
            && let Some(t) = config.bands.as_ref().and_then(|b| b.transfer)
            && ok(&t.bands[B63])
        {
            return t;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The calibrated tone reaches input 1 at `level` − 6.02 dBFS, and the 1 kHz calibrator
/// at −26.02 dBFS reads 94 dB: dB SPL = `level` + 113.96.
const TO_SPL: f64 = 113.96;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_63_hz_tone_fails_only_its_band_and_the_log_and_transfer_carry_on() {
    // Midday local time: the day limits (night + 5 dB) for the whole test.
    let (h, c, dir) = start(clock_at(12 * 3600)).await;
    // A window that is no whole number of seconds is refused.
    let e = c
        .call(Command::MeasCreate {
            config: meter(Some(BandLeqConfig {
                duration: Seconds(2.5),
                ..bands()
            })),
        })
        .await
        .unwrap_err();
    assert!(matches!(e, ClientError::Daemon(p) if p.code == ErrorCode::Invalid));
    c.call(Command::MeasCreate {
        config: meter(Some(bands())),
    })
    .await
    .unwrap();
    c.call(Command::MeasStart { meas: M }).await.unwrap();
    c.subscribe(Subscription::Meas(M)).unwrap();

    // Uncalibrated: dBFS, the limits shown but not judged.
    let f = band_until(&c, "uncalibrated frame", |f| f.elapsed.0 >= 1.0).await;
    assert_eq!(f.scale, LevelScale::Dbfs);
    assert_eq!(f.bands.len(), 11);
    assert_eq!(f.period, BandPeriod::Day);
    assert_eq!(f.limits_from, BandLimitPlace::AtMic);
    assert_eq!(f.bands[B63].nominal, Hz(63.0));
    assert_eq!(f.bands[B63].limit, Some(47.0));
    assert_eq!(f.bands[B63].judgement, LeqJudgement::NotCalibrated);
    assert!(f.predicted.is_none());

    let lease = c.acquire_lease(false, OnDrop::Release).await.unwrap();
    calibrate(&c, &lease).await;
    // 63 Hz at 52 dB SPL: 5 dB over the day limit of its band, under its neighbours'.
    let level = 52.0;
    let tone_from = WallNs(wall_now_ns());
    lease.set(tone(63.0, level - TO_SPL)).await.unwrap();
    let f = band_until(&c, "63 Hz over", |f| {
        f.bands[B63].judgement == LeqJudgement::Over && (f.bands[B63].leq - level).abs() < 0.5
    })
    .await;
    assert_eq!(f.scale, LevelScale::DbSpl);
    assert!((f.bands[B63].leq - level).abs() < 0.5, "{:?}", f.bands[B63]);
    for (i, b) in f.bands.iter().enumerate() {
        if i != B63 {
            assert_ne!(b.judgement, LeqJudgement::Over, "band {i}: {b:?}");
        }
    }
    assert_eq!(f.worst, Some(B63 as u8));
    let l = log_until(&c, "63 Hz alarm", |l| !l.alarms.is_empty()).await;
    let a = l.alarms.last().unwrap();
    assert_eq!(a.subject, AlarmSubject::Band { nominal: Hz(63.0) });
    assert_eq!(a.kind, LeqAlarmKind::Over);
    assert_eq!(a.limit, DbSpl(47.0));
    assert!(
        l.alarms
            .iter()
            .all(|a| a.subject == AlarmSubject::Band { nominal: Hz(63.0) }),
        "{:?}",
        l.alarms
    );
    // A few seconds more of the tone; the meter stops, the tone ends, the meter starts
    // again: the windows come back from the band log with the tone seconds and the gap in
    // them (a window started afresh would hold silence only).
    tokio::time::sleep(Duration::from_secs(2)).await;
    let tone_until = WallNs(wall_now_ns() - 1_000_000_000);
    c.call(Command::MeasStop { meas: M }).await.unwrap();
    lease.end().await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    c.call(Command::MeasStart { meas: M }).await.unwrap();
    let f = band_until(&c, "rebuilt windows", |f| {
        f.measured.0 < f.elapsed.0 - 0.5 && f.bands[B63].leq > 45.0
    })
    .await;
    assert_eq!(f.elapsed.0, 5.0);
    assert_eq!(f.bands[B63].judgement, LeqJudgement::Over);

    // Saved: the band log is written next to the SPL log and reads back in dB SPL.
    let path = dir.path().join("show");
    c.call(Command::FileSave {
        session: SessionRef::Path {
            path: path.to_string_lossy().into_owned(),
        },
    })
    .await
    .unwrap();
    let s = ac2_traces::session::load(&path).unwrap();
    let rows = &s.spl_logs[0].bands;
    let toned: Vec<_> = rows
        .iter()
        .filter(|r| r.start >= tone_from && r.start < tone_until)
        .filter(|r| r.measured.0 == 1.0)
        .filter_map(|r| Some(f64::from(r.levels[B63]) + r.sensitivity?.0))
        .collect();
    assert!(toned.len() >= 3, "{rows:?}");
    assert!(
        toned.iter().any(|l| (l - level).abs() < 0.3),
        "logged 63 Hz levels {toned:?}"
    );
    assert!(rows.iter().all(|r| r.period == BandPeriod::Day));

    // The transfer from typed levels, as read from a text file: FOH 100 dB in every band,
    // the dwelling 60 dB at 63 Hz over a 40 dB background, nothing else measured.
    let foh = [Some(100.0); BAND_COUNT];
    let mut dwelling = [None; BAND_COUNT];
    let mut background = [None; BAND_COUNT];
    dwelling[B63] = Some(60.0);
    background[B63] = Some(40.0);
    let read = |l: &[Option<f64>; BAND_COUNT]| {
        let text = ac2_traces::band_levels::format("test", l);
        let back = ac2_traces::band_levels::parse(&text).unwrap();
        assert_eq!(&back, l);
        BandLevelSource::Levels {
            levels: back.iter().map(|v| v.map(DbSpl)).collect(),
        }
    };
    c.call(Command::SplBandTransfer {
        meas: M,
        foh: read(&foh),
        dwelling: read(&dwelling),
        background: Some(read(&background)),
    })
    .await
    .unwrap();
    let t = transfer_until(&c, "the typed transfer", |b| {
        matches!(b, BandTransferBand::Clean { attenuation } if (attenuation.0 - 40.0).abs() < 1e-9)
    })
    .await;
    assert_eq!(t.bands[0], BandTransferBand::Missing);
    // Judged at FOH now: the dwelling's 47 dB plus 40 dB attenuation, so the window that
    // held the tone is well under, and the dwelling LAeq is predicted.
    let f = band_until(&c, "transferred limits", |f| {
        f.limits_from == BandLimitPlace::Transferred
    })
    .await;
    assert_eq!(f.bands[B63].limit, Some(87.0));
    assert_eq!(f.bands[0].limit, None);
    assert_ne!(f.bands[B63].judgement, LeqJudgement::Over);
    assert!(f.predicted.is_some());

    // From a span of the band log: the tone's seconds as FOH, the dwelling 32 dB at 63 Hz.
    let mut dwelling = [None; BAND_COUNT];
    dwelling[B63] = Some(DbSpl(32.0));
    c.call(Command::SplBandTransfer {
        meas: M,
        foh: BandLevelSource::Log {
            meas: M,
            from: WallNs(tone_from.0 + 2_000_000_000),
            until: tone_until,
        },
        dwelling: BandLevelSource::Levels {
            levels: dwelling.to_vec(),
        },
        background: None,
    })
    .await
    .unwrap();
    transfer_until(&c, "the transfer from the log", |b| {
        matches!(b, BandTransferBand::Unchecked { attenuation } if (attenuation.0 - (level - 32.0)).abs() < 0.5)
    })
    .await;

    // Refusals: levels of the wrong length, a span with nothing logged, no band meter.
    let e = c
        .call(Command::SplBandTransfer {
            meas: M,
            foh: BandLevelSource::Levels {
                levels: vec![Some(DbSpl(90.0))],
            },
            dwelling: BandLevelSource::Levels {
                levels: dwelling.to_vec(),
            },
            background: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(e, ClientError::Daemon(p) if p.code == ErrorCode::Invalid));
    let e = c
        .call(Command::SplBandTransfer {
            meas: M,
            foh: BandLevelSource::Log {
                meas: M,
                from: WallNs(1_000_000_000),
                until: WallNs(2_000_000_000),
            },
            dwelling: BandLevelSource::Levels {
                levels: dwelling.to_vec(),
            },
            background: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(e, ClientError::Daemon(p) if p.code == ErrorCode::Invalid));
    c.call(Command::MeasUpdate {
        meas: M,
        config: meter(None),
    })
    .await
    .unwrap();
    let e = c
        .call(Command::SplBandTransfer {
            meas: M,
            foh: BandLevelSource::Levels {
                levels: dwelling.to_vec(),
            },
            dwelling: BandLevelSource::Levels {
                levels: dwelling.to_vec(),
            },
            background: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(e, ClientError::Daemon(p) if p.code == ErrorCode::Invalid));
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_night_limits_take_over_at_22() {
    // Local time a few seconds before 22:00.
    let (h, c, dir) = start(clock_at(22 * 3600 - 8)).await;
    c.call(Command::MeasCreate {
        config: meter(Some(bands())),
    })
    .await
    .unwrap();
    c.call(Command::MeasStart { meas: M }).await.unwrap();
    c.subscribe(Subscription::Meas(M)).unwrap();
    let f = band_until(&c, "a day frame", |f| f.elapsed.0 >= 1.0).await;
    assert_eq!(f.period, BandPeriod::Day);
    assert_eq!(f.bands[B63].limit, Some(47.0));
    // The headroom looks past the horizon (2 s) before the windows turn.
    let f = band_until(&c, "night ahead", |f| {
        f.period_after_horizon == BandPeriod::Night
    })
    .await;
    assert_eq!(f.period, BandPeriod::Day);
    let f = band_until(&c, "night", |f| f.period == BandPeriod::Night).await;
    assert_eq!(f.bands[B63].limit, Some(42.0));
    assert_eq!(f.bands[0].limit, Some(74.0));
    // The log holds the seconds of both periods.
    let path = dir.path().join("late");
    c.call(Command::FileSave {
        session: SessionRef::Path {
            path: path.to_string_lossy().into_owned(),
        },
    })
    .await
    .unwrap();
    let s = ac2_traces::session::load(&path).unwrap();
    let rows = &s.spl_logs[0].bands;
    assert!(rows.iter().any(|r| r.period == BandPeriod::Day));
    assert!(rows.iter().any(|r| r.period == BandPeriod::Night));
    let first_night = rows
        .iter()
        .position(|r| r.period == BandPeriod::Night)
        .unwrap();
    assert!(
        rows[first_night..]
            .iter()
            .all(|r| r.period == BandPeriod::Night)
    );
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();
}
