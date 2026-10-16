//! `ac2 artnet …`, the `--*-dmx` levels of `ac2 cue` and the Art-Net lines of `ac2 rig
//! status` against a real daemon on the simulated rig, with a UDP socket on the loopback
//! interface as the node.
#![allow(clippy::unwrap_used)]

use std::net::UdpSocket;
use std::time::{Duration, Instant};

use ac2_client::Endpoints;
use ac2d::{BackendChoice, Daemon, DaemonConfig, Listen};

use super::osc_rig::{ac2, ok};

/// The next frame's 512 channels that `want` accepts.
fn frame_until(rx: &UdpSocket, want: impl Fn(&[u8]) -> bool) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut buf = [0u8; 1024];
    loop {
        assert!(Instant::now() < deadline, "no such frame");
        let Ok(n) = rx.recv(&mut buf) else {
            continue;
        };
        assert_eq!(n, 530);
        let data = &buf[18..n];
        if want(data) {
            return data.to_vec();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn artnet_target_test_cue_levels_and_status() {
    let backend = ac2d::backend(BackendChoice::Fake).unwrap();
    let listen = Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    };
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = DaemonConfig::new(backend, listen, -10.0);
    cfg.rig_settings = Some(dir.path().join("rig.json"));
    let h = Daemon::start(cfg).unwrap();
    let ep = Endpoints {
        ctrl: h.ctrl_endpoint().to_owned(),
        data: h.data_endpoint().to_owned(),
    };
    let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
    rx.set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let addr = rx.local_addr().unwrap().to_string();

    // The universe is required unless off; then the target is set and kept.
    let (code, e, _) = ac2(&ep, &["artnet", "target", &addr]).await;
    assert_eq!(code, 1, "{e}");
    assert_eq!(e["error"]["code"], "usage", "{e}");
    let (rig, _) = ok(&ep, &["artnet", "target", &addr, "0.1.2"]).await;
    assert_eq!(rig["artnet_target"]["addr"], addr.as_str(), "{rig}");
    assert_eq!(rig["artnet_target"]["universe"], 18, "{rig}");
    let saved = std::fs::read_to_string(dir.path().join("rig.json")).unwrap();
    assert!(saved.contains(&addr), "{saved}");
    assert_eq!(rig["artnet_fade"], 1.0, "{rig}");

    // The fade is kept by the rig, and status says it.
    let (rig, _) = ok(&ep, &["artnet", "fade", "250ms"]).await;
    assert_eq!(rig["artnet_fade"], 0.25, "{rig}");
    let saved = std::fs::read_to_string(dir.path().join("rig.json")).unwrap();
    assert!(saved.contains("\"artnet_fade\": 0.25"), "{saved}");

    // A test frame.
    let (t, _) = ok(&ep, &["artnet", "test", "12=255", "13=40"]).await;
    assert_eq!(t["sent"][0]["channel"], 12, "{t}");
    let f = frame_until(&rx, |d| d[11] == 255);
    assert_eq!(f[12], 40);

    // A meter's fault levels (no session: the rig cannot judge) go out and show in status.
    ok(
        &ep,
        &["meas", "new", "spl", "--input", "1", "--name", "Stage"],
    )
    .await;
    let (q, _) = ok(
        &ep,
        &[
            "cue",
            "Stage",
            "meter",
            "--fault-dmx",
            "20=255 21=7",
            "--ok-dmx",
            "22=255",
            "--over",
            "/light/red",
        ],
    )
    .await;
    assert_eq!(q["cue"]["fault"]["dmx"][1]["channel"], 21, "{q}");
    assert_eq!(q["cue"]["fault"]["osc"], serde_json::Value::Null, "{q}");
    assert_eq!(q["cue"]["over"]["osc"]["path"], "/light/red", "{q}");
    let f = frame_until(&rx, |d| d[19] == 255);
    assert_eq!((f[20], f[21], f[11]), (7, 0, 0));

    // Levels off for one state keep the others.
    let (q, _) = ok(&ep, &["cue", "Stage", "meter", "--ok-dmx", "off"]).await;
    assert!(q["cue"]["ok"].is_null(), "{q}");
    assert_eq!(q["cue"]["fault"]["dmx"][0]["value"], 255, "{q}");

    let deadline = Instant::now() + Duration::from_secs(10);
    let st = loop {
        let (st, _) = ok(&ep, &["rig", "status"]).await;
        if st["dmx"].as_array().is_some_and(|d| d.len() == 2) {
            break st;
        }
        assert!(Instant::now() < deadline, "{st}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(st["artnet_target"]["addr"], addr.as_str(), "{st}");
    assert!(st["artnet"]["sent"].as_u64().unwrap() >= 2, "{st}");
    assert_eq!(st["dmx"][0]["channel"], 20, "{st}");
    assert_eq!(st["artnet_fade"], 0.25, "{st}");

    let (rig, _) = ok(&ep, &["artnet", "target", "off"]).await;
    assert!(rig["artnet_target"].is_null(), "{rig}");
    h.shutdown();
}
