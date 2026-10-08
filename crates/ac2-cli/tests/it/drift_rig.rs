//! `ac2 status` names the session's clock domain and, once the loopback monitor has measured
//! it, the drift between output and input: here a simulated rig whose DAC runs 50 ppm slow.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use ac2_audio::fake::{FakeDrive, FakePath};
use ac2_audio::{FakeBackend, FakeConfig};
use ac2_cli::{Cli, Out, run_reporting};
use ac2_client::{Client, ClientConfig, Endpoints, OnDrop};
use ac2_proto::model::{
    DeviceSelector, GeneratorDesired, GeneratorSettings, LoopbackRoute, SessionConfig, Signal,
};
use ac2_proto::units::Dbfs;
use ac2_proto::{Command, ReplyBody};
use ac2d::{Daemon, DaemonConfig, Listen};
use clap::Parser;

async fn status(ep: &Endpoints) -> String {
    let argv = [
        "ac2",
        "--ctrl-endpoint",
        &ep.ctrl,
        "--data-endpoint",
        &ep.data,
        "status",
    ];
    let cli = Cli::try_parse_from(argv).unwrap();
    let (mut so, mut se) = (Vec::new(), Vec::new());
    let code = {
        let mut out = Out::new(false, &mut so);
        run_reporting(&cli, &mut out, &mut se).await
    };
    let text = String::from_utf8(so).unwrap();
    assert_eq!(code, 0, "{text} {}", String::from_utf8_lossy(&se));
    text
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn status_shows_the_clock_domain_and_the_drift() {
    let backend = FakeBackend::new(FakeConfig {
        drive: FakeDrive::Manual,
        paths: vec![FakePath::loopback(0, 0, 2000)],
        drift_ppm: -50.0,
        drift_horizon_seconds: 120.0,
        ..FakeConfig::default()
    })
    .unwrap();
    let listen = Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    };
    let mut cfg = DaemonConfig::new(Arc::new(backend.clone()), listen, -10.0);
    // Debug builds run the DSP slower than real time; the lease is not under test.
    cfg.lease_expiry = Duration::from_secs(60);
    let h = Daemon::start(cfg).unwrap();
    let ep = Endpoints {
        ctrl: h.ctrl_endpoint().to_owned(),
        data: h.data_endpoint().to_owned(),
    };
    let c = Client::connect(ClientConfig::new(ep.clone(), "drift-rig-test"))
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
            loopback: Some(LoopbackRoute {
                output: 0,
                input: 0,
            }),
        },
    })
    .await
    .unwrap();
    let before = status(&ep).await;
    assert!(
        before.contains(
            "clock        one clock (one callback for input and output); drift not measured yet (needs a stimulus)"
        ),
        "{before}"
    );

    let lease = c
        .acquire_lease(false, OnDrop::StopAndRelease)
        .await
        .unwrap();
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
    let mut d = {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(d) = backend.take_driver() {
                break d;
            }
            assert!(Instant::now() < deadline, "no driver parked");
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    };
    // 14 s of device time in 50 ms steps, each once the daemon has read the last: stepping
    // on regardless overflows the capture ring on a busy machine.
    for _ in 0..280 {
        d.run_blocks(10);
        let deadline = Instant::now() + Duration::from_secs(30);
        while d.capture_queued() > 0 {
            assert!(
                Instant::now() < deadline,
                "the daemon stopped reading audio"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    let text = loop {
        let ReplyBody::Snapshot(s) = c.call(Command::StateSnapshot).await.unwrap() else {
            panic!("snapshot");
        };
        if s.state.timing.drift.is_some_and(|d| d.warning) {
            break status(&ep).await;
        }
        assert!(
            Instant::now() < deadline,
            "no drift warning: {:?}",
            s.state.timing
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let line = text
        .lines()
        .find(|l| l.starts_with("clock"))
        .unwrap_or_else(|| panic!("no clock line in\n{text}"));
    assert!(
        line.starts_with(
            "clock        one clock (one callback for input and output); drift +50.0 ppm over "
        ) || line.starts_with(
            "clock        one clock (one callback for input and output); drift +49.9 ppm over "
        ) || line.starts_with(
            "clock        one clock (one callback for input and output); drift +50.1 ppm over "
        ),
        "{line}"
    );
    assert!(
        line.ends_with("  WARNING: output and input on different clocks"),
        "{line}"
    );
    drop(lease);
}
