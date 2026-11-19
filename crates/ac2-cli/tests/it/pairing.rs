//! What a remote client is told when it cannot get in: not paired at all (a local refusal,
//! before anything is sent), or paired but not authorized on the daemon (which looks like
//! silence, so the message names this client's fingerprint and where to authorize it).
#![allow(clippy::unwrap_used)]

use ac2_cli::Cli;
use ac2_zmq::PublicKey;
use ac2d::{BackendChoice, Daemon, DaemonConfig, Listen, NetworkSecurity};
use clap::Parser;

use ac2_client::Endpoints;

use super::common::{ac2 as on, cli as ac2};

#[tokio::test(flavor = "multi_thread")]
async fn unpaired_client_is_told_to_pair() {
    let keys = tempfile::tempdir().unwrap();
    let kd = keys.path().to_str().unwrap();
    let r = ac2(&["--key-dir", kd, "--remote", "pupu", "status"]).await;
    assert_eq!(r.code, 1);
    assert!(
        r.stderr
            .starts_with("ac2: not paired with pupu: this client has no key pair yet (no "),
        "{}",
        r.stderr
    );
    assert!(
        r.stderr
            .contains("; run `ac2 auth pair pupu --server-key <the key ac2d logs at startup>`"),
        "{}",
        r.stderr
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unauthorized_client_hears_its_fingerprint() {
    let dir = tempfile::tempdir().unwrap();
    let listen = Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    };
    let backend = ac2d::backend(BackendChoice::Fake).unwrap();
    let mut cfg = DaemonConfig::new(backend, listen, -10.0);
    cfg.network = ac2d::NetworkChoice::Fixed(ac2d::NetListen {
        bind: ac2d::NetBind::Addr {
            addr: std::net::Ipv4Addr::LOCALHOST.into(),
        },
        port: ac2d::NetPort::Any,
    });
    cfg.keys = Some(NetworkSecurity {
        server_key_file: dir.path().join("server.key"),
        authorized_clients_file: dir.path().join("authorized_clients"),
    });
    let h = Daemon::start(cfg).unwrap();
    let server: PublicKey = h.server_public_key().unwrap();
    let ac2d::NetStatus::Listening { ctrl, .. } = h.network_status() else {
        panic!("the network is not listening");
    };
    let port = ctrl.rsplit_once(':').unwrap().1.to_owned();

    let keys = tempfile::tempdir().unwrap();
    let kd = keys.path().to_str().unwrap();
    let pair = ac2(&[
        "--key-dir",
        kd,
        "--json",
        "auth",
        "pair",
        "127.0.0.1",
        "--server-key",
        &server.to_z85(),
    ])
    .await;
    assert_eq!(pair.code, 0, "{} {}", pair.stdout, pair.stderr);
    let paired: serde_json::Value = serde_json::from_str(&pair.stdout).unwrap();
    let fp = paired["client_fingerprint"].as_str().unwrap().to_owned();
    // A second client knocks too, so the right one of two is picked by fingerprint.
    let other_keys = tempfile::tempdir().unwrap();
    let okd = other_keys.path().to_str().unwrap();
    let pair = ac2(&[
        "--key-dir",
        okd,
        "--json",
        "auth",
        "pair",
        "127.0.0.1",
        "--server-key",
        &server.to_z85(),
    ])
    .await;
    assert_eq!(pair.code, 0, "{} {}", pair.stdout, pair.stderr);
    let paired: serde_json::Value = serde_json::from_str(&pair.stdout).unwrap();
    let other_fp = paired["client_fingerprint"].as_str().unwrap().to_owned();

    // Paired, but the daemon's authorized_clients is empty: the handshake is refused, and
    // said so, never taken for silence.
    let remote = format!("127.0.0.1:{port}");
    let r = ac2(&[
        "--key-dir",
        kd,
        "--remote",
        &remote,
        "--timeout",
        "200ms",
        "status",
    ])
    .await;
    assert_eq!(r.code, 1, "{}", r.stderr);
    assert!(
        r.stderr.starts_with(&format!(
            "ac2: the daemon has not authorized this client (fingerprint {fp})"
        )),
        "{}",
        r.stderr
    );
    assert!(
        r.stderr.contains(&format!(
            "run `ac2 server authorize <name> --refused {fp}` (or `--key '"
        )),
        "{}",
        r.stderr
    );
    for _ in 0..2 {
        let r = ac2(&[
            "--key-dir",
            okd,
            "--remote",
            &remote,
            "--timeout",
            "200ms",
            "status",
        ])
        .await;
        assert_eq!(r.code, 1, "{}", r.stderr);
    }

    // On the daemon's host, the refused key is authorized by the fingerprint it was shown
    // under, and the client gets in on its next try, no restart.
    let local = Endpoints {
        ctrl: h.ctrl_endpoint().to_owned(),
        data: h.data_endpoint().to_owned(),
    };
    let info = on(&local, true, &["server", "info"]).await;
    let listed = info.json(&[]);
    let refused: Vec<&str> = listed["network"]["refused"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["fingerprint"].as_str().unwrap())
        .collect();
    assert!(
        refused.contains(&fp.as_str()) && refused.contains(&other_fp.as_str()),
        "{listed}"
    );
    let text = on(&local, false, &["server", "info"]).await;
    assert!(
        text.stdout
            .contains(&format!("ac2 server authorize <name> --refused {fp}")),
        "{}",
        text.stdout
    );

    let wrong = on(
        &local,
        false,
        &["server", "authorize", "laptop", "--refused", "0000-0000"],
    )
    .await;
    assert_eq!(wrong.code, 1);
    // Each refused key listed once, however often it knocked.
    let (first, second) = (refused[0], refused[1]);
    assert_eq!(
        wrong.stderr,
        format!(
            "ac2: no refused key has fingerprint 0000-0000 (refused lately: {first}, {second}); \
             have the client try again, or give --key KEY (shown by `ac2 auth show` on the \
             client)\n"
        )
    );
    let args = ["server", "authorize", "laptop", "--refused", "0000-0000"];
    let wrong = on(&local, true, &args).await;
    assert_eq!(wrong.json(&args)["error"]["code"], "not_found");

    let args = [
        "server",
        "authorize",
        "laptop",
        "--refused",
        &fp.to_uppercase(),
    ];
    let ok = on(&local, true, &args).await;
    assert_eq!(ok.code, 0, "{}", ok.stderr);
    let a = ok.json(&args);
    assert_eq!(a["authorized"]["name"], "laptop");
    assert_eq!(a["authorized"]["fingerprint"], fp.as_str());
    assert_eq!(a["authorized"]["rig"], serde_json::Value::Null);
    let info = on(&local, true, &["server", "info"]).await.json(&[]);
    let refused = info["network"]["refused"].as_array().unwrap();
    assert!(
        refused
            .iter()
            .all(|r| r["fingerprint"] == other_fp.as_str())
            && !refused.is_empty(),
        "only the other client is still refused: {info}"
    );

    let r = ac2(&["--key-dir", kd, "--remote", &remote, "status"]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);

    // Over the network it is told it may not switch the network, and no refusals.
    let r = ac2(&["--key-dir", kd, "--remote", &remote, "server", "network"]).await;
    assert!(
        r.stdout
            .contains("\nswitch       only on the rig's own machine\n"),
        "{}",
        r.stdout
    );
    let r = ac2(&["--key-dir", kd, "--remote", &remote, "server", "info"]).await;
    assert!(
        r.stdout
            .contains("refused lately\n  listed on the rig's own host only\n"),
        "{}",
        r.stdout
    );
    let r = ac2(&[
        "--key-dir",
        kd,
        "--remote",
        &remote,
        "server",
        "authorize",
        "tablet",
        "--refused",
        &other_fp,
    ])
    .await;
    assert_eq!(r.code, 1, "{}", r.stderr);
    assert!(
        r.stderr.contains("refused only on its own host"),
        "{}",
        r.stderr
    );

    // Authorized, it may still not switch the network it came in on.
    let r = ac2(&[
        "--key-dir",
        kd,
        "--remote",
        &remote,
        "server",
        "network",
        "off",
    ])
    .await;
    assert_eq!(r.code, 1);
    assert_eq!(
        r.stderr,
        "ac2: refused: the network can only be turned on or off on the rig's own machine\n"
    );
    h.shutdown();
}

/// Z85 includes `-`, so about one daemon key in 85 starts with it; `--server-key` must still
/// take such a key as its value rather than as an unknown option.
#[test]
fn a_server_key_may_start_with_a_hyphen() {
    let key = "-b".to_string() + &"a".repeat(38);
    let cli = Cli::try_parse_from(["ac2", "auth", "pair", "pupu", "--server-key", key.as_str()]);
    assert!(cli.is_ok(), "{:?}", cli.err());
}
