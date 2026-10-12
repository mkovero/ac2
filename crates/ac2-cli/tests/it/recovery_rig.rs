//! `ac2 status` says when the open session's audio has stopped, why, and where reopening
//! stands; the line goes once the daemon has reopened the session by itself. A simulated
//! rig whose device vanishes and comes back. `ac2 session panel` opens the driver's
//! control panel, and `session status` says what a change there did to the session.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use ac2_audio::fake::{FakeDrive, Pace};
use ac2_audio::{FakeBackend, FakeConfig};
use ac2_cli::{Cli, Out, run_reporting};
use ac2_client::{Client, ClientConfig, Endpoints};
use ac2_proto::Command;
use ac2_proto::model::{DeviceSelector, SessionConfig};
use ac2d::{Daemon, DaemonConfig, Listen};
use clap::Parser;

async fn status(ep: &Endpoints, json: bool) -> String {
    run(ep, json, &["status"]).await
}

async fn run(ep: &Endpoints, json: bool, args: &[&str]) -> String {
    let mut argv = vec![
        "ac2",
        "--ctrl-endpoint",
        &ep.ctrl,
        "--data-endpoint",
        &ep.data,
    ];
    if json {
        argv.push("--json");
    }
    argv.extend_from_slice(args);
    let cli = Cli::try_parse_from(argv).unwrap();
    let (mut so, mut se) = (Vec::new(), Vec::new());
    let code = {
        let mut out = Out::new(json, &mut so);
        run_reporting(&cli, &mut out, &mut se).await
    };
    let text = String::from_utf8(so).unwrap();
    assert_eq!(code, 0, "{text} {}", String::from_utf8_lossy(&se));
    text
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
    let listen = Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    };
    let h = Daemon::start(DaemonConfig::new(Arc::new(backend.clone()), listen, -10.0)).unwrap();
    let ep = Endpoints {
        ctrl: h.ctrl_endpoint().to_owned(),
        data: h.data_endpoint().to_owned(),
    };
    let c = Client::connect(ClientConfig::new(ep.clone(), "recovery-rig-test"))
        .await
        .unwrap();
    c.wait_synced(Duration::from_secs(5)).await.unwrap();
    c.call(Command::SessionOpen {
        config: SessionConfig {
            backend: None,
            input_device: DeviceSelector::Default,
            output_device: DeviceSelector::Default,
            input_channels: vec![0, 1],
            output_channels: 2,
            sample_rate_hz: Some(48_000),
            buffer_frames: Some(256),
            loopback: None,
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
        .find(|l| l.starts_with("audio"))
        .unwrap_or_else(|| panic!("no audio line in\n{text}"));
    assert!(
        line.starts_with("audio        STOPPED: audio host ended the stream at "),
        "{line}"
    );
    assert!(
        line.contains("failed: Fake backend unavailable: the simulated device is gone · next in "),
        "{line}"
    );
    assert!(text.contains("session open"), "{text}");
    let json: serde_json::Value = serde_json::from_str(&status(&ep, true).await).unwrap();
    assert_eq!(json["session"]["stopped"]["cause"]["type"], "host_ended");
    assert_eq!(json["session"]["stopped"]["recovery"]["state"], "waiting");

    backend.restore();
    let text = status_until(&ep, "reopened session", |t| !t.contains("STOPPED")).await;
    assert!(text.contains("session open"), "{text}");
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
    let listen = Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    };
    let h = Daemon::start(DaemonConfig::new(Arc::new(backend.clone()), listen, -10.0)).unwrap();
    let ep = Endpoints {
        ctrl: h.ctrl_endpoint().to_owned(),
        data: h.data_endpoint().to_owned(),
    };
    let fail = {
        let cli = Cli::try_parse_from([
            "ac2",
            "--ctrl-endpoint",
            &ep.ctrl,
            "--data-endpoint",
            &ep.data,
            "session",
            "panel",
        ])
        .unwrap();
        let (mut so, mut se) = (Vec::new(), Vec::new());
        let code = run_reporting(&cli, &mut Out::new(false, &mut so), &mut se).await;
        (code, String::from_utf8(se).unwrap())
    };
    assert_ne!(fail.0, 0);
    assert!(
        fail.1.contains("no session is open: name the backend"),
        "{}",
        fail.1
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
}
