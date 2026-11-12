//! Typed commands through ac2-client against the real daemon, their results seen in the
//! client's mirror: a delay finding, traces captured, averaged, exported and imported, a
//! session saved and loaded, a band transfer from typed levels. What the daemon computes
//! is tested in its own modules (`transfer`, `traces`, `sessions`, `band_leq`).
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::Duration;

use ac2_client::{Client, ClientError, OnDrop};
use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Command, ErrorCode, ReplyBody};
use ac2d::Daemon;
use common::*;

const T: Duration = Duration::from_secs(30);

async fn trace(c: &Client, cmd: Command) -> TraceMeta {
    match c.call(cmd).await.unwrap() {
        ReplyBody::Trace(t) => t,
        other => panic!("{other:?}"),
    }
}

/// A finding is the reply, and is mirrored with its measurement; an insert with nothing to
/// insert is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delay_find_outcome_and_insert() {
    init_log();
    let h = Daemon::start(config(manual_rig(), local_tcp())).unwrap();
    let c = mirror(&h, "ac2-client-test").await;
    c.call(Command::SessionOpen {
        config: session(true),
    })
    .await
    .unwrap();
    let ReplyBody::Measurement(m) = c
        .call(Command::MeasCreate {
            config: transfer("tf"),
        })
        .await
        .unwrap()
    else {
        panic!("measurement");
    };
    c.call(Command::MeasStart { meas: m.id }).await.unwrap();
    // Nothing captured: no estimate, with its reason.
    let find = Command::DelayFind {
        meas: m.id,
        band: FinderBand::Auto,
        observation: None,
    };
    let want = DelayOutcome::NoEstimate {
        reasons: vec![NoEstimateReason::ObservationTooShort],
    };
    match c.call(find).await.unwrap() {
        ReplyBody::DelayFinding(d) => assert_eq!(d.outcome, want),
        other => panic!("{other:?}"),
    }
    let e = c
        .call(Command::DelayInsert {
            meas: m.id,
            pick: DelayPick::FirstArrival,
        })
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some(ErrorCode::Refused), "{e}");
    mirror_until(&c, Poll::new(T, 20), "finding mirrored", |s| {
        s.measurements
            .iter()
            .find(|x| x.id == m.id)
            .and_then(|x| x.delay.as_ref()?.last_finding.as_ref())
            .filter(|f| f.outcome == want)
            .map(drop)
    })
    .await;
    drop(c);
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();
}

/// Capture, data, average, export, import, save and load (disarmed, newer epoch), each
/// seen in the mirror.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn traces_and_sessions() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let backend = manual_rig();
    let mut cfg = steady_config(backend.clone(), local_tcp());
    cfg.session_dir = dir.path().join("sessions");
    let h = Daemon::start(cfg).unwrap();
    let c = mirror(&h, "ac2-client-test").await;
    c.call(Command::SessionOpen {
        config: session(true),
    })
    .await
    .unwrap();
    c.call(Command::MeasCreate {
        config: transfer("tf"),
    })
    .await
    .unwrap();
    let meas = MeasId(1);
    c.call(Command::MeasStart { meas }).await.unwrap();
    let lease = c.acquire_lease(false, OnDrop::Release).await.unwrap();
    lease
        .set(GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Pink,
                level: Dbfs(-20.0),
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: true,
        })
        .await
        .unwrap();
    let mut d = tokio::task::block_in_place(|| driver(&backend));
    tokio::task::block_in_place(|| run(&mut d, 1.0));
    // A capture needs a result to hold; one is there once a frame was published.
    c.subscribe(ac2_proto::Subscription::Meas(meas)).unwrap();
    tokio::task::block_in_place(|| run(&mut d, 0.5));
    frame_until(
        &c,
        (meas, ac2_proto::Stream::Tf),
        Poll::new(T, 20),
        "tf frame",
        |_| Some(()),
    )
    .await;

    let cap = |name: &str| Command::TraceCapture {
        meas,
        name: name.into(),
        slot: Some(1),
    };
    let a = trace(&c, cap("a")).await;
    let b = trace(&c, cap("b")).await;
    // The slot moved to b, and the mirror shows it.
    mirror_until(&c, Poll::new(T, 20), "slot moved", |s| {
        let slot = |id| s.traces.iter().find(|t| t.id == id).map(|t| t.edit.slot);
        (slot(a.id) == Some(None) && slot(b.id) == Some(Some(1))).then_some(())
    })
    .await;
    let ReplyBody::TraceData(data) = c.call(Command::TraceGet { trace: a.id }).await.unwrap()
    else {
        panic!("trace data");
    };
    let grid = c.grid(a.grid_id).await.unwrap();
    assert_eq!(data.mag_db.len(), grid.len());
    assert!(data.phase_deg.is_some() && data.coherence.is_some());
    let avg = trace(
        &c,
        Command::TraceAverage {
            traces: vec![a.id, b.id],
            method: AverageMethod::Power,
            reference: DelayReference::Trace { trace: a.id },
            name: "avg".into(),
        },
    )
    .await;
    assert!(matches!(avg.source, TraceSource::Average { .. }));
    let ReplyBody::Export { content, .. } = c
        .call(Command::TraceExport {
            trace: a.id,
            format: ExportFormat::Ac2Csv,
        })
        .await
        .unwrap()
    else {
        panic!("export");
    };
    let imp = trace(
        &c,
        Command::TraceImport {
            file_name: "a.csv".into(),
            format: ImportFormat::Auto,
            role: ImportRole::Trace,
            ir: None,
            content,
            owner: TraceOwner::Imported,
        },
    )
    .await;
    // The export re-imports on the capture's own grid.
    assert_eq!(imp.grid_id, a.grid_id);
    let bad = c
        .call(Command::TraceImport {
            file_name: "x.txt".into(),
            format: ImportFormat::AnalyzerText,
            role: ImportRole::Trace,
            ir: None,
            content: Blob(b"1 2\n".to_vec()),
            owner: TraceOwner::Imported,
        })
        .await;
    assert_eq!(bad.err().and_then(|e| e.code()), Some(ErrorCode::Invalid));

    // A reply can arrive before the events that put its trace in the mirror: the state
    // compared after the load must hold every trace the save writes.
    let ids = [a.id, b.id, avg.id, imp.id];
    let saved = mirror_until(&c, Poll::new(T, 20), "every trace mirrored", |s| {
        ids.iter()
            .all(|id| s.traces.iter().any(|t| t.id == *id))
            .then(|| s.clone())
    })
    .await;
    lease.end().await.unwrap();
    c.call(Command::FileSave {
        session: SessionRef::Name { name: "s1".into() },
        overwrite: false,
    })
    .await
    .unwrap();
    c.call(Command::TraceDelete { trace: a.id }).await.unwrap();
    match c.call(Command::FileList).await.unwrap() {
        ReplyBody::Sessions(l) => assert_eq!(l[0].traces, 4, "{l:?}"),
        other => panic!("{other:?}"),
    }
    c.call(Command::FileLoad {
        session: SessionRef::Name { name: "s1".into() },
        discard: true,
    })
    .await
    .unwrap();
    mirror_until(&c, Poll::new(T, 20), "loaded state mirrored", |s| {
        (s.traces == saved.traces
            && s.session.epoch > saved.session.epoch
            && s.generator.owner.is_none()
            && !s.generator.armed)
            .then_some(())
    })
    .await;
    drop(c);
    tokio::task::spawn_blocking(move || shutdown_driving(h, Some(d)))
        .await
        .unwrap();
}

/// `spl.band_transfer`: typed levels make the transfer and store it in the band meter (the
/// reply is the updated meter); a meter without a band meter and a band-log span with no
/// log behind it are refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn band_transfer_from_typed_levels() {
    init_log();
    let h = Daemon::start(config(manual_rig(), local_tcp())).unwrap();
    let c = mirror(&h, "ac2-client-test").await;
    let ReplyBody::Measurement(m) = c
        .call(Command::MeasCreate {
            config: spl("FOH SPL", 0),
        })
        .await
        .unwrap()
    else {
        panic!("measurement");
    };
    let levels = |db: f64| BandLevelSource::Levels {
        levels: (0..BAND_COUNT)
            .map(|i| (i < LF_BAND_COUNT).then_some(DbSpl(db)))
            .collect(),
    };
    let transfer = |background: Option<BandLevelSource>| Command::SplBandTransfer {
        meas: m.id,
        foh: levels(80.0),
        at_place: levels(50.0),
        background,
        place: "flat 4".into(),
    };
    match c.call(transfer(None)).await {
        Err(ClientError::Daemon(p)) => {
            assert_eq!(p.code, ErrorCode::Invalid);
            assert!(p.msg.contains("no band meter"), "{}", p.msg);
        }
        other => panic!("{other:?}"),
    }
    let MeasKind::Spl { config } = &m.config.kind else {
        unreachable!()
    };
    let with_bands = MeasConfig {
        kind: MeasKind::Spl {
            config: SplConfig {
                bands: Some(Box::new(BandLeqPreset::Finland545Lf.apply(None))),
                ..config.clone()
            },
        },
        ..m.config.clone()
    };
    c.call(Command::MeasUpdate {
        meas: m.id,
        base: m.config_rev,
        config: with_bands,
    })
    .await
    .unwrap();
    let ReplyBody::Measurement(m2) = c.call(transfer(Some(levels(45.0)))).await.unwrap() else {
        panic!("measurement");
    };
    let MeasKind::Spl { config } = &m2.config.kind else {
        unreachable!()
    };
    let t = config
        .bands
        .as_ref()
        .and_then(|b| b.transfer.clone())
        .expect("stored");
    assert_eq!(t.place, "flat 4");
    // 50 dB over a 45 dB background: 5 dB, its energy subtracted first.
    assert!(
        matches!(t.bands[5], BandTransferBand::Corrected { .. }),
        "{:?}",
        t.bands[5]
    );
    assert_eq!(t.bands[11], BandTransferBand::Missing);
    let span = Command::SplBandTransfer {
        meas: m.id,
        foh: BandLevelSource::Log {
            meas: m.id,
            from: WallNs(0),
            until: WallNs(1),
        },
        at_place: levels(50.0),
        background: None,
        place: "flat 4".into(),
    };
    assert!(matches!(c.call(span).await, Err(ClientError::Daemon(_))));
    drop(c);
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();
}
