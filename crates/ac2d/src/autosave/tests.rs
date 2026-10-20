use super::*;
use crate::leq_log::{self, LeqLog, SharedLog};
use ac2_proto::model::{
    MeasConfig, MeasKind, PeakWeighting, SplConfig, SplLogRow, TimeWeighting, Weighting,
};
use ac2_proto::spl_log::SplLogInfo;
use ac2_proto::units::{Db, Dbfs, MeasId, Seconds, WallNs};
use std::sync::{Arc, Mutex};

const NS: u64 = 1_000_000_000;
const T0: u64 = 1_790_000_000 * NS;

fn data(at: u64) -> SessionData {
    SessionData {
        saved_at: WallNs(at),
        measurements: Vec::new(),
        spl_logs: Vec::new(),
        traces: Vec::new(),
    }
}

fn meter() -> SavedMeasurement {
    SavedMeasurement {
        id: MeasId(1),
        config: MeasConfig {
            scope: ac2_proto::model::MeasScope::Session,
            name: "x".into(),
            kind: MeasKind::Spl {
                config: SplConfig {
                    cue: None,
                    bands: None,
                    input: 0,
                    weighting: Weighting::A,
                    time_weighting: TimeWeighting::Fast,
                    peak_weighting: PeakWeighting::C,
                    leq: ac2_proto::model::LeqConfig::default_windows(),
                    position: None,
                },
            },
        },
        running: true,
        delay: None,
        colour: None,
        layout: None,
        compared: Vec::new(),
        offset: ac2_proto::units::Db(0.0),
    }
}

fn with_meter(at: u64) -> SessionData {
    SessionData {
        measurements: vec![meter()],
        ..data(at)
    }
}

fn row(k: u64) -> SplLogRow {
    SplLogRow {
        start: WallNs(T0 + k * NS),
        measured: Seconds(1.0),
        laeq: Dbfs(-30.0 - (k % 17) as f64 * 0.37),
        lceq: Dbfs(-28.0),
        lzeq: Dbfs(-27.0),
        lcpeak: Dbfs(-12.0),
        lafmax: Dbfs(-25.0),
        sensitivity: Some(Db(120.0)),
        position: None,
    }
}

fn push(log: &SharedLog, from: u64, n: u64) {
    let mut l = leq_log::lock(log);
    for k in from..from + n {
        l.push(row(k));
    }
}

fn spec(log: &SharedLog, resume: Option<Resume>) -> LogSpec {
    LogSpec {
        meas: MeasId(1),
        log: log.clone(),
        info: SplLogInfo {
            meas: MeasId(1),
            name: "x".into(),
            input: 0,
            mic: None,
        },
        resume,
        resume_bands: None,
    }
}

/// Bytes of every file under `dir`.
fn du(dir: &Path) -> u64 {
    let mut n = 0;
    for e in fs::read_dir(dir).expect("dir").flatten() {
        let m = e.metadata().expect("meta");
        n += if m.is_dir() { du(&e.path()) } else { m.len() };
    }
    n
}

#[test]
fn write_keeps_the_previous_as_backup() {
    let root = tempfile::tempdir().expect("tempdir");
    let dir = root.path().join("autosave");
    write(&dir, &data(1), &[]).expect("first");
    assert!(!dir.join(PREV_MANIFEST).exists());
    write(&dir, &data(2), &[]).expect("second");
    write(&dir, &data(3), &[]).expect("third");
    assert_eq!(
        session::read_manifest(&dir).expect("cur").saved_at,
        WallNs(3)
    );
    assert_eq!(
        session::read_manifest_named(&dir, PREV_MANIFEST)
            .expect("prev")
            .saved_at,
        WallNs(2)
    );
    let r = restore(&dir).0.expect("restore");
    assert_eq!((r.data.saved_at, r.from), (WallNs(3), dir.join(MANIFEST)));
}

#[test]
fn restore_falls_back_to_the_backup_and_sets_damage_aside() {
    let root = tempfile::tempdir().expect("tempdir");
    let dir = root.path().join("autosave");
    write(&dir, &data(1), &[]).expect("first");
    write(&dir, &data(2), &[]).expect("second");
    fs::write(dir.join(MANIFEST), b"{ not json").expect("damage");
    let (r, damage) = restore(&dir);
    let r = r.expect("restore");
    assert_eq!(
        (r.data.saved_at, r.from),
        (WallNs(1), dir.join(PREV_MANIFEST))
    );
    assert_eq!(damage.len(), 1);
    assert_eq!(damage[0].kind, StateFile::Autosave);
    assert_eq!(set_aside_count(&dir, "session.json.damaged-"), 1);
    assert!(!dir.join(MANIFEST).exists());
    // Restored again (nothing written in between): the backup still.
    assert_eq!(restore(&dir).0.expect("again").data.saved_at, WallNs(1));
    // The next write goes on from there.
    write(&dir, &data(3), &[]).expect("third");
    assert_eq!(restore(&dir).0.expect("third").data.saved_at, WallNs(3));
    // Both unreadable: the directory is set aside.
    fs::write(dir.join(MANIFEST), b"{ not json").expect("damage");
    fs::write(dir.join(PREV_MANIFEST), b"{ not json").expect("damage");
    let (r, damage) = restore(&dir);
    assert!(r.is_none());
    assert_eq!(damage.len(), 1);
    assert!(!dir.exists());
    assert_eq!(set_aside_count(root.path(), "autosave.damaged-"), 1);
}

fn set_aside_count(dir: &Path, prefix: &str) -> usize {
    fs::read_dir(dir)
        .expect("read_dir")
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
        .count()
}

/// A write interrupted between the manifest's two renames leaves only the previous
/// manifest, which a restore takes; a directory with only the SPL logs of a first write
/// that never completed restores nothing and is left in place.
#[test]
fn an_interrupted_write_restores_the_previous() {
    let root = tempfile::tempdir().expect("tempdir");
    let dir = root.path().join("autosave");
    write(&dir, &data(1), &[]).expect("first");
    fs::rename(dir.join(MANIFEST), dir.join(PREV_MANIFEST)).expect("cut");
    assert_eq!(restore(&dir).0.expect("restore").data.saved_at, WallNs(1));
    let fresh = root.path().join("fresh");
    fs::create_dir_all(fresh.join("spl")).expect("dir");
    fs::write(fresh.join("spl/1-5.csv"), b"# ac2 spl log v2\n").expect("log");
    assert!(restore(&fresh).0.is_none());
    assert!(fresh.exists());
}

#[test]
fn another_version_is_set_aside_not_deleted() {
    let root = tempfile::tempdir().expect("tempdir");
    let dir = root.path().join("autosave");
    write(&dir, &data(1), &[]).expect("first");
    let m = dir.join(session::MANIFEST);
    let text = fs::read_to_string(&m).expect("manifest");
    let other = text.replace(
        &format!("\"version\": {}", session::VERSION),
        "\"version\": 999",
    );
    assert_ne!(text, other);
    fs::write(&m, other).expect("version");
    assert!(restore(&dir).0.is_none());
    assert!(!dir.exists());
    let aside = root.path().join("autosave.v999");
    assert!(aside.join(session::MANIFEST).exists());
}

#[test]
fn no_restore_moves_the_autosave_aside() {
    let root = tempfile::tempdir().expect("tempdir");
    let dir = root.path().join("autosave");
    write(&dir, &data(1), &[]).expect("first");
    skip_restore(&dir);
    assert!(!dir.exists());
    assert!(
        root.path()
            .join("autosave.unrestored")
            .join(session::MANIFEST)
            .exists()
    );
    assert!(restore(&dir).0.is_none());
}

/// A meter with a day of log runs for an hour of simulated minutes, each minute's rows
/// appended and an autosave written every minute regardless: what reaches the disk is
/// the log once, then the new rows and the manifests, never the log again.
#[test]
fn a_running_meter_costs_its_new_rows() {
    let root = tempfile::tempdir().expect("tempdir");
    let dir = root.path().join("autosave");
    let log: SharedLog = Arc::new(Mutex::new(LeqLog::default()));
    let day = 24 * 3600;
    push(&log, 0, day);
    let mut files = SplFiles::new(dir.clone(), Instant::now());
    files.apply(vec![spec(&log, None)]);
    write(&dir, &with_meter(1), &files.linked(&[meter()])).expect("first");
    let whole = files.bytes;
    let path = files.path_of(MeasId(1)).expect("file");
    assert_eq!(fs::metadata(&path).expect("log").len(), whole);
    let before = du(&dir);

    let minutes = 60;
    let mut manifests = 0;
    for m in 0..minutes {
        push(&log, day + m * 60, 60);
        files.append_all();
        write(&dir, &with_meter(2 + m), &files.linked(&[meter()])).expect("write");
        manifests += fs::metadata(dir.join(MANIFEST)).expect("manifest").len();
    }
    let appended = files.bytes - whole;
    let line = whole / day;
    let rows = minutes * 60;
    assert!(
        appended <= rows * (line + 2),
        "appended {appended} B for {rows} rows of ~{line} B"
    );
    // The directory grew by the rows alone (the manifests replace each other).
    let grown = du(&dir) - before;
    assert!(
        grown <= appended + 4096,
        "grew {grown} B, appended {appended} B"
    );
    // Everything written: the log once, the rows, the manifests. The old scheme wrote
    // the whole log (the day and then some) every minute.
    let total = whole + appended + manifests;
    let old = minutes * whole;
    assert!(total * 20 < old, "{total} B vs {old} B");
    assert_eq!(files.path_of(MeasId(1)), Some(path.clone()));

    // Read back: every row, in order.
    let (back, on_disk) = session::load_named(&dir, MANIFEST).expect("load");
    let r = &back.spl_logs[0].rows;
    assert_eq!(r.len() as u64, day + rows);
    assert!(
        r.iter()
            .enumerate()
            .all(|(k, x)| x.start == row(k as u64).start)
    );
    assert_eq!(on_disk[0].rows, day + rows);
}

/// Restored after a power cut that kept half of the last append: every whole row is
/// back, the Leq over them is what was logged, and appending carries on after the last
/// whole row without a broken line.
#[test]
fn restore_reads_past_a_cut_append_and_carries_on() {
    let root = tempfile::tempdir().expect("tempdir");
    let dir = root.path().join("autosave");
    let log: SharedLog = Arc::new(Mutex::new(LeqLog::default()));
    push(&log, 0, 100);
    let mut files = SplFiles::new(dir.clone(), Instant::now());
    files.apply(vec![spec(&log, None)]);
    write(&dir, &with_meter(1), &files.linked(&[meter()])).expect("first");
    push(&log, 100, 50);
    files.append_all();
    files.close_all();
    let path = files.path_of(MeasId(1));
    assert!(path.is_none(), "closed");
    let (_, on_disk) = session::load_named(&dir, MANIFEST).expect("load");
    let path = dir.join(&on_disk[0].file);
    let mut text = fs::read(&path).expect("log");
    // Half a line more, as a cut append leaves it.
    text.extend_from_slice(b"2026-09-21T14:20:00.000Z,17900001");
    fs::write(&path, &text).expect("cut");

    let r = restore(&dir).0.expect("restore");
    let rows = &r.data.spl_logs[0].rows;
    assert_eq!(rows.len(), 150);
    let restored = LeqLog::from_rows(rows.clone(), Vec::new());
    let logged = leq_log::lock(&log).run().expect("run");
    let back = restored.run().expect("run");
    assert_eq!(back.started_at, logged.started_at);
    assert!((back.levels_dbfs[0] - logged.levels_dbfs[0]).abs() < 1e-3);

    // The daemon carries on: the restored log, the file resumed, more rows.
    let log: SharedLog = Arc::new(Mutex::new(restored));
    let disk = &r.logs[0];
    let mut files = SplFiles::new(dir.clone(), Instant::now());
    files.apply(vec![spec(
        &log,
        Some(Resume {
            file: disk.file.clone(),
            rows: disk.rows,
            complete_len: disk.complete_len,
        }),
    )]);
    assert_eq!(files.path_of(MeasId(1)), Some(path.clone()));
    let rewritten = files.bytes;
    assert_eq!(rewritten, 0, "resumed, not rewritten");
    push(&log, 150, 30);
    files.close_all();
    let (back, _) = session::load_named(&dir, MANIFEST).expect("load");
    let r = &back.spl_logs[0].rows;
    assert_eq!(r.len(), 180);
    assert!(
        r.iter()
            .enumerate()
            .all(|(k, x)| x.start == row(k as u64).start)
    );
}

fn band_row(k: u64) -> ac2_traces::band_log::BandLogRow {
    let mut levels = [-70.0f32; ac2_proto::model::BAND_COUNT];
    levels[5] = -60.0 - (k % 7) as f32 * 0.25;
    ac2_traces::band_log::BandLogRow {
        start: WallNs(T0 + k * NS),
        measured: Seconds(1.0),
        levels,
        correction: Db(if k.is_multiple_of(2) { 0.0 } else { 5.0 }),
        period: ac2_proto::model::BandPeriod::Night,
        sensitivity: Some(Db(120.0)),
    }
}

fn push_bands(log: &SharedLog, from: u64, n: u64) {
    let mut l = leq_log::lock(log);
    for k in from..from + n {
        l.push(row(k));
        l.push_band(band_row(k));
    }
}

/// A band meter's rows go to their own file beside the SPL log: written whole, then
/// appended, read back row for row, and resumed after a restore.
#[test]
fn band_rows_are_appended_beside_and_resumed() {
    let root = tempfile::tempdir().expect("tempdir");
    let dir = root.path().join("autosave");
    let log: SharedLog = Arc::new(Mutex::new(LeqLog::default()));
    push_bands(&log, 0, 40);
    let mut files = SplFiles::new(dir.clone(), Instant::now());
    files.apply(vec![spec(&log, None)]);
    write(&dir, &with_meter(1), &files.linked(&[meter()])).expect("first");
    push_bands(&log, 40, 20);
    files.append_all();
    write(&dir, &with_meter(2), &files.linked(&[meter()])).expect("second");
    files.close_all();

    let r = restore(&dir).0.expect("restore");
    let bands = &r.data.spl_logs[0].bands;
    assert_eq!(bands.len(), 60);
    for (k, b) in bands.iter().enumerate() {
        let want = band_row(k as u64);
        assert_eq!(b.start, want.start);
        assert_eq!(b.correction, want.correction);
        assert_eq!(b.period, want.period);
        assert!((b.levels[5] - want.levels[5]).abs() < 0.006, "{b:?}");
    }
    let disk = r.logs[0].bands.as_ref().expect("band file");
    assert!(disk.file.ends_with(".bands.csv"), "{}", disk.file);
    assert_eq!(disk.rows, 60);

    // Resumed: appended after the last whole line, not rewritten.
    let restored = LeqLog::from_rows(r.data.spl_logs[0].rows.clone(), bands.clone());
    let log: SharedLog = Arc::new(Mutex::new(restored));
    let main = &r.logs[0];
    let mut files = SplFiles::new(dir.clone(), Instant::now());
    files.apply(vec![LogSpec {
        resume_bands: Some(Resume {
            file: disk.file.clone(),
            rows: disk.rows,
            complete_len: disk.complete_len,
        }),
        ..spec(
            &log,
            Some(Resume {
                file: main.file.clone(),
                rows: main.rows,
                complete_len: main.complete_len,
            }),
        )
    }]);
    assert_eq!(files.bytes, 0, "resumed, not rewritten");
    push_bands(&log, 60, 10);
    files.close_all();
    let (back, _) = session::load_named(&dir, MANIFEST).expect("load");
    let b = &back.spl_logs[0].bands;
    assert_eq!(b.len(), 70);
    assert!(
        b.iter()
            .enumerate()
            .all(|(k, x)| x.start == band_row(k as u64).start)
    );
}

/// `spl.log_new` swaps the log inside the shared handle: the next append closes the
/// ended log's file and starts one for the new log; a log past its retention by
/// [`spl_files::COMPACT_SLACK`] rows is rewritten with the retained rows only.
#[test]
fn a_new_log_gets_a_new_file_and_long_logs_are_compacted() {
    use ac2_proto::model::SplLogPage;
    let root = tempfile::tempdir().expect("tempdir");
    let dir = root.path().join("autosave");
    let log: SharedLog = Arc::new(Mutex::new(LeqLog::default()));
    push(&log, 0, 10);
    let mut files = SplFiles::new(dir.clone(), Instant::now());
    files.apply(vec![spec(&log, None)]);
    let first = files.path_of(MeasId(1)).expect("file");
    {
        let mut l = leq_log::lock(&log);
        let next = l.next();
        *l = next;
    }
    push(&log, 20, 5);
    files.append_all();
    let second = files.path_of(MeasId(1)).expect("file");
    assert_ne!(first, second);
    write(&dir, &with_meter(1), &files.linked(&[meter()])).expect("write");
    let (back, _) = session::load_named(&dir, MANIFEST).expect("load");
    assert_eq!(back.spl_logs[0].rows.len(), 5);

    let retained = SplLogPage::RETAINED_ROWS as u64;
    // Up to the limit (5 rows are in already): appended.
    let n = retained + spl_files::COMPACT_SLACK - 5;
    for k in (0..n).step_by(3600) {
        push(&log, 100 + k, 3600.min(n - k));
        files.append_all();
    }
    let lines = || fs::read_to_string(&second).expect("log").lines().count() as u64;
    assert_eq!(lines(), 5 + retained + spl_files::COMPACT_SLACK);
    // One more: rewritten.
    push(&log, 200 + retained + spl_files::COMPACT_SLACK, 1);
    files.append_all();
    // Header lines, then the retained rows.
    assert_eq!(lines(), retained + 5);
    assert_eq!(files.path_of(MeasId(1)), Some(second));
}

#[test]
fn an_unwritable_directory_fails_and_keeps_nothing_half_written() {
    let root = tempfile::tempdir().expect("tempdir");
    // A file where the directory's parent should be: no user, root included, can
    // create the autosave under it.
    let blocker = root.path().join("blocker");
    fs::write(&blocker, b"").expect("file");
    let dir = blocker.join("autosave");
    assert!(write(&dir, &data(1), &[]).is_err());
}

/// A log that cannot be written is reported once, with a short reason and since when;
/// written again, it is reported cleared; a meter that goes clears its failure too.
#[test]
fn a_failing_log_is_reported_and_cleared() {
    let root = tempfile::tempdir().expect("tempdir");
    let dir = root.path().join("autosave");
    fs::create_dir_all(&dir).expect("dir");
    // A file where the log directory should be.
    let blocker = dir.join(ac2_traces::session::SPL_DIR);
    fs::write(&blocker, b"").expect("file");
    let log: SharedLog = Arc::new(Mutex::new(LeqLog::default()));
    push(&log, 0, 10);
    let (tx, rx) = mpsc::channel();
    let mut files = SplFiles::new(dir.clone(), Instant::now()).reporting(Some(tx));
    files.apply(vec![spec(&log, None)]);
    let failure = match rx.try_recv() {
        Ok(ControlMsg::LogWrite {
            meas: MeasId(1),
            log: ac2_proto::model::MeterLog::Spl,
            failure: Some(f),
        }) => f,
        other => panic!("{other:?}"),
    };
    assert!(failure.since.0 > T0, "{failure:?}");
    assert!(!failure.reason.is_empty());
    // Retries that fail again say nothing new.
    files.append_all();
    assert!(rx.try_recv().is_err());

    fs::remove_file(&blocker).expect("unblock");
    push(&log, 10, 5);
    files.append_all();
    assert!(matches!(
        rx.try_recv(),
        Ok(ControlMsg::LogWrite {
            meas: MeasId(1),
            failure: None,
            ..
        })
    ));
    assert!(files.path_of(MeasId(1)).is_some_and(|p| p.exists()));

    // Failing again, then the meter goes: cleared.
    fs::remove_dir_all(dir.join(ac2_traces::session::SPL_DIR)).expect("rm");
    fs::write(&blocker, b"").expect("file");
    files.apply(vec![]);
    files.apply(vec![spec(&log, None)]);
    assert!(matches!(
        rx.try_recv(),
        Ok(ControlMsg::LogWrite {
            failure: Some(_),
            ..
        })
    ));
    files.apply(vec![]);
    assert!(matches!(
        rx.try_recv(),
        Ok(ControlMsg::LogWrite { failure: None, .. })
    ));
}

#[test]
fn debounce_waits_for_quiet_and_caps_the_wait() {
    let (tx, _rx) = mpsc::channel();
    let root = tempfile::tempdir().expect("tempdir");
    let w = Writer::spawn(root.path().join("a"), tx).expect("writer");
    let mut a = Autosaver::new(root.path().join("a"), w);
    let t0 = Instant::now();
    let fp: Fingerprint = (Vec::new(), Vec::new(), Vec::new());
    assert!(a.due().is_none());
    assert!(a.changed(&fp, t0));
    assert_eq!(a.due(), Some(t0 + DEBOUNCE));
    let t1 = t0 + Duration::from_secs(1);
    a.changed(&fp, t1);
    assert_eq!(a.due(), Some(t1 + DEBOUNCE));
    let t9 = t0 + Duration::from_secs(9);
    a.changed(&fp, t9);
    assert_eq!(a.due(), Some(t0 + MAX_WAIT));
    // Written: nothing due while in flight, and the same state is not written twice.
    assert!(a.write(data(1), fp.clone(), Vec::new()));
    assert!(a.due().is_none());
    assert!(!a.finished(true, t9));
    assert!(!a.changed(&fp, t9));
    assert!(!a.write(data(2), fp.clone(), Vec::new()));
    // A failure retries, not before RETRY.
    let other: Fingerprint = (
        vec![SavedMeasurement {
            id: MeasId(1),
            config: MeasConfig {
                scope: ac2_proto::model::MeasScope::Session,
                name: "x".into(),
                kind: MeasKind::Spl {
                    config: SplConfig {
                        cue: None,
                        bands: None,
                        input: 0,
                        weighting: Weighting::A,
                        time_weighting: TimeWeighting::Fast,
                        peak_weighting: PeakWeighting::C,
                        leq: ac2_proto::model::LeqConfig::default_windows(),
                        position: None,
                    },
                },
            },
            running: false,
            delay: None,
            colour: None,
            layout: None,
            compared: Vec::new(),
            offset: ac2_proto::units::Db(0.0),
        }],
        Vec::new(),
        Vec::new(),
    );
    assert!(a.changed(&other, t9));
    assert!(a.write(data(3), other, Vec::new()));
    assert!(a.finished(false, t9));
    assert_eq!(a.due(), Some(t9 + RETRY));
}
