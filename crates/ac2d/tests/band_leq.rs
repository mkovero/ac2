//! The band meter end to end on the fake rig (`docs/design/band-leq.md`), from an empty
//! daemon: the STM 545/2015 low-frequency preset judged on a calibrated 63 Hz tone by a
//! bedroom monitor and not judged at FOH without a transfer, the day/night flip at 22:00
//! local time on an injected clock, the band log kept through a stopped meter and a saved
//! session, a FOH → dwelling transfer from typed levels and from a span of the log, and the
//! bedroom spans from a recorder's WAV replayed.
#![allow(clippy::unwrap_used)]

mod common;

use std::time::{Duration, Instant};

use ac2_client::{Client, ClientConfig, ClientError, Endpoints, OnDrop, StimulusLease};
use ac2_proto::frame::{BandLeqMeta, FrameData};
use ac2_proto::model::{
    AlarmSubject, BAND_COUNT, BandLeqConfig, BandLeqPreset, BandLevelSource, BandLimitPlace,
    BandMicPlace, BandPeriod, BandTransferBand, BandTransferSet, GeneratorDesired,
    GeneratorSettings, LeqAlarmKind, LeqConfig, LeqJudgement, LevelScale, MeasConfig, MeasKind,
    PeakWeighting, SessionRef, Signal, SplConfig, SplLog, TimeWeighting, Weighting,
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
    // The client refreshes the lease every 0.5 s, but a slow runner can stall the refresher
    // past the 1.5 s expiry while the DSP catches up; the lease is not under test here.
    cfg.lease_expiry = Duration::from_secs(60);
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
    // First a bedroom monitor: the mic in the dwelling, its limits judged as they are.
    c.call(Command::MeasCreate {
        config: meter(Some(BandLeqConfig {
            mic: BandMicPlace::Dwelling,
            ..bands()
        })),
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
    assert_eq!(
        f.limits_from,
        BandLimitPlace::AtMic,
        "the mic is in the dwelling"
    );
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

    // The mic at FOH without a transfer: the bedroom's limits shown, not judged.
    c.call(Command::MeasUpdate {
        meas: M,
        config: meter(Some(bands())),
    })
    .await
    .unwrap();
    let f = band_until(&c, "limits not judged at FOH", |f| {
        f.limits_from == BandLimitPlace::NoTransfer
    })
    .await;
    assert_eq!(f.bands[B63].limit, Some(47.0));
    assert!(
        f.bands.iter().all(|b| b.judgement == LeqJudgement::NoLimit),
        "{:?}",
        f.bands
    );
    assert_eq!(f.worst, None);

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

/// `spl.band_log_get` from an empty daemon: the seconds logged read back, uncalibrated
/// ones in dBFS without an average, calibrated ones in dB SPL; the span's average is what
/// `spl.band_transfer` takes from the same span.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_band_log_span_reads_back_and_averages_as_the_transfer_does() {
    let (h, c, _dir) = start(clock_at(12 * 3600)).await;
    c.call(Command::MeasCreate {
        config: meter(Some(bands())),
    })
    .await
    .unwrap();
    let t0 = WallNs(wall_now_ns());
    c.call(Command::MeasStart { meas: M }).await.unwrap();
    c.subscribe(Subscription::Meas(M)).unwrap();
    band_until(&c, "two seconds", |f| f.elapsed.0 >= 2.0).await;
    let get = |from: WallNs, until: WallNs, step: Option<u32>| {
        let c = &c;
        async move {
            match c
                .call(Command::SplBandLogGet {
                    meas: M,
                    from,
                    until,
                    step,
                })
                .await
            {
                Ok(ReplyBody::SplBandLog(l)) => Ok(*l),
                Ok(other) => panic!("{other:?}"),
                Err(e) => Err(e),
            }
        }
    };
    let l = get(t0, WallNs(wall_now_ns()), Some(1)).await.unwrap();
    assert!(l.average.seconds >= 1, "{l:?}");
    assert_eq!(l.rows.len() as u32, l.average.seconds);
    assert_eq!(l.average.uncalibrated, l.average.seconds);
    assert_eq!(
        l.average.levels, None,
        "no dB SPL average while uncalibrated"
    );
    assert!(l.rows.iter().all(|r| r.sensitivity.is_none()));
    assert!(l.rows.windows(2).all(|w| w[0].start < w[1].start));

    let lease = c.acquire_lease(false, OnDrop::Release).await.unwrap();
    calibrate(&c, &lease).await;
    let level = 70.0;
    lease.set(tone(63.0, level - TO_SPL)).await.unwrap();
    band_until(&c, "the tone in its band", |f| {
        f.scale == LevelScale::DbSpl && (f.bands[B63].leq - level).abs() < 0.5
    })
    .await;
    let from = WallNs(wall_now_ns());
    tokio::time::sleep(Duration::from_secs(3)).await;
    let until = WallNs(wall_now_ns() - 1_000_000_000);
    let l = get(from, until, Some(1)).await.unwrap();
    assert!(l.average.seconds >= 1, "{l:?}");
    assert_eq!(l.average.uncalibrated, 0);
    assert_eq!(l.rows.len() as u32, l.average.seconds);
    let avg = l.average.levels.unwrap();
    assert!((avg[B63].unwrap().0 - level).abs() < 0.5, "{avg:?}");
    for r in &l.rows {
        assert!(r.sensitivity.is_some());
        assert!((r.levels[B63].unwrap() - level).abs() < 0.5, "{r:?}");
        assert!(r.start >= from && r.start < until);
    }
    // The average alone, the same.
    let alone = get(from, until, None).await.unwrap();
    assert!(alone.rows.is_empty());
    assert_eq!(alone.average, l.average);

    // The transfer over the same span against a dwelling of 30 dB in every band: each
    // attenuation is the span's average less 30.
    lease.end().await.unwrap();
    c.call(Command::SplBandTransfer {
        meas: M,
        foh: BandLevelSource::Log {
            meas: M,
            from,
            until,
        },
        dwelling: BandLevelSource::Levels {
            levels: vec![Some(DbSpl(30.0)); BAND_COUNT],
        },
        background: None,
    })
    .await
    .unwrap();
    let t = transfer_until(&c, "the transfer", |b| {
        matches!(b, BandTransferBand::Unchecked { .. })
    })
    .await;
    for (i, (b, a)) in t.bands.iter().zip(avg).enumerate() {
        if let Some(a) = a {
            let BandTransferBand::Unchecked { attenuation } = b else {
                panic!("band {i}: {b:?}");
            };
            assert!((attenuation.0 - (a.0 - 30.0)).abs() < 1e-9, "band {i}");
        }
    }

    // Refusals: a step of 0, a backwards span, not an SPL meter.
    for (from, until, step) in [(from, until, Some(0)), (until, from, None)] {
        let e = get(from, until, step).await.unwrap_err();
        assert!(matches!(e, ClientError::Daemon(p) if p.code == ErrorCode::Invalid));
    }
    let e = c
        .call(Command::SplBandLogGet {
            meas: MeasId(99),
            from,
            until,
            step: None,
        })
        .await
        .unwrap_err();
    assert!(
        matches!(&e, ClientError::Daemon(p) if p.code == ErrorCode::NotFound),
        "{e:?}"
    );
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

/// A recorder's 16-bit mono WAV at 48 kHz, written here: a 94 dB calibrator tone, then
/// pink-ish test signal (a sine at each limited band's centre, the same level in every band
/// as pink noise has), then the background with the system silent.
fn recorder_wav(path: &std::path::Path, cal_s: f64, signal_s: f64, background_s: f64) {
    const FS: f64 = 48_000.0;
    // 94 dB SPL ↔ amplitude 0.1: the signal's bands at 0.01 (74 dB), the background's at
    // 0.0001 (34 dB).
    let n = ((cal_s + signal_s + background_s) * FS) as usize;
    let mut data = Vec::with_capacity(n * 2);
    for i in 0..n {
        let t = i as f64 / FS;
        let bands = |a: f64| -> f64 {
            ac2_proto::model::BAND_NOMINAL_HZ[..ac2_proto::model::LF_BAND_COUNT]
                .iter()
                .enumerate()
                .map(|(k, hz)| a * (std::f64::consts::TAU * hz * t + k as f64).sin())
                .sum()
        };
        let x = if t < cal_s {
            0.1 * (std::f64::consts::TAU * 1000.0 * t).sin()
        } else if t < cal_s + signal_s {
            bands(0.01)
        } else {
            bands(0.0001)
        };
        data.extend_from_slice(&((x * 32_767.0).round() as i16).to_le_bytes());
    }
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    for v in [1u16, 1] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    for v in [48_000u32, 96_000] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    for v in [2u16, 16] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    std::fs::write(path, b).unwrap();
}

/// The bedroom without a cable: a recorder's WAV (calibrator tone, the test signal, the
/// silent system) imported and replayed in real time, its meter calibrated from the
/// recorded tone, and spans of the replay's band log — file second `t` at the replay's
/// start + `t` — giving the bedroom and background of a transfer whose FOH levels were
/// typed (measured on the rig at FOH under the same signal).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_recorders_wav_replayed_gives_the_bedroom_spans_of_a_transfer() {
    use ac2_proto::model::{RecordingRef, ReplayPace};
    let (h, c, dir) = start(clock_at(12 * 3600)).await;
    let (cal_s, signal_s, background_s) = (8.0, 8.0, 6.0);
    let src = dir.path().join("ZOOM0001.WAV");
    recorder_wav(&src, cal_s, signal_s, background_s);
    ac2_traces::raw::import_wav(&src, dir.path(), "bedroom", wall_now_ns()).unwrap();

    let t0 = wall_now_ns();
    match c
        .call(Command::SessionReplay {
            recording: RecordingRef::Path {
                path: ac2_traces::raw::sidecar_path(dir.path(), "bedroom")
                    .to_string_lossy()
                    .into_owned(),
            },
            pace: ReplayPace::Realtime,
        })
        .await
        .unwrap()
    {
        ReplyBody::Session(_) => {}
        other => panic!("{other:?}"),
    }
    let mut cfg = meter(Some(bands()));
    cfg.name = "Recorder".into();
    if let MeasKind::Spl { config } = &mut cfg.kind {
        config.input = 0;
    }
    c.call(Command::MeasCreate { config: cfg }).await.unwrap();
    c.call(Command::MeasStart { meas: M }).await.unwrap();
    c.subscribe(Subscription::Meas(M)).unwrap();

    // The calibrator tone in the file calibrates the replayed input.
    let deadline = Instant::now() + Duration::from_secs_f64(cal_s);
    loop {
        match c
            .call(Command::CalSpl {
                input: 0,
                mic: "recorder".into(),
                calibrator_level: DbSpl(94.0),
                calibrator_freq: Hz(1000.0),
            })
            .await
        {
            Ok(ReplyBody::Calibration(_)) => break,
            Err(ClientError::Daemon(p))
                if p.msg.contains("not steady") || p.msg.contains("no calibrator") => {}
            other => panic!("{other:?}"),
        }
        assert!(
            Instant::now() < deadline,
            "the recorded tone never calibrated"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    let s = 1_000_000_000u64;
    let at = |file_s: f64| WallNs(t0 + (file_s * 1e9) as u64);
    let bedroom = (at(cal_s + 1.5), at(cal_s + signal_s - 1.5));
    let background = (
        at(cal_s + signal_s + 1.5),
        at(cal_s + signal_s + background_s - 1.5),
    );
    while wall_now_ns() < background.1.0 + 2 * s {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let span = |(from, until): (WallNs, WallNs)| BandLevelSource::Log {
        meas: M,
        from,
        until,
    };
    match c
        .call(Command::SplBandLogGet {
            meas: M,
            from: bedroom.0,
            until: bedroom.1,
            step: None,
        })
        .await
        .unwrap()
    {
        ReplyBody::SplBandLog(l) => {
            assert_eq!(l.average.uncalibrated, 0, "{l:?}");
            let avg = l.average.levels.unwrap();
            for (i, a) in avg[..ac2_proto::model::LF_BAND_COUNT].iter().enumerate() {
                let a = a.unwrap().0;
                assert!((a - 74.0).abs() < 1.5, "band {i}: {a} dB, not 74");
            }
        }
        other => panic!("{other:?}"),
    }
    let foh = BandLevelSource::Levels {
        levels: vec![Some(DbSpl(100.0)); BAND_COUNT],
    };
    let r = c
        .call(Command::SplBandTransfer {
            meas: M,
            foh,
            dwelling: span(bedroom),
            background: Some(span(background)),
        })
        .await
        .unwrap();
    let ReplyBody::Measurement(m) = r else {
        panic!("{r:?}");
    };
    let MeasKind::Spl { config } = &m.config.kind else {
        panic!("{m:?}");
    };
    let set = config.bands.as_ref().unwrap().transfer.unwrap();
    for (i, b) in set.bands[..ac2_proto::model::LF_BAND_COUNT]
        .iter()
        .enumerate()
    {
        match b {
            BandTransferBand::Clean { attenuation } => {
                assert!((attenuation.0 - 26.0).abs() < 1.5, "band {i}: {b:?}");
            }
            other => panic!("band {i}: {other:?}"),
        }
    }

    // One recorder cannot be in the bedroom with the signal and the silence at once.
    let e = c
        .call(Command::SplBandTransfer {
            meas: M,
            foh: BandLevelSource::Levels {
                levels: vec![Some(DbSpl(100.0)); BAND_COUNT],
            },
            dwelling: span(bedroom),
            background: Some(span((bedroom.0, background.1))),
        })
        .await
        .unwrap_err();
    assert!(
        matches!(&e, ClientError::Daemon(p) if p.msg.contains("bedroom and background spans overlap on Recorder")),
        "{e:?}"
    );
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();
}
