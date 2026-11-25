//! The audit trail outlives whatever a client makes the daemon log: eight `hello`s as large
//! as a ctrl message may be leave an earlier audit line readable, and a newline in a client
//! string cannot start a line of its own. Its own test binary because it installs the
//! global log subscriber, files and all.
#![allow(clippy::unwrap_used)]

#[path = "it/common/mod.rs"]
mod common;

use ac2_proto::model::ClientRole;
use ac2_proto::{Command, MAX_CTRL_BYTES};
use ac2_zmq::{AuthorizedKeys, CurveClient, KeyPair, Role};
use ac2d::{Daemon, NetworkSecurity};
use common::*;

#[test]
fn audit_lines_outlive_floods_and_forged_lines() {
    let logs = tempfile::tempdir().unwrap();
    let (general, audit) = ac2_paths::log::init_with(
        Some(logs.path()),
        Some(ac2_paths::log::AuditLog {
            dir: logs.path(),
            target: ac2d::AUDIT_TARGET,
        }),
        "ac2d",
    );
    let (general, audit) = (general.unwrap(), audit.unwrap());

    let dir = tempfile::tempdir().unwrap();
    let laptop = KeyPair::generate().unwrap();
    let tablet = KeyPair::generate().unwrap();
    let mut keys = AuthorizedKeys::new();
    keys.insert("laptop", laptop.public, Role::Operator)
        .unwrap();
    let security = NetworkSecurity {
        server_key_file: dir.path().join("server.key"),
        authorized_clients_file: dir.path().join("authorized_clients"),
    };
    keys.save(&security.authorized_clients_file).unwrap();
    let mut cfg = net_config(manual_rig(), security);
    cfg.session_dir = dir.path().join("sessions");
    let h = Daemon::start(cfg).unwrap();

    let mut host = Client::connect(h.context(), h.ctrl_endpoint());
    host.ok(Command::AuthAuthorize {
        name: "tablet".into(),
        key: tablet.public.to_z85(),
        role: ClientRole::Viewer,
    });

    let ep = net_endpoints(&h).0;
    let curve = CurveClient {
        keys: laptop.clone(),
        server_key: h.server_public_key().unwrap(),
    };
    let mut c = Client::connect_with(h.context(), ep.as_str(), Some(&curve));
    let big = "x".repeat(MAX_CTRL_BYTES - 4096);
    for _ in 0..8 {
        c.welcome(Command::Hello {
            client: big.clone(),
        });
    }
    c.welcome(Command::Hello {
        client: "app\nFORGED tablet revoked by laptop".into(),
    });
    // A request after the last hello: everything before it has been logged.
    c.server(Command::ServerInfo);
    h.shutdown();

    let audit = std::fs::read_to_string(&audit).unwrap();
    assert!(
        audit
            .lines()
            .any(|l| l.contains("\"tablet\"") && l.contains("authorized by")),
        "{audit}"
    );
    let general = std::fs::read_to_string(&general).unwrap();
    assert!(general.len() < 1 << 20, "{} bytes", general.len());
    for text in [&audit, &general] {
        assert!(!text.lines().any(|l| l.starts_with("FORGED")), "{text}");
    }
    assert!(
        general.contains("app?FORGED"),
        "the hello is logged on one line"
    );
}
