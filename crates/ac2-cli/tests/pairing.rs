//! What a remote client is told when it cannot get in: not paired at all (a local refusal,
//! before anything is sent), or paired but not authorized on the daemon (which looks like
//! silence, so the message names this client's fingerprint and where to authorize it).
#![allow(clippy::unwrap_used)]

use ac2_cli::{Cli, Out, run_reporting};
use ac2_zmq::PublicKey;
use ac2d::{BackendChoice, Daemon, DaemonConfig, Listen, NetworkSecurity};
use clap::Parser;

struct Run {
    code: u8,
    stdout: String,
    stderr: String,
}

async fn ac2(args: &[&str]) -> Run {
    let cli = Cli::try_parse_from(std::iter::once("ac2").chain(args.iter().copied())).unwrap();
    let (mut so, mut se) = (Vec::new(), Vec::new());
    let code = {
        let mut out = Out::new(cli.json, &mut so);
        run_reporting(&cli, &mut out, &mut se).await
    };
    Run {
        code,
        stdout: String::from_utf8(so).unwrap(),
        stderr: String::from_utf8(se).unwrap(),
    }
}

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
    let listen = Listen::Network {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
        security: NetworkSecurity {
            server_key_file: dir.path().join("server.key"),
            authorized_clients_file: dir.path().join("authorized_clients"),
        },
    };
    let backend = ac2d::backend(BackendChoice::Fake).unwrap();
    let h = Daemon::start(DaemonConfig::new(backend, listen, -10.0)).unwrap();
    let server: PublicKey = h.server_public_key().unwrap();
    let port = h.ctrl_endpoint().rsplit_once(':').unwrap().1.to_owned();

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

    // Paired, but the daemon's authorized_clients is empty: the handshake is refused.
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
    assert_eq!(r.code, 3, "{}", r.stderr);
    assert!(
        r.stderr.starts_with(&format!(
            "ac2: daemon at {remote} is not responding — or this client is not authorized on \
             it (this client's fingerprint: {fp};"
        )),
        "{}",
        r.stderr
    );
    assert!(
        r.stderr.contains("authorized_clients") && r.stderr.contains("restart ac2d"),
        "{}",
        r.stderr
    );
    h.shutdown();
}
