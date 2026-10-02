//! Advertise and browse over real mDNS, confined to 127.0.0.1 on a private port so the test
//! neither needs nor disturbs a network. Some hosts (containers, CI sandboxes) have no
//! multicast on loopback; there the test reports a skip unless `AC2_REQUIRE_MDNS=1`, which CI
//! sets on the runners where it is known to work.

use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};

use ac2_discovery::{Advert, Advertiser, Bind, Browser, Options, RigTable, Update};

fn private_port() -> u16 {
    // A free UDP port right now; mdns-sd binds it with SO_REUSEADDR on both sides.
    std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|s| s.local_addr())
        .map_or(53_530, |a| a.port())
}

fn required() -> bool {
    std::env::var_os("AC2_REQUIRE_MDNS").is_some_and(|v| v == "1")
}

#[test]
fn advert_is_browsed_and_goodbye_removes_it() {
    let opts = Options {
        mdns_port: private_port(),
        loopback_only: true,
    };
    let advert = Advert {
        name: "loopback rig".into(),
        version: "9.9.9".into(),
        proto: 1,
        fingerprint: "0000-1111-2222-3333-4444".into(),
    };
    let browser = Browser::start(&opts).unwrap_or_else(|e| panic!("{e}"));
    let adv = Advertiser::start(
        &advert,
        47_999,
        &Bind::Addr(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        &opts,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let fullname = adv.fullname().to_owned();

    let mut table = RigTable::default();
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline && table.is_empty() {
        if let Some(u) = browser.next(Duration::from_millis(200)) {
            table.apply(u);
        }
    }
    if table.is_empty() {
        assert!(!required(), "no mDNS answer on loopback within 8 s");
        eprintln!("skipped: no multicast on loopback here (set AC2_REQUIRE_MDNS=1 to fail)");
        return;
    }
    let rigs = table.rigs();
    let rig = rigs[0];
    assert_eq!(rig.instance, fullname);
    assert_eq!(rig.advert, advert);
    assert_eq!(rig.port, 47_999);
    assert_eq!(rig.connect_host(), "127.0.0.1");

    drop(adv);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut removed = false;
    while Instant::now() < deadline && !removed {
        if let Some(u) = browser.next(Duration::from_millis(200)) {
            removed = matches!(&u, Update::Removed { instance } if *instance == fullname);
            table.apply(u);
        }
    }
    assert!(removed, "goodbye did not remove the rig");
    assert!(table.is_empty());
}
