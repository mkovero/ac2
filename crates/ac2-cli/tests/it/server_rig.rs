//! `ac2 server network` against a real daemon: shown, turned on and off live and kept in
//! the daemon's own settings file; refused while `ac2d --listen` fixes it, by a daemon with
//! no network to serve, and for a port whose data port cannot exist.
#![allow(clippy::unwrap_used)]

use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};

use ac2d::{NetBind, NetListen, NetPort, NetworkChoice, NetworkSecurity};
use serde_json::{Value, json};

use super::common::{ac2, fake_backend, rig, rig_on};

fn security(dir: &Path) -> NetworkSecurity {
    NetworkSecurity {
        server_key_file: dir.join("server.key"),
        authorized_clients_file: dir.join("authorized_clients"),
    }
}

/// A port whose next one looked free too: below every OS's ephemeral range, where a daemon
/// binding its local pair on port 0 never lands, and a different start per test process.
/// Only a guess: the caller retries another when the daemon cannot bind it.
fn free_pair() -> u16 {
    const FIRST: u16 = 20_000;
    const PAIRS: u16 = 6_000;
    static NEXT: AtomicU16 = AtomicU16::new(0);
    let start = u16::try_from(std::process::id() % u32::from(PAIRS)).unwrap();
    let binds = |p: u16| {
        std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, p)).is_ok()
            && std::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, p)).is_ok()
    };
    loop {
        let k = (start + NEXT.fetch_add(1, Ordering::Relaxed)) % PAIRS;
        let p = FIRST + 2 * k;
        if binds(p) && binds(p + 1) {
            return p;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_network_is_shown_turned_on_and_off_and_kept() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("network.json");
    let (h, ep) = rig_on(fake_backend(), |c| {
        c.network = NetworkChoice::FromSettings {
            store: Some(store.clone()),
        };
        c.keys = Some(security(dir.path()));
    });

    let shown = ac2(&ep, true, &["server", "network"]).await;
    assert_eq!(shown.code, 0, "{}", shown.stderr);
    let v = shown.json(&[]);
    assert_eq!(v["setting"]["type"], "host");
    assert_eq!(v["setting"]["setting"]["type"], "off");
    assert_eq!(v["setting"]["setting"]["kept"], Value::Null);
    assert_eq!(v["toggle"], json!({ "type": "allowed" }));
    assert_eq!(
        v["status"],
        json!({ "type": "host", "status": { "type": "off" } })
    );
    assert_eq!(v["server_key"], Value::Null);
    let text = ac2(&ep, false, &["server", "network"]).await.stdout;
    assert!(text.starts_with("network      off; "), "{text}");
    assert!(
        text.contains("status       off\nserver key   none yet"),
        "{text}"
    );

    // On at a pair that looked free; another process may take it first, and then the
    // daemon waits on it, so another pair.
    let mut on = None;
    'pairs: for _ in 0..5 {
        let port = free_pair();
        let p = port.to_string();
        let args = [
            "server",
            "network",
            "on",
            "--addr",
            "127.0.0.1",
            "--port",
            &p,
        ];
        let r = ac2(&ep, true, &args).await;
        assert_eq!(r.code, 0, "{}", r.stderr);
        let v = r.json(&args);
        assert_eq!(
            v["setting"]["setting"],
            json!({
                "type": "on",
                "choice": {
                    "bind": { "type": "addr", "addr": "127.0.0.1" },
                    "port": port,
                },
                "saved": { "type": "saved" },
            })
        );
        assert!(
            v["fingerprint"].is_string(),
            "the key is made on the first enable: {v}"
        );
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(3) {
            let v = ac2(&ep, true, &["server", "network"]).await.json(&[]);
            if v["status"]["status"]["type"] == "listening" {
                on = Some(port);
                break 'pairs;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    let port = on.expect("no free port pair");
    let text = ac2(&ep, false, &["server", "network"]).await.stdout;
    assert!(
        text.starts_with(&format!(
            "network      on: address 127.0.0.1, port {port} (data {}); kept across restarts\n\
             status       listening on tcp://127.0.0.1:{port} (data tcp://127.0.0.1:{})\n",
            port + 1,
            port + 1
        )),
        "{text}"
    );
    let kept: Value = serde_json::from_str(&std::fs::read_to_string(&store).unwrap()).unwrap();
    assert_eq!(kept["enabled"], true);

    // A data port past 65535 cannot exist: refused in the daemon's words, nothing changed.
    let bad = ac2(&ep, false, &["server", "network", "on", "--port", "65535"]).await;
    assert_eq!(bad.code, 1);
    assert!(
        bad.stderr.starts_with("ac2: invalid: port 65535: "),
        "{}",
        bad.stderr
    );

    let off = ac2(&ep, true, &["server", "network", "off"]).await;
    assert_eq!(off.code, 0, "{}", off.stderr);
    let v = off.json(&[]);
    assert_eq!(
        v["setting"]["setting"],
        json!({
            "type": "off",
            "kept": {
                "bind": { "type": "addr", "addr": "127.0.0.1" },
                "port": port,
            },
            "saved": { "type": "saved" },
        })
    );
    assert_eq!(v["status"]["status"]["type"], "off");
    let text = ac2(&ep, false, &["server", "network", "off"]).await.stdout;
    assert!(
        text.starts_with(&format!(
            "network      off (on again at address 127.0.0.1, port {port} (data {})); \
             kept across restarts\nstatus       off\n",
            port + 1
        )),
        "{text}"
    );
    assert!(!text.contains("switch"), "allowed: nothing said: {text}");
    h.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_network_fixed_by_listen_is_shown_and_not_switched() {
    let dir = tempfile::tempdir().unwrap();
    let (h, ep) = rig_on(fake_backend(), |c| {
        c.network = NetworkChoice::Fixed(NetListen {
            bind: NetBind::Addr {
                addr: Ipv4Addr::LOCALHOST.into(),
            },
            port: NetPort::Any,
        });
        c.keys = Some(security(dir.path()));
    });
    let text = ac2(&ep, false, &["server", "network"]).await.stdout;
    assert!(
        text.starts_with("network      fixed by ac2d --listen tcp://"),
        "{text}"
    );
    assert!(
        text.contains(
            "\nswitch       fixed by ac2d --listen: restart ac2d without it to switch here\n"
        ),
        "{text}"
    );
    let v = ac2(&ep, true, &["server", "network"]).await.json(&[]);
    assert_eq!(v["setting"]["setting"]["type"], "fixed");
    assert_eq!(v["toggle"], json!({ "type": "fixed" }));

    for args in [
        &["server", "network", "off"][..],
        &["server", "network", "on", "--all"],
    ] {
        let r = ac2(&ep, false, args).await;
        assert_eq!(r.code, 1, "{args:?}");
        assert!(
            r.stderr
                .starts_with("ac2: refused: the network is fixed by ac2d --listen ")
                && r.stderr.contains("restart ac2d without --listen"),
            "{}",
            r.stderr
        );
    }
    let r = ac2(&ep, true, &["server", "network", "off"]).await;
    assert_eq!(r.code, 1);
    assert_eq!(
        super::common::document(&r.stdout)["error"]["code"],
        "refused"
    );
    h.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_with_no_network_says_so() {
    let (h, ep) = rig();
    let r = ac2(&ep, false, &["server", "network", "on"]).await;
    assert_eq!(r.code, 1);
    assert!(
        r.stderr
            .starts_with("ac2: unsupported: this daemon serves in-process only"),
        "{}",
        r.stderr
    );
    let v = ac2(&ep, true, &["server", "network"]).await.json(&[]);
    assert_eq!(
        v["toggle"],
        json!({ "type": "unavailable", "reason": "in_process" })
    );
    let info = ac2(&ep, false, &["server", "info"]).await;
    assert_eq!(info.code, 0, "{}", info.stderr);
    assert!(
        info.stdout
            .contains("switch       unavailable: this daemon serves in-process only\n"),
        "{}",
        info.stdout
    );
    assert!(
        info.stdout
            .contains("authorized\n  none\nrefused lately\n  none\n"),
        "{}",
        info.stdout
    );
    h.shutdown();
}
