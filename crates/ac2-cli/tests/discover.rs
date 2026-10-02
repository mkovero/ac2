//! `ac2 discover --json` against adverts on loopback (private mDNS port). The pairing column
//! comes from the key dir only: an advert whose fingerprint matches a pin reads as paired, a
//! pinned host advertising another fingerprint reads as a mismatch.

use std::net::{IpAddr, Ipv4Addr};

use ac2_cli::{Cli, Out, run_reporting};
use ac2_client::KeyDir;
use ac2_discovery::{Advert, Advertiser, Bind, Options};
use ac2_zmq::KeyPair;
use clap::Parser;
use serde_json::Value;

type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn advert(name: &str, key: &KeyPair) -> Advert {
    Advert {
        name: name.into(),
        version: "1.2.3".into(),
        proto: ac2_proto::PROTO_VERSION,
        fingerprint: key.public.fingerprint(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn discover_lists_rigs_with_pairing_status() -> R {
    let port = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?
        .local_addr()?
        .port();
    let opts = Options {
        mdns_port: port,
        loopback_only: true,
    };
    let lo = Bind::Addr(IpAddr::V4(Ipv4Addr::LOCALHOST));
    let paired = KeyPair::generate()?;
    let rekeyed = KeyPair::generate()?;
    let pinned_elsewhere = KeyPair::generate()?;
    let _a = Advertiser::start(&advert("paired rig", &paired), 47_900, &lo, &opts)?;
    let _b = Advertiser::start(&advert("rekeyed rig", &rekeyed), 47_910, &lo, &opts)?;

    let dir = tempfile::tempdir()?;
    let kd = KeyDir::new(dir.path());
    kd.pin_server("foh.example", paired.public)?;
    kd.pin_server("127.0.0.1", pinned_elsewhere.public)?;

    let key_dir = dir.path().to_string_lossy().into_owned();
    let port_s = port.to_string();
    let cli = Cli::try_parse_from([
        "ac2",
        "--json",
        "--key-dir",
        &key_dir,
        "discover",
        "--wait",
        "3s",
        "--loopback",
        "--mdns-port",
        &port_s,
    ])?;
    let (mut so, mut se) = (Vec::new(), Vec::new());
    let code = {
        let mut out = Out::new(cli.json, &mut so);
        run_reporting(&cli, &mut out, &mut se).await
    };
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&se));
    let v: Value = serde_json::from_slice(&so)?;
    let rigs = v.as_array().ok_or("not an array")?;
    if rigs.is_empty() {
        assert!(
            std::env::var_os("AC2_REQUIRE_MDNS").is_none_or(|v| v != "1"),
            "no mDNS answer on loopback"
        );
        eprintln!("skipped: no multicast on loopback here");
        return Ok(());
    }
    assert_eq!(rigs.len(), 2, "{v:#}");
    let by_name = |n: &str| rigs.iter().find(|r| r["name"] == n).cloned();
    let p = by_name("paired rig").ok_or("paired rig missing")?;
    assert_eq!(p["pairing"]["status"], "paired");
    assert_eq!(p["pairing"]["host"], "foh.example");
    // Connect under the name the key is pinned as.
    assert_eq!(p["remote"], "foh.example:47900");
    assert_eq!(p["fingerprint"], paired.public.fingerprint());
    assert_eq!(p["proto_compatible"], true);
    let r = by_name("rekeyed rig").ok_or("rekeyed rig missing")?;
    assert_eq!(r["pairing"]["status"], "mismatch");
    assert_eq!(
        r["pairing"]["pinned_fingerprint"],
        pinned_elsewhere.public.fingerprint()
    );
    assert_eq!(r["remote"], "127.0.0.1:47910");
    Ok(())
}
