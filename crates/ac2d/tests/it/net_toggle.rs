//! The network turned on and off live (`server.network`) on a daemon that keeps the setting
//! in its own file: only a client on the rig's own host may switch it, the setting outlives
//! a restart, a damaged file means off, and turning off tells the network's apps before it
//! cuts them, while their lease runs out as on any loss.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ac2_proto::model::{
    GeneratorDesired, GeneratorSettings, LocalServer, NetBind, NetSaved, NetSetting, NetStatus,
    NetStatusKind, REMOTE_BASE, RemoteAudio, RemoteInput, RigPeer, ServerInfo, SessionConfig,
    Signal, StateFile, SystemClock,
};
use ac2_proto::units::{Dbfs, Hz};
use ac2_proto::{Command, ErrorCode};
use ac2_zmq::{AuthorizedKeys, Context, CurveClient, KeyPair};
use ac2d::config::SystemClockSource;
use ac2d::{Backoff, Daemon, Handle, Interfaces, NetInterface, NetworkChoice, NetworkSecurity};
use common::*;

const T: Duration = Duration::from_secs(10);

const FAST: Backoff = Backoff {
    first: Duration::from_millis(50),
    max: Duration::from_millis(200),
};

const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// The host as the daemon sees it: loopback to bind on, and one more interface to list.
fn interfaces() -> Interfaces {
    Interfaces::new(|| {
        vec![
            NetInterface {
                name: "lo".into(),
                addrs: vec![LOOPBACK],
                up: true,
            },
            NetInterface {
                name: "stage0".into(),
                addrs: vec![IpAddr::V4(Ipv4Addr::new(10, 9, 9, 9))],
                up: true,
            },
        ]
    })
}

/// `laptop` authorized in `dir`; no server key yet.
fn security(dir: &Path, laptop: &KeyPair) -> NetworkSecurity {
    let mut keys = AuthorizedKeys::new();
    keys.insert("laptop", laptop.public).unwrap();
    let authorized = dir.join("authorized_clients");
    keys.save(&authorized).unwrap();
    NetworkSecurity {
        server_key_file: dir.join("server.key"),
        authorized_clients_file: authorized,
    }
}

/// A daemon whose network follows `store`, local on loopback TCP.
fn settings_config(
    backend: ac2_audio::fake::FakeBackend,
    dir: &Path,
    laptop: &KeyPair,
    store: Option<PathBuf>,
) -> ac2d::DaemonConfig {
    let mut cfg = config(backend, local_tcp());
    cfg.network = NetworkChoice::FromSettings { store };
    cfg.keys = Some(security(dir, laptop));
    cfg.interfaces = interfaces();
    cfg.net_retry = FAST;
    cfg
}

fn store(dir: &Path) -> PathBuf {
    dir.join("network.json")
}

fn turn(c: &mut Client, enabled: bool, port: Option<u16>) -> ServerInfo {
    c.server(Command::ServerNetwork {
        enabled,
        bind: Some(NetBind::Addr { addr: LOOPBACK }),
        port,
    })
}

/// Turns the network on at a free pair and returns its ctrl port. A probed pair can be taken
/// before the daemon binds it, which leaves the network waiting: then another pair.
fn turn_on(c: &mut Client, h: &Handle) -> u16 {
    for _ in 0..5 {
        let port = free_pair();
        turn(c, true, Some(port));
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(3) {
            if listening_on(port)(&h.network_status()) {
                return port;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    panic!("no free port pair: {:?}", h.network_status());
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

fn listening_on(port: u16) -> impl Fn(&NetStatus) -> bool {
    move |s| matches!(s, NetStatus::Listening { ctrl, .. } if ctrl.ends_with(&format!(":{port}")))
}

/// `laptop` says hello over the network at `ctrl`.
fn hello_over_network(ctx: &Context, h: &Handle, laptop: &KeyPair, ctrl: &str) {
    let curve = CurveClient {
        keys: laptop.clone(),
        server_key: h.server_public_key().unwrap(),
    };
    let mut c = Client::connect_with(ctx, ctrl, Some(&curve));
    let w = c.welcome(Command::Hello {
        client: "laptop".into(),
    });
    assert_eq!(w.client_id.0, "laptop");
}

fn file_enabled(dir: &Path) -> bool {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(store(dir)).unwrap()).unwrap();
    v["enabled"].as_bool().unwrap()
}

#[test]
fn on_change_the_port_on_again_and_kept_across_a_restart() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let laptop = KeyPair::generate().unwrap();
    let cfg = settings_config(manual_rig(), dir.path(), &laptop, Some(store(dir.path())));
    let h = Daemon::start(cfg.clone()).unwrap();
    let ctx = Context::new().unwrap();
    let mut c = Client::connect(&ctx, h.ctrl_endpoint());

    // Off from the start, without a key until the first enable.
    let s = c.server(Command::ServerInfo);
    assert_eq!(s.network.setting, NetSetting::Off);
    assert_eq!(s.network.status, NetStatus::Off);
    assert_eq!(s.network.server_key, None);
    assert!(h.server_public_key().is_none());
    assert_eq!(c.state().server.status, NetStatusKind::Off);

    // Keys are managed with the network off.
    let tablet = KeyPair::generate().unwrap();
    c.call(Command::ServerAuthorize {
        name: "tablet".into(),
        key: tablet.public.to_z85(),
    })
    .unwrap();
    let named = |s: &ServerInfo| -> Vec<String> {
        s.network
            .authorized
            .iter()
            .map(|a| a.name.clone())
            .collect()
    };
    assert_eq!(named(&c.server(Command::ServerInfo)), ["laptop", "tablet"]);
    c.call(Command::ServerRevoke {
        name: "tablet".into(),
    })
    .unwrap();
    assert_eq!(named(&c.server(Command::ServerInfo)), ["laptop"]);

    let p1 = turn_on(&mut c, &h);
    let s = c.server(Command::ServerInfo);
    assert!(listening_on(p1)(&s.network.status), "{s:?}");
    assert_eq!(s.network.saved, NetSaved::Saved);
    assert!(s.network.server_key.is_some() && h.server_public_key().is_some());
    assert!(file_enabled(dir.path()));
    let summary = c.state().server;
    assert_eq!(summary.status, NetStatusKind::Listening);
    assert_eq!(summary.port, Some(p1));
    assert_eq!(summary.fingerprint, s.network.fingerprint);
    hello_over_network(&ctx, &h, &laptop, &format!("tcp://127.0.0.1:{p1}"));

    // On again at another port: moved there.
    let p2 = turn_on(&mut c, &h);
    assert_ne!(p1, p2);
    hello_over_network(&ctx, &h, &laptop, &format!("tcp://127.0.0.1:{p2}"));

    // Off, then on with the port kept.
    let s = turn(&mut c, false, None);
    assert_eq!(s.network.status, NetStatus::Off);
    assert!(s.network.server_key.is_some(), "the key stays");
    assert!(!file_enabled(dir.path()));
    let s = turn(&mut c, true, None);
    assert_eq!(
        s.network.setting,
        NetSetting::On {
            bind: NetBind::Addr { addr: LOOPBACK },
            port: p2,
        }
    );
    wait_status(&h, "on again", listening_on(p2));
    let key = h.server_public_key();
    h.shutdown();

    // The setting and the key outlive the daemon.
    let h = Daemon::start(cfg).unwrap();
    wait_status(&h, "after a restart", listening_on(p2));
    assert_eq!(h.server_public_key(), key);
    hello_over_network(&ctx, &h, &laptop, &format!("tcp://127.0.0.1:{p2}"));
    h.shutdown();
}

#[test]
fn rapid_on_off_on_ends_listening() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let laptop = KeyPair::generate().unwrap();
    let h = Daemon::start(settings_config(manual_rig(), dir.path(), &laptop, None)).unwrap();
    let ctx = Context::new().unwrap();
    let mut c = Client::connect(&ctx, h.ctrl_endpoint());
    let port = turn_on(&mut c, &h);
    for _ in 0..5 {
        turn(&mut c, true, Some(port));
        turn(&mut c, false, None);
    }
    let s = turn(&mut c, true, Some(port));
    // In memory only: no store to keep it in.
    assert!(
        matches!(s.network.saved, NetSaved::NotSaved { .. }),
        "{s:?}"
    );
    // The closing pair may still hold the port: then it waits and the retry binds it.
    wait_status(&h, "on after the flurry", listening_on(port));
    hello_over_network(&ctx, &h, &laptop, &format!("tcp://127.0.0.1:{port}"));
    let summary = c.state().server;
    assert_eq!(summary.status, NetStatusKind::Listening);
    h.shutdown();
}

#[test]
fn a_network_client_may_not_toggle_and_is_told_no_host_detail() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let laptop = KeyPair::generate().unwrap();
    let h = Daemon::start(settings_config(manual_rig(), dir.path(), &laptop, None)).unwrap();
    let ctx = Context::new().unwrap();
    let mut local = Client::connect(&ctx, h.ctrl_endpoint());
    let port = turn_on(&mut local, &h);

    let s = local.server(Command::ServerInfo);
    assert!(matches!(s.local, Some(LocalServer::Local { .. })), "{s:?}");
    let names: Vec<&str> = s.interfaces.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["stage0"], "loopback left out");

    let curve = CurveClient {
        keys: laptop.clone(),
        server_key: h.server_public_key().unwrap(),
    };
    let mut remote = Client::connect_with(&ctx, &format!("tcp://127.0.0.1:{port}"), Some(&curve));
    let s = remote.server(Command::ServerInfo);
    assert_eq!(s.local, None);
    assert!(s.interfaces.is_empty());
    assert!(matches!(s.network.status, NetStatus::Listening { .. }));
    for enabled in [false, true] {
        let e = remote
            .call(Command::ServerNetwork {
                enabled,
                bind: None,
                port: None,
            })
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    }
    assert!(listening_on(port)(&h.network_status()));

    // Bad ports are refused before anything changes.
    for bad in [0, u16::MAX] {
        let e = local
            .call(Command::ServerNetwork {
                enabled: true,
                bind: None,
                port: Some(bad),
            })
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::Invalid, "{e:?}");
    }
    assert!(listening_on(port)(&h.network_status()));
    h.shutdown();
}

#[test]
fn fixed_by_listen_or_in_process_the_toggle_is_refused() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let laptop = KeyPair::generate().unwrap();
    let h = Daemon::start(net_config(manual_rig(), security(dir.path(), &laptop))).unwrap();
    let ctx = Context::new().unwrap();
    let mut c = Client::connect(&ctx, h.ctrl_endpoint());
    let e = c
        .call(Command::ServerNetwork {
            enabled: false,
            bind: None,
            port: None,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    let s = c.server(Command::ServerInfo);
    assert!(
        matches!(s.network.setting, NetSetting::Fixed { .. }),
        "{s:?}"
    );
    assert!(matches!(s.network.status, NetStatus::Listening { .. }));
    assert!(!store(dir.path()).exists(), "--listen writes no setting");
    h.shutdown();

    let h = Daemon::start(config(manual_rig(), inproc("net-toggle-inproc"))).unwrap();
    let (mut c, _) = connect(&h, &[b"evt"]);
    let e = c
        .call(Command::ServerNetwork {
            enabled: true,
            bind: None,
            port: None,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Unsupported, "{e:?}");
    h.shutdown();
}

#[test]
fn a_damaged_or_other_version_setting_is_set_aside_and_means_off() {
    init_log();
    for (what, text) in [
        ("torn", r#"{"version":1,"enabled":tr"#.to_owned()),
        (
            "another version",
            r#"{"version":2,"enabled":true,"bind":{"type":"all"},"port":47820}"#.to_owned(),
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let laptop = KeyPair::generate().unwrap();
        std::fs::write(store(dir.path()), text).unwrap();
        let cfg = settings_config(manual_rig(), dir.path(), &laptop, Some(store(dir.path())));
        let h = Daemon::start(cfg).unwrap();
        assert_eq!(h.network_status(), NetStatus::Off, "{what}");
        let ctx = Context::new().unwrap();
        let mut c = Client::connect(&ctx, h.ctrl_endpoint());
        let damaged = c.state().rig.damaged_files;
        let d = damaged
            .iter()
            .find(|d| d.kind == StateFile::NetworkSettings)
            .unwrap_or_else(|| panic!("{what}: {damaged:?}"));
        assert!(d.set_aside_as.is_some(), "{what}");
        assert!(!store(dir.path()).exists(), "{what}: set aside");
        assert_eq!(
            c.server(Command::ServerInfo).network.setting,
            NetSetting::Off
        );
        h.shutdown();
    }
}

fn desired(firing: bool) -> GeneratorDesired {
    GeneratorDesired {
        settings: GeneratorSettings {
            signal: Signal::Sine { freq: Hz(1000.0) },
            level: Dbfs(-30.0),
            band: None,
            outputs: vec![0],
        },
        armed: true,
        firing,
    }
}

/// A network app holding the stimulus is told the network went off before its connection
/// ends, and the stimulus it held stops as its lease runs out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn turning_off_tells_the_network_app_and_its_lease_fades() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let laptop = KeyPair::generate().unwrap();
    let mut cfg = settings_config(realtime_rig(), dir.path(), &laptop, None);
    cfg.lease_expiry = Duration::from_millis(600);
    let h = Daemon::start(cfg).unwrap();
    let ctx = Context::new().unwrap();
    let port = turn_on(&mut Client::connect(&ctx, h.ctrl_endpoint()), &h);
    let local = mirror(&h, "local-test").await;
    local
        .call(Command::SessionOpen {
            config: session(false),
        })
        .await
        .unwrap();

    let mut cc = ac2_client::ClientConfig::new(
        ac2_client::Endpoints {
            ctrl: format!("tcp://127.0.0.1:{port}"),
            data: format!("tcp://127.0.0.1:{}", port + 1),
        },
        "laptop",
    );
    cc.curve = Some(CurveClient {
        keys: laptop,
        server_key: h.server_public_key().unwrap(),
    });
    let remote = ac2_client::Client::connect(cc).await.unwrap();
    remote.wait_synced(Duration::from_secs(5)).await.unwrap();
    let lease = remote
        .acquire_lease(false, ac2_client::OnDrop::StopAndRelease)
        .await
        .unwrap();
    lease.set(desired(true)).await.unwrap();
    let poll = Poll::new(T, 20);
    mirror_until(&local, poll, "the remote app's tone", |s| {
        (s.generator.firing && s.generator.owner.as_ref().is_some_and(|o| o.0 == "laptop"))
            .then_some(())
    })
    .await;
    assert!(!remote.network_turned_off());

    local
        .call(Command::ServerNetwork {
            enabled: false,
            bind: None,
            port: None,
        })
        .await
        .unwrap();
    let t0 = Instant::now();
    while !remote.network_turned_off() {
        assert!(t0.elapsed() < T, "the remote app was not told");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let s = mirror_until(&local, poll, "the lease run out", |s| {
        (s.generator.owner.is_none() && !s.generator.firing).then(|| s.server.clone())
    })
    .await;
    assert_eq!(s.status, NetStatusKind::Off);
    drop(lease);
    drop(remote);
    h.shutdown();
}

/// A rig whose network is turned on while its session runs serves a paired rig its input
/// at once: the session needs no reopening for the new pair.
#[test]
fn slow_turned_on_mid_session_a_paired_rig_gets_its_audio() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let app = KeyPair::generate().unwrap();
    let synced = SystemClockSource::Stated(SystemClock::Synced { maxerror_us: 100 });
    let b_dir = dir.path().join("b");
    std::fs::create_dir_all(&b_dir).unwrap();
    let mut cfg = settings_config(realtime_rig(), &b_dir, &app, Some(store(&b_dir)));
    cfg.name = "rig-b".into();
    cfg.system_clock = synced;
    let b = Daemon::start(cfg).unwrap();
    let a_dir = dir.path().join("a");
    std::fs::create_dir_all(&a_dir).unwrap();
    let mut cfg = net_config(realtime_rig(), security(&a_dir, &app));
    cfg.name = "rig-a".into();
    cfg.system_clock = synced;
    let a = Daemon::start(cfg).unwrap();

    let ctx = Context::new().unwrap();
    let mut cb = Client::connect(&ctx, b.ctrl_endpoint());
    let b_id = cb
        .welcome(Command::Hello {
            client: "test".into(),
        })
        .rig_id;
    cb.ok(Command::SessionOpen {
        config: session(false),
    });
    let epoch = cb.state().session.epoch;
    let port = turn_on(&mut cb, &b);
    let a_key = a.server_public_key().unwrap();
    let mut ca = Client::connect(&ctx, a.ctrl_endpoint());
    let a_id = ca
        .welcome(Command::Hello {
            client: "test".into(),
        })
        .rig_id;
    cb.ok(Command::AuthAdmitRig {
        key: a_key.to_z85(),
        rig_id: a_id,
        name: "rig-a".into(),
    });
    ca.ok(Command::RigPeers {
        peers: vec![RigPeer {
            rig: b_id,
            name: "rig-b".into(),
            ctrl: format!("tcp://127.0.0.1:{port}"),
            data: format!("tcp://127.0.0.1:{}", port + 1),
            key: b.server_public_key().unwrap().to_z85(),
        }],
    });
    ca.ok(Command::SessionOpen {
        config: SessionConfig {
            input_channels: vec![0, REMOTE_BASE],
            sample_rate_hz: Some(FS),
            remote_inputs: vec![RemoteInput {
                slot: 0,
                rig: b_id,
                rig_name: "rig-b".into(),
                channel: 1,
            }],
            ..session(false)
        },
    });
    let t0 = Instant::now();
    loop {
        let r = ca.state().remote;
        if r.inputs.first().map(|i| i.audio) == Some(RemoteAudio::Streaming) {
            break;
        }
        assert!(t0.elapsed() < T, "rig-b's input never streamed: {r:#?}");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        cb.state().session.epoch,
        epoch,
        "the session was not reopened"
    );
    a.shutdown();
    b.shutdown();
}
