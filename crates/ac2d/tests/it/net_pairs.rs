//! The two socket pairs: the local pair served for the daemon's whole life, the network
//! pair attached beside it, retried on its own whatever stops it, and told apart by the
//! socket a request came in on, never by a name.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ac2_proto::model::SessionRef;
use ac2_proto::{Command, ErrorCode};
use ac2_zmq::{AuthorizedKeys, Context, CurveClient, KeyPair};
use ac2d::{
    Backoff, Daemon, Handle, Interfaces, NetBind, NetInterface, NetListen, NetPort, NetReason,
    NetStatus, NetworkChoice, NetworkSecurity,
};
use common::*;

const T: Duration = Duration::from_secs(10);

/// Retries fast enough for a quick test.
const FAST: Backoff = Backoff {
    first: Duration::from_millis(50),
    max: Duration::from_millis(200),
};

/// A key file with `laptop` authorized, in `dir`.
fn security(dir: &std::path::Path, laptop: &KeyPair) -> NetworkSecurity {
    let mut keys = AuthorizedKeys::new();
    keys.insert("laptop", laptop.public).unwrap();
    let authorized = dir.join("authorized_clients");
    keys.save(&authorized).unwrap();
    NetworkSecurity {
        server_key_file: dir.join("server.key"),
        authorized_clients_file: authorized,
    }
}

fn curve(h: &Handle, keys: &KeyPair) -> CurveClient {
    CurveClient {
        keys: keys.clone(),
        server_key: h.server_public_key().unwrap(),
    }
}

fn wait_status(h: &Handle, what: &str, ok: impl Fn(&NetStatus) -> bool) -> NetStatus {
    let t0 = Instant::now();
    loop {
        let s = h.network_status();
        if ok(&s) {
            return s;
        }
        assert!(t0.elapsed() < T, "{what}: still {s:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn listening(s: &NetStatus) -> bool {
    matches!(s, NetStatus::Listening { .. })
}

#[test]
fn a_path_is_taken_on_the_local_pair_and_refused_on_the_network_one() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let laptop = KeyPair::generate().unwrap();
    let mut cfg = net_config(manual_rig(), security(dir.path(), &laptop));
    cfg.session_dir = dir.path().join("sessions");
    let h = Daemon::start(cfg).unwrap();
    let ctx = Context::new().unwrap();
    let path = SessionRef::Path {
        path: dir.path().join("show").to_string_lossy().into_owned(),
    };
    let save = || Command::FileSave {
        session: path.clone(),
        overwrite: true,
    };

    let mut remote = Client::connect_with(&ctx, &net_endpoints(&h).0, Some(&curve(&h, &laptop)));
    assert_eq!(remote.call(save()).unwrap_err().code, ErrorCode::Refused);

    // The network listening changes nothing for this machine's own clients.
    let mut local = Client::connect(&ctx, h.ctrl_endpoint());
    local.ok(save());
    assert!(dir.path().join("show").is_dir());
    h.shutdown();
}

#[test]
fn a_local_looking_name_is_refused_but_one_already_on_file_still_connects() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let laptop = KeyPair::generate().unwrap();
    let old = KeyPair::generate().unwrap();
    let sec = security(dir.path(), &laptop);
    let mut keys = AuthorizedKeys::load(&sec.authorized_clients_file).unwrap();
    keys.insert("local-x", old.public).unwrap();
    keys.save(&sec.authorized_clients_file).unwrap();
    let h = Daemon::start(net_config(manual_rig(), sec)).unwrap();
    let ctx = Context::new().unwrap();
    let net = net_endpoints(&h).0;

    let mut c = Client::connect_with(&ctx, &net, Some(&curve(&h, &laptop)));
    let e = c
        .call(Command::ServerAuthorize {
            name: "local-y".into(),
            key: KeyPair::generate().unwrap().public.to_z85(),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");

    // The entry on file is a network client like any other: it connects, and a path is
    // still refused to it.
    let mut x = Client::connect_with(&ctx, &net, Some(&curve(&h, &old)));
    let e = x
        .call(Command::FileSave {
            session: SessionRef::Path {
                path: dir.path().join("x").to_string_lossy().into_owned(),
            },
            overwrite: true,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    h.shutdown();
}

#[test]
fn a_dead_zap_handler_closes_the_network_alone_and_it_comes_back() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let laptop = KeyPair::generate().unwrap();
    let mut cfg = net_config(manual_rig(), security(dir.path(), &laptop));
    // Long enough a retry delay for `Failed` to be seen before the rebind.
    cfg.net_retry = Backoff {
        first: Duration::from_millis(500),
        max: Duration::from_secs(1),
    };
    let h = Daemon::start(cfg).unwrap();
    let ctx = Context::new().unwrap();
    let mut local = Client::connect(&ctx, h.ctrl_endpoint());
    local.ok(Command::ServerInfo);
    let before = net_endpoints(&h);

    h.fail_zap_handler().unwrap();
    wait_status(&h, "the pair failed", |s| {
        *s == NetStatus::Failed {
            reason: NetReason::ZapFailed,
        }
    });
    local.ok(Command::ServerInfo);
    wait_status(
        &h,
        "the pair rebound",
        |s| matches!(s, NetStatus::Listening { ctrl, .. } if *ctrl != before.0),
    );
    let mut remote = Client::connect_with(&ctx, &net_endpoints(&h).0, Some(&curve(&h, &laptop)));
    remote.ok(Command::ServerInfo);
    h.shutdown();
}

#[test]
fn an_interface_is_waited_for_and_followed_while_it_comes_and_goes() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let laptop = KeyPair::generate().unwrap();
    let present: Arc<Mutex<Vec<NetInterface>>> = Arc::default();
    let lo = NetInterface {
        name: "stage0".into(),
        addrs: vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
        up: true,
    };
    let mut cfg = net_config(manual_rig(), security(dir.path(), &laptop));
    cfg.network = NetworkChoice::Fixed(NetListen {
        bind: NetBind::Interface("stage0".into()),
        port: NetPort::Any,
    });
    let list = Arc::clone(&present);
    cfg.interfaces = Interfaces::new(move || list.lock().unwrap().clone());
    cfg.net_retry = FAST;
    let h = Daemon::start(cfg).unwrap();
    assert_eq!(
        h.network_status(),
        NetStatus::Waiting {
            reason: NetReason::InterfaceMissing
        }
    );
    let ctx = Context::new().unwrap();
    Client::connect(&ctx, h.ctrl_endpoint()).ok(Command::ServerInfo);

    present.lock().unwrap().push(lo);
    wait_status(&h, "the interface came", listening);
    Client::connect_with(&ctx, &net_endpoints(&h).0, Some(&curve(&h, &laptop)))
        .ok(Command::ServerInfo);

    // Its address went away while bound: the pair is let go until it is back.
    present.lock().unwrap()[0].addrs.clear();
    wait_status(&h, "the address went", |s| {
        matches!(s, NetStatus::Waiting { .. })
    });
    Client::connect(&ctx, h.ctrl_endpoint()).ok(Command::ServerInfo);
    present.lock().unwrap()[0]
        .addrs
        .push(IpAddr::V4(Ipv4Addr::LOCALHOST));
    wait_status(&h, "the address came back", listening);
    Client::connect_with(&ctx, &net_endpoints(&h).0, Some(&curve(&h, &laptop)))
        .ok(Command::ServerInfo);
    h.shutdown();
}

#[test]
fn a_network_port_on_the_local_ones_is_refused_at_the_command_line() {
    let dir = tempfile::tempdir().unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_ac2d"))
        .args([
            "--backend",
            "fake",
            "--ctrl",
            "tcp://127.0.0.1:47950",
            "--data",
            "tcp://127.0.0.1:47951",
            "--listen",
            "tcp://127.0.0.1:47949",
            // Should the clash go unnoticed, the daemon must not outlive the test.
            "--stop-on-stdin-eof",
        ])
        .env("AC2_RUNTIME_DIR", dir.path().join("run"))
        .env("AC2_CONFIG_DIR", dir.path().join("config"))
        .env("AC2_LOG_DIR", dir.path().join("logs"))
        .env("AC2_SESSION_DIR", dir.path().join("sessions"))
        .env("HOME", dir.path().join("home"))
        .env("XDG_CONFIG_HOME", dir.path().join("xdg/config"))
        .env("XDG_DATA_HOME", dir.path().join("xdg/data"))
        .env("XDG_STATE_HOME", dir.path().join("xdg/state"))
        .env("XDG_RUNTIME_DIR", dir.path().join("xdg/runtime"))
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(i32::from(ac2d::instance::exit::SETUP)),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
