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
