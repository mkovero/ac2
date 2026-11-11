//! `ac2 status` says when the open session's audio has stopped, why, and where reopening
//! stands; the line goes once the daemon has reopened the session by itself. A simulated
//! rig whose device vanishes and comes back. `ac2 session panel` opens the driver's
//! control panel, and `session status` says what a change there did to the session.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use ac2_audio::fake::{FakeDrive, Pace};
use ac2_audio::{FakeBackend, FakeConfig};
use ac2_client::Endpoints;
use ac2_proto::Command;
use ac2_proto::model::SessionConfig;

use super::common::{ac2, client, rig_on, session};

async fn status(ep: &Endpoints, json: bool) -> String {
    run(ep, json, &["status"]).await
}

async fn run(ep: &Endpoints, json: bool, args: &[&str]) -> String {
    let r = ac2(ep, json, args).await;
    assert_eq!(r.code, 0, "{} {}", r.stdout, r.stderr);
    r.stdout
}

async fn status_until(ep: &Endpoints, what: &str, ok: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let t = status(ep, false).await;
        if ok(&t) {
            return t;
        }
        assert!(Instant::now() < deadline, "no {what} in\n{t}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn status_shows_stopped_audio_until_the_daemon_reopens_it() {
    let backend = FakeBackend::new(FakeConfig {
        drive: FakeDrive::Thread(Pace::Realtime),
        ..FakeConfig::default()
    })
    .unwrap();
    let (_h, ep) = rig_on(Arc::new(backend.clone()), |_| {});
    let c = client(&ep, "recovery-rig-test").await;
    c.call(Command::SessionOpen {
        config: SessionConfig {
            sample_rate_hz: Some(48_000),
            buffer_frames: Some(256),
            ..session(vec![0, 1])
        },
    })
    .await
    .unwrap();
    let before = status(&ep, false).await;
    assert!(!before.contains("STOPPED"), "{before}");

    backend.vanish(None);
    let text = status_until(&ep, "failed attempt", |t| t.contains("failed:")).await;
    let line = text
        .lines()
        .find(|l| l.starts_with("audio   "))
        .unwrap_or_else(|| panic!("no audio line in\n{text}"));
    assert!(
        line.starts_with("audio        STOPPED: audio host ended the stream at "),
        "{line}"
    );
    assert!(
        line.contains("failed: Fake backend unavailable: the simulated device is gone · next in "),
        "{line}"
    );
    assert!(text.contains("audio I/O open"), "{text}");
    let json: serde_json::Value = serde_json::from_str(&status(&ep, true).await).unwrap();
    assert_eq!(json["session"]["stopped"]["cause"]["type"], "host_ended");
    assert_eq!(json["session"]["stopped"]["recovery"]["state"], "waiting");

    backend.restore();
    let text = status_until(&ep, "reopened session", |t| !t.contains("STOPPED")).await;
    assert!(text.contains("audio I/O open"), "{text}");
}

/// `ac2 session panel` on the open session's device; the operator picks 44.1 kHz and 512
/// frames there; `session status` then shows the session at them and says the driver
/// changed them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_panel_opens_the_drivers_panel_and_status_says_what_it_changed() {
    let backend = FakeBackend::new(FakeConfig {
        drive: FakeDrive::Thread(Pace::Realtime),
        ..FakeConfig::default()
    })
    .unwrap();
    let (_h, ep) = rig_on(Arc::new(backend.clone()), |_| {});
    let fail = ac2(&ep, false, &["session", "panel"]).await;
    assert_ne!(fail.code, 0);
    assert!(
        fail.stderr
            .contains("no audio I/O is open: name the backend"),
        "{}",
        fail.stderr
    );

    run(
        &ep,
        false,
        &[
            "session",
            "open",
            "--backend",
            "fake",
            "--in",
            "1-2",
            "--rate",
            "48khz",
            "--buffer",
            "256samples",
        ],
    )
    .await;
    backend.script_panel(Some((44_100, 512)));
    let said = run(&ep, false, &["session", "panel"]).await;
    assert!(
        said.starts_with("fake: the driver's control panel is open"),
        "{said}"
    );
    assert_eq!(backend.panel_opens(), 1);

    let deadline = Instant::now() + Duration::from_secs(30);
    let text = loop {
        let t = run(&ep, false, &["session", "status"]).await;
        if t.contains("44.1 kHz") {
            break t;
        }
        assert!(Instant::now() < deadline, "no reopen at 44.1 kHz in\n{t}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(
        text.contains("rate   44.1 kHz  buffer 512 samples"),
        "{text}"
    );
    assert!(
        text.contains(
            "NOTE   the driver changed the rate to 44.1 kHz (was 48.0 kHz) and the buffer to \
             512 frames (was 256); measurements restarted at the new settings"
        ),
        "{text}"
    );
    assert!(
        text.contains("NOTE   Audio driver reset (driver asked to be reset)"),
        "{text}"
    );
}
