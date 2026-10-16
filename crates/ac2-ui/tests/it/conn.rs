//! The daemon link against the fake daemon, no window: quitting stops and releases the
//! stimulus when the daemon answers, and never waits longer than `QUIT_GRACE` when it does
//! not (decision K6; the daemon's lease expiry is the safety net).
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use ac2_client::ClientConfig;
use ac2_client::fake::{FakeDaemon, FakeOptions};
use ac2_proto::model::{GeneratorSettings, Signal};
use ac2_proto::units::Dbfs;
use ac2_ui::conn::{Conn, ConnEvent, QUIT_GRACE, Request, StimEvent, Target};

fn link(fake: &FakeDaemon) -> Conn {
    Conn::start(
        Target {
            config: ClientConfig::new(fake.endpoints(), "ac2-ui link test"),
            describe: "fake daemon".into(),
        },
        Arc::new(|| {}),
    )
    .unwrap()
}

fn wait_for(conn: &Conn, what: &str, mut ok: impl FnMut(&ConnEvent) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if conn.drain().iter().any(&mut ok) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Connects and arms (never fires: the fake has no audio, and arming is enough to hold the
/// lease).
fn armed(fake: &FakeDaemon) -> Conn {
    let conn = link(fake);
    wait_for(&conn, "connected", |e| {
        matches!(e, ConnEvent::Connected { .. })
    });
    conn.send(Request::StimArm {
        settings: GeneratorSettings {
            signal: Signal::Pink,
            level: Dbfs(-30.0),
            band: None,
            outputs: vec![0],
        },
        force: false,
    });
    wait_for(&conn, "armed", |e| {
        matches!(e, ConnEvent::Stimulus(StimEvent::Armed))
    });
    assert!(fake.lock().state.generator.armed);
    conn
}

#[test]
fn quit_stops_and_releases_the_stimulus() {
    let fake = FakeDaemon::start(FakeOptions::default()).unwrap();
    let conn = armed(&fake);
    assert!(conn.close(), "the link finished within the grace period");
    let s = fake.lock();
    assert!(!s.state.generator.armed && s.state.generator.owner.is_none());
    assert!(s.executions.get("gen.release").is_some_and(|n| *n >= 1));
}

#[test]
fn quit_waits_at_most_the_grace_period_for_a_silent_daemon() {
    let fake = FakeDaemon::start(FakeOptions::default()).unwrap();
    let conn = armed(&fake);
    // The daemon stops answering: stop and release can never complete.
    fake.lock().mute = true;
    let t0 = Instant::now();
    // The link gives up on the stop at the same deadline, so `close` may see it finish
    // just in time or not; either way the quit takes the grace period and no longer.
    conn.close();
    let took = t0.elapsed();
    assert!(took >= QUIT_GRACE - Duration::from_millis(50), "{took:?}");
    assert!(took <= QUIT_GRACE + Duration::from_millis(500), "{took:?}");
    // The daemon side's lease expiry is what stops the output.
    let deadline = Instant::now() + Duration::from_secs(5);
    while fake.lock().state.generator.armed {
        assert!(Instant::now() < deadline, "the lease never expired");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(fake.lock().expiries, 1);
}

/// The reducer's preview requests reach the daemon in the order it issued them. Closing and
/// reopening the session dialog sends `preview_stop` then `preview` back to back; were they
/// to cross on the way, the daemon would close the preview just asked for and the meters
/// would stay blank until the next renewal.
#[test]
fn preview_requests_reach_the_daemon_in_order() {
    use ac2_proto::model::{BackendKind, DeviceId};
    let fake = FakeDaemon::start(FakeOptions::default()).unwrap();
    let conn = link(&fake);
    wait_for(&conn, "connected", |e| {
        matches!(e, ConnEvent::Connected { .. })
    });
    let dev = DeviceId("fake:loop".into());
    let preview = || Request::Preview {
        backend: BackendKind::Fake,
        device: dev.clone(),
    };
    let rounds = 100;
    for _ in 0..rounds {
        conn.send(Request::PreviewStop);
        conn.send(preview());
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    let ops = loop {
        let ops: Vec<&'static str> = fake
            .lock()
            .requests
            .iter()
            .map(|(op, _)| *op)
            .filter(|op| op.starts_with("session.preview"))
            .collect();
        if ops.len() >= 2 * rounds {
            break ops;
        }
        assert!(
            Instant::now() < deadline,
            "{} of {} arrived",
            ops.len(),
            2 * rounds
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    let want: Vec<&str> = (0..rounds)
        .flat_map(|_| ["session.preview_stop", "session.preview"])
        .collect();
    assert_eq!(ops, want);
    assert_eq!(fake.lock().preview, Some((BackendKind::Fake, dev)));
}

/// "Export Leq log…": every row of the meter's log, read a page at a time off the UI
/// thread, written as the log CSV in a folder made for it; the toast counts the seconds.
#[test]
fn the_leq_log_exports_as_csv_in_a_new_folder() {
    use ac2_proto::model::{SplLogRow, SplLogWhich};
    use ac2_proto::spl_log::SplLogInfo;
    use ac2_proto::units::{Dbfs, MeasId, Seconds, WallNs};
    let fake = FakeDaemon::start(FakeOptions::default()).unwrap();
    let m = ac2_proto::samples::spl_measurement();
    // More than a page of `spl.log_get`.
    let n = 25_000;
    let rows: Vec<SplLogRow> = (0..n)
        .map(|k| SplLogRow {
            start: WallNs((1_792_000_000 + k) * 1_000_000_000),
            measured: Seconds(1.0),
            laeq: Dbfs(-20.0),
            lceq: Dbfs(-18.0),
            lzeq: Dbfs(-17.0),
            lcpeak: Dbfs(-5.0),
            lafmax: Dbfs(-12.0),
            sensitivity: None,
            position: None,
        })
        .collect();
    {
        let mut s = fake.lock();
        s.state.measurements.push(m.clone());
        s.spl_rows.insert(m.id, rows);
    }
    let conn = link(&fake);
    wait_for(&conn, "connected", |e| {
        matches!(e, ConnEvent::Connected { .. })
    });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ac2-exports").join("FOH-SPL-leq.csv");
    conn.send(Request::ExportLeqLog {
        info: SplLogInfo {
            meas: MeasId(4),
            name: "FOH SPL".into(),
            input: 1,
            mic: None,
        },
        log: SplLogWhich::Current,
        path: path.clone(),
    });
    let mut said = String::new();
    wait_for(&conn, "the export", |e| match e {
        ConnEvent::Reply { what, .. } if what.contains("written to") => {
            said = what.clone();
            true
        }
        _ => false,
    });
    assert!(
        said.starts_with("25000 seconds of FOH SPL written to "),
        "{said}"
    );
    assert!(said.ends_with("FOH-SPL-leq.csv"), "{said}");
    let csv = std::fs::read_to_string(&path).unwrap();
    let data = csv
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .count();
    // The rows and the column header.
    assert_eq!(data, n as usize + 1, "{}", &csv[..300.min(csv.len())]);
}
