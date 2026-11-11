//! `ac2 status` names the session's clock domain and, once the loopback monitor has measured
//! it, the drift between output and input: here a simulated rig whose DAC runs 50 ppm slow.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use ac2_audio::fake::{FakeDrive, FakePath};
use ac2_audio::{FakeBackend, FakeConfig};
use ac2_client::{Endpoints, OnDrop};
use ac2_proto::model::{GeneratorDesired, GeneratorSettings, LoopbackRoute, SessionConfig, Signal};
use ac2_proto::units::Dbfs;
use ac2_proto::{Command, ReplyBody};

use super::common::{ac2, client, rig_on, session};

async fn status(ep: &Endpoints) -> String {
    let r = ac2(ep, false, &["status"]).await;
    assert_eq!(r.code, 0, "{} {}", r.stdout, r.stderr);
    r.stdout
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_status_shows_the_clock_domain_and_the_drift() {
    let backend = FakeBackend::new(FakeConfig {
        drive: FakeDrive::Manual,
        paths: vec![FakePath::loopback(0, 0, 2000)],
        drift_ppm: -50.0,
        drift_horizon_seconds: 120.0,
        ..FakeConfig::default()
    })
    .unwrap();
    // Debug builds run the DSP slower than real time; the lease is not under test.
    let (_h, ep) = rig_on(Arc::new(backend.clone()), |cfg| {
        cfg.lease_expiry = Duration::from_secs(60);
    });
    let c = client(&ep, "drift-rig-test").await;
    c.call(Command::SessionOpen {
        config: SessionConfig {
            sample_rate_hz: Some(48_000),
            buffer_frames: Some(256),
            loopback: Some(LoopbackRoute {
                output: 0,
                input: 0,
            }),
            ..session(vec![0, 1])
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
    // A session just opened has no minute of stamps: no figure for its clock yet.
    assert!(before.contains("\nsample clock measuring…\n"), "{before}");
    assert!(before.contains("\nsystem clock "), "{before}");

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
