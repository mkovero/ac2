//! Where runs start: after a clean stop, a power cut with and without a mark, a clock
//! stepped back, an unsynchronised clock, a suspend within the process; rows held for
//! their run's anchor; re-stamping once the clock is synchronised.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ac2_proto::model::{LogClock, LogGap, SplLogRow};
use ac2_proto::spl_log::SplLogInfo;
use ac2_proto::units::{MeasId, WallNs};
use ac2_traces::log_mark::{Anchor, LogMark};
use ac2_traces::session::SavedSplLog;

use super::super::tests::row;
use super::super::{LeqLog, NS};
use super::{MAX_HELD, RESERVE_S, RETRY_S, RunClock, RunStart, Waker};

const BOOT: &str = "boot-a";
/// 2026-11-25T12:00:00Z.
const T0: u64 = 1_795_867_200;

/// Run 0's rows at indices `0..n`, a second apart from `T0`.
fn rows(n: u64, clock: LogClock) -> Vec<SplLogRow> {
    (0..n)
        .map(|k| SplLogRow {
            second: k,
            clock,
            gap: (k == 0).then_some(LogGap::Known),
            ..row((T0 + k) * NS, -30.0)
        })
        .collect()
}

fn mark(second: u64, boot_ms: u64) -> LogMark {
    LogMark::new(
        0,
        Anchor {
            boot_id: BOOT.into(),
            run: 0,
            second,
            boot_ms,
        },
    )
}

fn restored(rows: Vec<SplLogRow>, mark: Option<LogMark>) -> LeqLog {
    LeqLog::restored(SavedSplLog {
        info: SplLogInfo {
            meas: MeasId(1),
            name: "FOH".into(),
            input: 0,
            mic: None,
            zone: "Europe/Helsinki".into(),
            started: WallNs(T0 * NS),
        },
        rows,
        bands: Vec::new(),
        mark,
    })
}

fn now(boot_id: &str, boot_ms: u64, wall_s: u64, clock: LogClock) -> RunClock<'_> {
    RunClock {
        boot_ms,
        boot_id,
        wall_ns: wall_s * NS,
        clock,
    }
}

/// Last row at index 99, ending at boot-clock 1 000 000 ms: a restart 30 s later on the
/// same boot leaves 30 seconds out.
#[test]
fn the_same_boot_counts_the_seconds_off_the_boot_clock() {
    let mut log = restored(rows(100, LogClock::Synced), Some(mark(99, 1_000_000)));
    let s = log.begin_run(&now(BOOT, 1_030_000, 0, LogClock::Unsynced));
    assert_eq!(
        s,
        RunStart {
            run: 1,
            first: 130,
            gap: LogGap::Known
        }
    );
    assert_eq!(
        log.started(),
        WallNs(T0 * NS),
        "the log goes on for clients"
    );
}

/// A power cut: the mark reserved up to 699 and the boot clock restarted, so the gap
/// comes from the trusted wall clocks, and never lands below the reservation.
#[test]
fn a_new_boot_counts_trusted_wall_clocks_and_keeps_above_the_reservation() {
    let m = mark(99 + RESERVE_S, 1_600_000);
    let mut log = restored(rows(100, LogClock::Synced), Some(m.clone()));
    let s = log.begin_run(&now("boot-b", 5_000, T0 + 99 + 3_600, LogClock::NotStated));
    assert_eq!((s.first, s.gap), (99 + 3_600, LogGap::Known));
    let mut log = restored(rows(100, LogClock::Synced), Some(m));
    let s = log.begin_run(&now("boot-b", 5_000, T0 + 99 + 20, LogClock::Synced));
    assert_eq!((s.first, s.gap), (100 + RESERVE_S, LogGap::Known));
}

/// The wall clock came back before the last row (an RTC that lost its time), or is not
/// trusted: the gap is unknown, the index still rises above the reservation, and no
/// window reaches below the new run.
#[test]
fn a_clock_stepped_back_or_unsynced_leaves_the_gap_unknown() {
    for clock in [LogClock::Synced, LogClock::Unsynced] {
        let wall = if clock == LogClock::Synced {
            T0 - 86_400
        } else {
            T0 + 500
        };
        let mut log = restored(rows(100, LogClock::Synced), Some(mark(699, 1_600_000)));
        let s = log.begin_run(&now("boot-b", 5_000, wall, clock));
        assert_eq!((s.first, s.gap), (700, LogGap::Unknown), "{clock:?}");
        assert_eq!(log.runs.floor, 700);
        log.push_row(SplLogRow {
            second: s.first,
            run: s.run,
            ..row(wall * NS, -30.0)
        });
        assert_eq!(log.rows().last().expect("a row").gap, Some(LogGap::Unknown));
    }
}

/// No mark, or one below the last row (written before rows it does not cover): rows were
/// maybe served and never written, so the log takes a new start and the gap is unknown.
#[test]
fn a_missing_or_stale_mark_starts_the_log_anew() {
    for m in [None, Some(mark(50, 1_000_000))] {
        let mut log = restored(rows(100, LogClock::Synced), m.clone());
        assert_ne!(log.started(), WallNs(T0 * NS), "{m:?}");
        let s = log.begin_run(&now(BOOT, 1_030_000, T0 + 130, LogClock::Synced));
        assert_eq!((s.first, s.gap), (100, LogGap::Unknown), "{m:?}");
    }
}

/// A suspend the backend did not report ends the run; the next starts after the boot
/// clock's seconds since the last index issued.
#[test]
fn within_a_process_the_next_run_follows_the_boot_clock() {
    let mut log = LeqLog::default();
    let a = log.begin_run(&now(BOOT, 10_000, T0, LogClock::Synced));
    assert_eq!((a.run, a.first, a.gap), (0, 0, LogGap::Known));
    log.issue(0, 41, 52_000);
    let b = log.begin_run(&now(BOOT, 351_500, T0 + 341, LogClock::Synced));
    assert_eq!((b.run, b.first, b.gap), (1, 341, LogGap::Known));
}

/// With a write thread attached, a run's rows wait until its anchor is written (or could
/// not be): no second is served that a power cut could hand out again.
#[test]
fn rows_wait_for_their_runs_anchor() {
    let woken = Arc::new(AtomicUsize::new(0));
    let w = woken.clone();
    let mut log = LeqLog::default();
    log.attach(Waker(Arc::new(move || {
        w.fetch_add(1, Ordering::Relaxed);
    })));
    let s = log.begin_run(&now(BOOT, 10_000, T0, LogClock::Synced));
    assert_eq!(woken.load(Ordering::Relaxed), 1);
    log.push_row(SplLogRow {
        second: 0,
        run: s.run,
        ..row(T0 * NS, -30.0)
    });
    assert!(log.rows().is_empty(), "held for the anchor");
    let m = log.take_want().expect("anchor asked for");
    assert_eq!(m.anchor.second, RESERVE_S);
    log.mark_written(MeasId(1), &m, Err("disk full".into()));
    assert_eq!(log.rows().len(), 1, "served once the write was tried");
    assert_eq!(log.rows()[0].gap, Some(LogGap::Known));
    // The exact mark after a clean stop is the last index issued.
    log.issue(s.run, 0, 11_000);
    assert_eq!(log.exact_mark().expect("a mark").anchor.second, 0);
}

/// A mark whose write failed reserves nothing: the write is asked for again [`RETRY_S`]
/// seconds on (not every second, nor only near a reservation never stored), and once one
/// is written its reservation stands.
#[test]
fn a_failed_mark_write_is_retried_not_taken_as_written() {
    let mut log = LeqLog::default();
    log.attach(Waker(Arc::new(|| {})));
    let s = log.begin_run(&now(BOOT, 10_000, T0, LogClock::Synced));
    let m = log.take_want().expect("anchor asked for");
    log.mark_written(MeasId(1), &m, Err("disk full".into()));
    for k in 0..RETRY_S {
        log.issue(s.run, k, 11_000 + k * 1000);
        assert!(log.take_want().is_none(), "asked again at second {k}");
    }
    log.issue(s.run, RETRY_S, 11_000 + RETRY_S * 1000);
    let m = log.take_want().expect("retried");
    assert_eq!(m.anchor.second, RETRY_S + RESERVE_S);
    log.mark_written(MeasId(1), &m, Ok(()));
    log.issue(s.run, RETRY_S + 1, 12_000 + RETRY_S * 1000);
    assert!(log.take_want().is_none(), "the written reservation stands");
}

/// A write thread that never reports holds at most [`MAX_HELD`] rows: the oldest go, so
/// nothing is served unanchored, and the newest are served once the anchor is written.
#[test]
fn rows_held_for_an_anchor_never_written_are_bounded() {
    let mut log = LeqLog::default();
    log.attach(Waker(Arc::new(|| {})));
    let s = log.begin_run(&now(BOOT, 10_000, T0, LogClock::Synced));
    let n = MAX_HELD as u64 + 5;
    for k in 0..n {
        log.push_row(SplLogRow {
            second: k,
            run: s.run,
            ..row((T0 + k) * NS, -30.0)
        });
    }
    assert!(log.rows().is_empty(), "nothing served unanchored");
    assert_eq!(log.runs.held.len(), MAX_HELD);
    let m = log.take_want().expect("anchor asked for");
    log.mark_written(MeasId(1), &m, Ok(()));
    let rows = log.rows();
    assert_eq!(rows.len(), MAX_HELD);
    assert_eq!((rows[0].second, rows[MAX_HELD - 1].second), (5, n - 1));
    assert_eq!(rows[0].gap, Some(LogGap::Known), "the run's gap kept");
}

/// Rows logged before the clock synchronised are re-stamped from their distance to the
/// second it did, and marked derived; synchronised rows and other runs stay as logged.
#[test]
fn a_synchronised_clock_restamps_the_runs_unsynced_rows() {
    let mut r = rows(5, LogClock::Unsynced);
    r[4].clock = LogClock::Synced;
    let mut log = LeqLog::from_rows(r, Vec::new());
    let synced = WallNs((T0 + 10_000) * NS);
    log.restamp(0, 4, synced);
    let back = log.rows();
    for (k, r) in back.iter().enumerate().take(4) {
        assert_eq!(r.clock, LogClock::Derived);
        assert_eq!(r.start.0, synced.0 - (4 - k as u64) * NS);
    }
    assert_eq!(back[4].start.0, (T0 + 4) * NS);
    log.restamp(1, 4, WallNs(0));
    assert_eq!(log.rows(), back);
}
