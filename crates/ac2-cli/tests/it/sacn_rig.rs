//! `ac2 sacn target`, `ac2 artnet test` reaching both outputs and the DMX lines of `ac2 rig
//! status`, against a real daemon on the simulated rig. Its sACN goes to a UDP socket on the
//! loopback interface at a port the test picks; the daemon's Art-Net socket is on the
//! loopback interface too ([`ac2d::DmxPorts::LOOPBACK`]).
#![allow(clippy::unwrap_used)]

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use super::common::{fake_backend, rig_on, run_plain};
use super::osc_rig::{ac2, ok};

/// The next E1.31 data packet that `want` accepts.
fn packet_until(rx: &UdpSocket, want: impl Fn(&[u8]) -> bool) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut buf = [0u8; 1024];
    loop {
        assert!(Instant::now() < deadline, "no such packet");
        let Ok(n) = rx.recv(&mut buf) else {
            continue;
        };
        assert_eq!(n, 638);
        if want(&buf[..n]) {
            return buf[..n].to_vec();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sacn_target_test_and_status() {
    let dir = tempfile::tempdir().unwrap();
    let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
    rx.set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let port = rx.local_addr().unwrap().port();
    let (h, ep) = rig_on(fake_backend(), |cfg| {
        cfg.rig_settings = Some(dir.path().join("rig.json"));
        cfg.dmx.sacn_port = port;
    });

    // What is missing is said before anything is sent.
    for bad in [
        &["sacn", "target", "127.0.0.1"][..],
        &["sacn", "target", "multicast", "1"],
        &["sacn", "target", "127.0.0.1", "1", "--via", "127.0.0.1"],
        &["sacn", "target", "off", "1"],
    ] {
        let (code, e, _) = ac2(&ep, bad).await;
        assert_eq!(
            (code, e["error"]["code"].as_str()),
            (1, Some("usage")),
            "{bad:?} {e}"
        );
    }
    // The daemon refuses a group as a receiver.
    let (code, e, _) = ac2(&ep, &["sacn", "target", "239.255.0.1", "1"]).await;
    assert_eq!(
        (code, e["error"]["code"].as_str()),
        (1, Some("invalid")),
        "{e}"
    );

    let (rig, _) = ok(
        &ep,
        &["sacn", "target", "127.0.0.1", "7", "--priority", "150"],
    )
    .await;
    assert_eq!(rig["sacn_target"]["dest"]["addr"], "127.0.0.1", "{rig}");
    assert_eq!(rig["sacn_target"]["universe"], 7, "{rig}");
    assert_eq!(rig["sacn_target"]["priority"], 150, "{rig}");
    let saved = std::fs::read_to_string(dir.path().join("rig.json")).unwrap();
    assert!(saved.contains("\"sacn_target\""), "{saved}");
    let p = packet_until(&rx, |_| true);
    assert_eq!((p[108], p[113], p[114]), (150, 0, 7));

    // A test frame with only sACN set reaches it.
    let (t, _) = ok(&ep, &["artnet", "test", "12=255", "13=40"]).await;
    assert_eq!(t["sent"][0]["channel"], 12, "{t}");
    let p = packet_until(&rx, |p| p[125 + 12] == 255);
    assert_eq!(p[125 + 13], 40);

    let deadline = Instant::now() + Duration::from_secs(10);
    let text = loop {
        let (_, text) = run_plain(&ep, &["rig", "status"]).await;
        if !text.contains("sACN target: 127.0.0.1 universe 7 priority 150\nsent: 0\n") {
            break text;
        }
        assert!(Instant::now() < deadline, "{text}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(
        text.contains("Art-Net target: off\n")
            && text.contains("sACN target: 127.0.0.1 universe 7 priority 150\n")
            && text.contains("DMX fade: 1 s\n"),
        "{text}"
    );

    // Off ends the stream.
    let (rig, _) = ok(&ep, &["sacn", "target", "off"]).await;
    assert!(rig["sacn_target"].is_null(), "{rig}");
    packet_until(&rx, |p| p[112] & 0x40 != 0);
    h.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_test_frame_one_output_did_not_send_is_a_warning() {
    // Linux binding an address no interface has, which this test needs to fail.
    if cfg!(target_os = "linux")
        && std::fs::read_to_string("/proc/sys/net/ipv4/ip_nonlocal_bind")
            .is_ok_and(|v| v.trim() == "1")
    {
        eprintln!("skipped: net.ipv4.ip_nonlocal_bind is 1, so a bind to 192.0.2.77 opens");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
    rx.set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let port = rx.local_addr().unwrap().port();
    let (h, ep) = rig_on(fake_backend(), |cfg| {
        cfg.rig_settings = Some(dir.path().join("rig.json"));
        cfg.dmx.sacn_port = port;
        // An address no interface of this host has (TEST-NET-1): no Art-Net socket opens,
        // so Art-Net sends nothing while sACN does.
        cfg.dmx.artnet_bind = SocketAddrV4::new(Ipv4Addr::new(192, 0, 2, 77), 0);
    });
    ok(&ep, &["sacn", "target", "127.0.0.1", "7"]).await;
    let node = UdpSocket::bind("127.0.0.1:0").unwrap();
    let to = node.local_addr().unwrap().to_string();
    let (_, err) = ok(&ep, &["artnet", "target", &to, "1"]).await;
    assert!(err.contains("no Art-Net socket could be opened"), "{err}");
    let (t, err) = ok(&ep, &["artnet", "test", "12=255"]).await;
    assert_eq!(t["sent"][0]["channel"], 12, "{t}");
    assert!(
        err.contains("the test frame did not go out on Art-Net"),
        "{err}"
    );
    packet_until(&rx, |p| p[125 + 12] == 255);
    h.shutdown();
}
