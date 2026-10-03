//! mDNS advert: a network-mode daemon names the rig, its versions and its key fingerprint;
//! a local daemon advertises nothing. Runs on loopback with a private mDNS port; where the
//! host has no multicast on loopback the test reports a skip unless `AC2_REQUIRE_MDNS=1`.
#![allow(clippy::unwrap_used)]

mod common;

use std::time::{Duration, Instant};

use ac2d::{Advertise, Daemon, Listen, NetworkSecurity};
use common::*;

fn mdns_opts() -> ac2_discovery::Options {
    let port = std::net::UdpSocket::bind(("127.0.0.1", 0))
        .and_then(|s| s.local_addr())
        .map_or(53_531, |a| a.port());
    ac2_discovery::Options {
        mdns_port: port,
        loopback_only: true,
    }
}

#[test]
fn network_mode_advertises_fingerprint_local_mode_does_not() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let mdns = mdns_opts();
    let browser = ac2_discovery::Browser::start(&mdns).unwrap();

    let local = Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    };
    let mut cfg = config(manual_rig(), local);
    cfg.advertise = Some(Advertise {
        name: "local rig".into(),
        mdns: mdns.clone(),
    });
    let hl = Daemon::start(cfg).unwrap();
    assert_eq!(hl.advertised_as(), None, "local mode is not on the network");

    let listen = Listen::Network {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
        security: NetworkSecurity {
            server_key_file: dir.path().join("server.key"),
            authorized_clients_file: dir.path().join("authorized_clients"),
        },
    };
    let mut cfg = config(manual_rig(), listen);
    cfg.advertise = Some(Advertise {
        name: "stage.rig".into(),
        mdns: mdns.clone(),
    });
    let h = Daemon::start(cfg).unwrap();
    let fullname = h.advertised_as().expect("advert registered").to_owned();
    let port: u16 = h
        .ctrl_endpoint()
        .rsplit_once(':')
        .and_then(|(_, p)| p.parse().ok())
        .unwrap();

    let mut table = ac2_discovery::RigTable::default();
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline && table.is_empty() {
        if let Some(u) = browser.next(Duration::from_millis(200)) {
            table.apply(u);
        }
    }
    if table.is_empty() {
        assert!(
            std::env::var_os("AC2_REQUIRE_MDNS").is_none_or(|v| v != "1"),
            "no mDNS answer on loopback within 8 s"
        );
        eprintln!("skipped: no multicast on loopback here (AC2_REQUIRE_MDNS=1 makes this fail)");
        return;
    }
    let rigs = table.rigs();
    assert_eq!(rigs.len(), 1, "only the network-mode daemon advertises");
    let rig = rigs[0];
    assert_eq!(rig.instance, fullname);
    // Dots would read as DNS label separators; the name is advertised without them.
    assert_eq!(rig.advert.name, "stage-rig");
    assert_eq!(rig.advert.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(rig.advert.proto, ac2_proto::PROTO_VERSION);
    assert_eq!(
        rig.advert.fingerprint,
        h.server_public_key().unwrap().fingerprint()
    );
    assert_eq!(rig.port, port);
    assert_eq!(rig.connect_host(), "127.0.0.1");

    // Served the way `ac2d` serves (blocked in `wait`), the rig still answers a browser
    // that asks only now: the advert lives as long as the daemon does.
    let stopper = h.stopper();
    let serving = std::thread::spawn(move || h.wait());
    std::thread::sleep(Duration::from_millis(500));
    let late = ac2_discovery::Browser::start(&mdns).unwrap();
    let mut table = ac2_discovery::RigTable::default();
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline && table.is_empty() {
        if let Some(u) = late.next(Duration::from_millis(200)) {
            table.apply(u);
        }
    }
    assert_eq!(
        table.rigs().first().map(|r| r.instance.as_str()),
        Some(fullname.as_str()),
        "a serving daemon answers queries"
    );
    let mut asked = Vec::new();
    late.queried(&mut asked);
    assert!(
        asked.iter().any(|q| q.name == "lo" || q.addr.is_loopback())
            && asked.iter().all(|q| q.error.is_none()),
        "the browser asked on loopback itself: {asked:?}"
    );
    stopper.stop();
    serving.join().unwrap();
    hl.shutdown();
}
