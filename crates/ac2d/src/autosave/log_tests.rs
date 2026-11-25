//! The SPL and band logs on disk over the years an unattended meter logs: rows across a
//! clock stepped back, a daylight-saving change and a year's end read back exactly; the
//! high-water mark lies beside the log after a clean stop; a log of another version is set
//! aside under its own name, never deleted.

use super::tests::{band_row, meter, row, spec, with_meter};
use super::*;
use crate::leq_log::{LeqLog, SharedLog};
use ac2_proto::model::{BandPeriod, LogClock, LogGap};
use ac2_proto::units::{Dbfs, MeasId, WallNs};
use ac2_traces::log_mark::{self, MarkRead};
use std::sync::{Arc, Mutex};

const NS: u64 = 1_000_000_000;
/// 2026-12-31T23:59:58Z.
const EVE: u64 = 1_798_761_598;

/// Run 0 across the year's end, its wall clock then stepped back an hour (the index goes
/// on); run 1 after a power cut with the gap unknown and the clock not yet synchronised.
/// Band rows judged in UTC+2, then in UTC+3 after a daylight-saving change, then can't
/// tell with the clock unsynchronised.
fn rows() -> (
    Vec<ac2_proto::model::SplLogRow>,
    Vec<ac2_traces::band_log::BandLogRow>,
) {
    let walls = [
        EVE,
        EVE + 1,
        EVE + 2,
        EVE + 3,
        EVE - 3600 + 4,
        EVE - 3600 + 5,
    ];
    let mut spl: Vec<_> = walls
        .iter()
        .enumerate()
        .map(|(k, w)| ac2_proto::model::SplLogRow {
            start: WallNs(w * NS),
            laeq: Dbfs(-30.25),
            ..row(k as u64)
        })
        .collect();
    let mut bands: Vec<_> = walls
        .iter()
        .enumerate()
        .map(|(k, w)| ac2_traces::band_log::BandLogRow {
            start: WallNs(w * NS),
            utc_offset_s: if k < 3 { 7_200 } else { 10_800 },
            period: Some(if k < 3 {
                BandPeriod::Night
            } else {
                BandPeriod::Day
            }),
            ..band_row(k as u64)
        })
        .collect();
    for k in 0..3u64 {
        let wall = WallNs((EVE + 40 + k) * NS);
        let gap = (k == 0).then_some(LogGap::Unknown);
        spl.push(ac2_proto::model::SplLogRow {
            second: 700 + k,
            run: 1,
            start: wall,
            clock: LogClock::Unsynced,
            gap,
            laeq: Dbfs(-30.25),
            ..row(0)
        });
        bands.push(ac2_traces::band_log::BandLogRow {
            second: 700 + k,
            run: 1,
            start: wall,
            clock: LogClock::Unsynced,
            gap,
            utc_offset_s: 10_800,
            period: None,
            ..band_row(0)
        });
    }
    (spl, bands)
}

#[test]
fn keys_clocks_and_offsets_read_back_from_disk_with_the_mark_beside() {
    let root = tempfile::tempdir().expect("tempdir");
    let dir = root.path().join("autosave");
    let (spl, bands) = rows();
    let mut l = LeqLog::from_rows(spl.clone(), bands.clone());
    l.issue(1, 702, 9_000);
    let log: SharedLog = Arc::new(Mutex::new(l));
    let mut files = SplFiles::new(dir.clone(), Instant::now());
    files.apply(vec![spec(&log, None)]);
    write(&dir, &with_meter(1), &files.linked(&[meter()])).expect("write");
    let path = files.path_of(MeasId(1)).expect("log file");
    files.close_all();

    let text = fs::read_to_string(&path).expect("log");
    assert!(text.contains("2027-01-01T00:00:00.000Z"), "{text}");
    let r = restore(&dir).0.expect("restore");
    let back = &r.data.spl_logs[0];
    assert_eq!(back.rows, spl);
    assert_eq!(back.bands.len(), bands.len());
    for (a, b) in back.bands.iter().zip(&bands) {
        assert_eq!(
            (
                a.second,
                a.run,
                a.start,
                a.clock,
                a.gap,
                a.utc_offset_s,
                a.period
            ),
            (
                b.second,
                b.run,
                b.start,
                b.clock,
                b.gap,
                b.utc_offset_s,
                b.period
            )
        );
    }
    // The exact mark at the last index issued, written as the log closed.
    let MarkRead::Read(m) = log_mark::read(&log_mark::path_of(&path)) else {
        panic!("no mark beside {}", path.display())
    };
    assert_eq!((m.highest_run, m.anchor.run, m.anchor.second), (1, 1, 702));
    assert_eq!(back.mark.as_ref(), Some(&m));
}

#[test]
fn a_log_of_another_version_is_set_aside_by_its_version() {
    let root = tempfile::tempdir().expect("tempdir");
    let dir = root.path().join("autosave");
    let (spl, _) = rows();
    let log: SharedLog = Arc::new(Mutex::new(LeqLog::from_rows(spl, Vec::new())));
    let mut files = SplFiles::new(dir.clone(), Instant::now());
    files.apply(vec![spec(&log, None)]);
    write(&dir, &with_meter(1), &files.linked(&[meter()])).expect("write");
    let path = files.path_of(MeasId(1)).expect("log file");
    files.close_all();
    let old =
        fs::read_to_string(&path)
            .expect("log")
            .replacen("# ac2 spl log v3", "# ac2 spl log v2", 1);
    fs::write(&path, &old).expect("v2");

    let (r, damage) = restore(&dir);
    let r = r.expect("restore");
    assert!(r.data.spl_logs.is_empty(), "the meter starts a new log");
    assert_eq!(damage.len(), 1, "{damage:?}");
    let d = &damage[0];
    assert_eq!(d.kind, StateFile::SplLog);
    assert!(d.reason.contains("version 2"), "{}", d.reason);
    assert!(
        d.reason.contains("plain CSV of its own version"),
        "{}",
        d.reason
    );
    let aside = d.set_aside_as.as_ref().expect("set aside");
    assert_eq!(
        aside.parent(),
        Some(root.path().join("autosave.old-version-files").as_path())
    );
    let name = aside
        .file_name()
        .expect("a file name")
        .to_string_lossy()
        .into_owned();
    assert!(name.contains(".csv.v2-"), "{name}");
    assert_eq!(fs::read_to_string(aside).expect("aside"), old, "kept whole");
    assert!(!path.exists());
}
