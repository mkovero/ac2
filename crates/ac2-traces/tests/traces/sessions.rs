//! Session directory round trips, autosave and refusals.

use super::*;

pub(super) fn session_sample() -> Session {
    let mut a = delayed(4, 2, 0.0125, 0.0125, 0.95);
    a.meta.edit.slot = Some(1);
    a.columns.mag_db[200] = 9.0;
    a.meta.edit.smoothing = Some(Smoothing {
        fraction: SmoothingFraction::Third,
        mode: SmoothingMode::MagnitudePhase,
    });
    a.meta.edit.visible = false;
    let b = delayed(9, 2, 0.0100, 0.0100, 0.9);
    Session {
        saved_at: WallNs(1_790_000_000_123_456_789),
        measurements: vec![
            SavedMeasurement {
                id: MeasId(1),
                config: MeasConfig {
                    scope: ac2_proto::model::MeasScope::Session,
                    name: "main".into(),
                    kind: MeasKind::Transfer {
                        config: TransferConfig {
                            reference_input: 0,
                            measurement_input: 1,
                            averaging: TfAveraging::Fifo { blocks: 8 },
                            resolution: Resolution::FortyEighth,
                            smoothing: None,
                            depth: DepthPolicy::EqualConfidence,
                        },
                    },
                },
                running: true,
                delay: Some(SavedDelay {
                    applied: Seconds(0.0125),
                    nudged: Seconds(0.000_25),
                    tracking: false,
                }),
                colour: None,
                layout: Some(ac2_proto::samples::pane_layout()),
                compared: Vec::new(),
                offset: ac2_proto::units::Db(0.0),
                eq: Vec::new(),
            },
            SavedMeasurement {
                id: MeasId(2),
                config: MeasConfig {
                    scope: ac2_proto::model::MeasScope::Session,
                    name: "FOH SPL".into(),
                    kind: MeasKind::Spl {
                        config: SplConfig::on_input(1, Weighting::A, TimeWeighting::Fast),
                    },
                },
                running: true,
                delay: None,
                colour: None,
                layout: None,
                compared: Vec::new(),
                offset: ac2_proto::units::Db(0.0),
                eq: Vec::new(),
            },
        ],
        spl_logs: vec![SavedSplLog {
            info: SplLogInfo {
                meas: MeasId(2),
                name: "FOH SPL".into(),
                input: 1,
                mic: None,
            },
            rows: (0..3)
                .map(|k| SplLogRow {
                    start: WallNs(1_790_000_000_000_000_000 + k * 1_000_000_000),
                    measured: Seconds(1.0),
                    laeq: Dbfs(-25.5 + k as f64),
                    lceq: Dbfs(-23.25),
                    lzeq: Dbfs(-22.0),
                    lcpeak: Dbfs(-5.5),
                    lafmax: Dbfs(-20.25),
                    sensitivity: Some(Db(120.0)),
                    position: Some(ac2_proto::model::PositionCorrection::both(3.0)),
                })
                .collect(),
            // Quarter-dB levels: the file's 0.01 dB reads them back exactly.
            bands: (0..2)
                .map(|k| ac2_traces::band_log::BandLogRow {
                    start: WallNs(1_790_000_000_000_000_000 + k * 1_000_000_000),
                    measured: Seconds(1.0),
                    levels: std::array::from_fn(|i| -60.25 + i as f32 * 0.5 + k as f32),
                    correction: Db(if k == 0 { 0.0 } else { 5.0 }),
                    period: if k == 0 {
                        ac2_proto::model::BandPeriod::Day
                    } else {
                        ac2_proto::model::BandPeriod::Night
                    },
                    sensitivity: Some(Db(120.0)),
                })
                .collect(),
        }],
        traces: vec![a, b],
    }
}

#[test]
fn session_round_trip_and_generations() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("show");
    let s = session_sample();
    session::save(&dir, &s).unwrap();
    let back = session::load(&dir).unwrap();
    assert_eq!(back.saved_at, s.saved_at);
    assert_eq!(back.measurements, s.measurements);
    assert_eq!(back.spl_logs.len(), 1);
    assert_eq!(back.spl_logs[0].info, s.spl_logs[0].info);
    assert_eq!(back.spl_logs[0].rows.len(), 3);
    for (x, y) in back.spl_logs[0].rows.iter().zip(&s.spl_logs[0].rows) {
        assert_eq!(
            (x.start, x.measured, x.sensitivity),
            (y.start, y.measured, y.sensitivity)
        );
        assert!((x.laeq.0 - y.laeq.0).abs() < 1e-9);
    }
    // The band log beside it comes back as written.
    assert_eq!(back.spl_logs[0].bands, s.spl_logs[0].bands);
    assert_eq!(back.traces.len(), 2);
    // Columns come back unsmoothed (bit for bit) with the smoothing as an edit.
    assert_eq!(
        back.traces[0].meta.edit.smoothing,
        s.traces[0].meta.edit.smoothing
    );
    assert_eq!(back.traces[0].columns.mag_db[200], 9.0);
    for (x, y) in back.traces.iter().zip(&s.traces) {
        assert_eq!(x.meta, y.meta);
        assert_eq!(x.grid, y.grid);
        let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&x.columns.mag_db), bits(&y.columns.mag_db));
        assert_eq!(
            bits(x.columns.phase_deg.as_ref().unwrap()),
            bits(y.columns.phase_deg.as_ref().unwrap())
        );
    }
    // A second save with one trace leaves only that generation's file behind.
    let mut s2 = s.clone();
    s2.saved_at = WallNs(s.saved_at.0 + 1);
    s2.traces.truncate(1);
    session::save(&dir, &s2).unwrap();
    let files: Vec<_> = std::fs::read_dir(dir.join("traces"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files.len(), 1);
    assert!(files[0].starts_with("4-"), "{files:?}");
    let mut logs: Vec<_> = std::fs::read_dir(dir.join("spl"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    logs.sort();
    assert_eq!(
        logs,
        vec![
            format!("{}-2.bands.csv", s2.saved_at.0),
            format!("{}-2.csv", s2.saved_at.0)
        ]
    );
    assert_eq!(session::load(&dir).unwrap().traces.len(), 1);
    let listed = session::list(tmp.path()).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].0, "show");
}

/// The autosave's in-place save: an unchanged trace's file is not written again, a renamed
/// trace gets a new file, the replaced manifest stays as the previous one with its files,
/// and a linked SPL log (appended to by its owner, cut short at its end) is named, kept and
/// read back up to its last whole line.
#[test]
fn autosave_in_place_writes_only_what_changed() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("autosave");
    let mut s = session_sample();
    let log = s.spl_logs.pop().unwrap();
    std::fs::create_dir_all(dir.join(session::SPL_DIR)).unwrap();
    let rel = format!("{}/2-live.csv", session::SPL_DIR);
    let mut text = ac2_proto::spl_log::export_csv(&log.info, &log.rows);
    let whole_len = text.len();
    text.push_str("2026-09-21T14:17:2"); // An append cut short.
    std::fs::write(dir.join(&rel), &text).unwrap();
    let linked = [session::SavedSplLogFile {
        meas: MeasId(2),
        file: rel.clone(),
        bands: None,
    }];
    let first = session::save_autosave(&dir, &s, &linked).unwrap();
    assert!(!dir.join(session::PREV_MANIFEST).exists());
    let stamp = |f: &str| {
        let m = std::fs::metadata(dir.join(f)).unwrap();
        (m.modified().unwrap(), m.len())
    };
    let before: Vec<_> = first.traces.iter().map(|t| stamp(&t.file)).collect();
    let log_before = stamp(&rel);

    // The same content again: the same files, untouched.
    s.saved_at = WallNs(s.saved_at.0 + 1);
    let second = session::save_autosave(&dir, &s, &linked).unwrap();
    assert_eq!(
        first.traces.iter().map(|t| &t.file).collect::<Vec<_>>(),
        second.traces.iter().map(|t| &t.file).collect::<Vec<_>>()
    );
    let after: Vec<_> = second.traces.iter().map(|t| stamp(&t.file)).collect();
    assert_eq!(before, after);
    assert_eq!(stamp(&rel), log_before);
    let prev = session::read_manifest_named(&dir, session::PREV_MANIFEST).unwrap();
    assert_eq!(prev.saved_at, first.saved_at);

    // A rename: that trace's file is new, the other one stays; the previous manifest's
    // files are kept for it.
    s.saved_at = WallNs(s.saved_at.0 + 1);
    s.traces[0].meta.edit.name = "renamed".into();
    let third = session::save_autosave(&dir, &s, &linked).unwrap();
    assert_ne!(third.traces[0].file, second.traces[0].file);
    assert_eq!(third.traces[1].file, second.traces[1].file);
    assert!(dir.join(&second.traces[0].file).exists());
    let (old, _) = session::load_named(&dir, session::PREV_MANIFEST).unwrap();
    assert_eq!(old.traces[0].meta.edit.name, first.traces[0].meta.edit.name);
    // One save later the file only the oldest manifest named is gone.
    s.saved_at = WallNs(s.saved_at.0 + 1);
    session::save_autosave(&dir, &s, &linked).unwrap();
    assert!(!dir.join(&second.traces[0].file).exists());

    let (back, on_disk) = session::load_named(&dir, session::MANIFEST).unwrap();
    assert_eq!(back.traces[0].meta.edit.name, "renamed");
    assert_eq!(back.spl_logs[0].rows.len(), log.rows.len());
    assert_eq!(
        on_disk,
        vec![session::SplLogOnDisk {
            meas: MeasId(2),
            file: rel,
            rows: log.rows.len() as u64,
            complete_len: whole_len as u64,
            bands: None,
        }]
    );
}

#[test]
fn session_refusals() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("s");
    session::save(&dir, &session_sample()).unwrap();
    let m = dir.join(session::MANIFEST);
    let text = std::fs::read_to_string(&m).unwrap();
    // A session of the previous format is refused with its version named, never read
    // best-effort.
    let (now, before) = (session::VERSION, session::VERSION - 1);
    std::fs::write(
        &m,
        text.replace(
            &format!("\"version\": {now}"),
            &format!("\"version\": {before}"),
        ),
    )
    .unwrap();
    let e = session::load(&dir).unwrap_err();
    assert_eq!(
        e,
        SessionError::Version {
            path: dir.clone(),
            found: before
        }
    );
    assert!(
        e.to_string().contains(&format!("reads version {now} only")),
        "{e}"
    );
    assert_eq!(
        session::load(&tmp.path().join("missing")),
        Err(SessionError::NotFound(tmp.path().join("missing")))
    );
    // A directory with other files is not overwritten.
    let other = tmp.path().join("other");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("notes.txt"), "x").unwrap();
    assert_eq!(
        session::save(&other, &session_sample()),
        Err(SessionError::NotASession(other.clone()))
    );
    assert!(session::validate_name("friday show-2").is_ok());
    for bad in ["", "../x", "a/b", ".hidden", "a\\b", " x"] {
        assert!(session::validate_name(bad).is_err(), "{bad:?}");
    }
}
