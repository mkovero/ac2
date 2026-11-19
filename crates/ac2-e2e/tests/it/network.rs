//! Settings › Connection's network rows against a real daemon: one that has no network says
//! so when asked; one on this computer is turned on and off from the page, its status line
//! following `State.server`, and an app connected over the network reads the rig as having
//! turned its network off, not as a lost link.

use crate::harness::{Driver, NAME, R, connected};
use ac2_client::{ClientConfig, Endpoints};
use ac2_e2e::{Setup, sim_daemon_with};
use ac2_proto::model::{NetSetting, NetSettingView, NetStatus, NetStatusView, ServerInfo};
use ac2_scene::chrome::RigDown;
use ac2_ui::settings::{ConnLine, ConnectionPage, Page};
use ac2_ui::state::{AppState, ConnState};
use ac2_zmq::{AuthorizedKeys, CurveClient, KeyPair, PublicKey};
use std::net::{IpAddr, Ipv4Addr, TcpListener};
use std::time::Duration;

fn page(s: &AppState) -> Option<&ConnectionPage> {
    s.overlay
        .settings()
        .filter(|x| x.page == Page::Connection)
        .map(|x| &x.connection)
}

fn info(s: &AppState) -> Option<&ServerInfo> {
    page(s)?.server.as_ref()?.as_ref().ok()
}

/// Settings › Connection, once the daemon answered.
fn open_connection(d: &mut Driver) -> R {
    d.synced()?;
    d.key("Ctrl+P");
    d.key("Alt+6");
    d.until("the daemon's server info", |s| info(s).is_some())
}

fn focus(d: &mut Driver, l: ConnLine) -> R {
    for _ in 0..16 {
        if page(&d.st).and_then(ConnectionPage::focused) == Some(l.clone()) {
            return Ok(());
        }
        d.key("Down");
    }
    Err(format!("no {l:?} line").into())
}

/// The test's simulated rig has no network: Enter twice on the switch, and the page says
/// why in the daemon's words.
#[test]
fn a_daemon_without_a_network_says_so() -> R {
    let (_daemon, mut d) = connected()?;
    open_connection(&mut d)?;
    focus(&mut d, ConnLine::Network)?;
    d.key("Enter");
    d.key("Enter");
    d.until("the refusal on the page", |s| {
        page(s).and_then(|p| p.error.as_deref()).is_some_and(|e| {
            e.starts_with("cannot turn the network on") && e.contains("in-process only")
        })
    })?;
    assert!(
        matches!(
            info(&d.st).map(|i| &i.network.status),
            Some(NetStatusView::Host {
                status: NetStatus::Off
            })
        ),
        "still off"
    );
    Ok(())
}

/// A ctrl port whose data port (the next one) is free too, probed below every OS's
/// ephemeral range: macOS and Windows hand ephemeral ports out in sequence, so the port after
/// one the OS just picked may be taken before the daemon binds it.
fn free_pair() -> R<u16> {
    let start = 20_000 + (std::process::id() % 6_000) as u16 * 2;
    (start..32_000)
        .step_by(2)
        .find(|&p| {
            TcpListener::bind((Ipv4Addr::LOCALHOST, p)).is_ok()
                && TcpListener::bind((Ipv4Addr::LOCALHOST, p + 1)).is_ok()
        })
        .ok_or_else(|| "no free port pair".into())
}

fn status(s: &AppState) -> Option<&NetStatus> {
    match &info(s)?.network.status {
        NetStatusView::Host { status } => Some(status),
        NetStatusView::Peer { .. } => None,
    }
}

/// On from the page (an address typed, a port typed, Enter twice), the status line follows;
/// an app on the network connects; off from the page cuts it, and it says the rig turned its
/// network off.
#[test]
fn network_on_and_off_from_the_page() -> R {
    let dir = tempfile::tempdir()?;
    let app = KeyPair::generate()?;
    let mut authorized = AuthorizedKeys::new();
    authorized.insert("tablet", app.public)?;
    let authorized_file = dir.path().join("authorized_clients");
    authorized.save(&authorized_file)?;
    let keys = ac2d::NetworkSecurity {
        server_key_file: dir.path().join("server.key"),
        authorized_clients_file: authorized_file,
    };
    let daemon = sim_daemon_with(Setup::Empty, move |cfg| {
        cfg.network = ac2d::NetworkChoice::FromSettings { store: None };
        cfg.keys = Some(keys);
        cfg.interfaces = ac2d::Interfaces::new(|| {
            vec![ac2d::NetInterface {
                name: "lo".into(),
                addrs: vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
                up: true,
            }]
        });
        cfg.net_retry = ac2d::Backoff {
            first: Duration::from_millis(50),
            max: Duration::from_millis(200),
        };
    })?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    open_connection(&mut d)?;
    assert!(matches!(
        &info(&d.st).ok_or("info")?.network.setting,
        NetSettingView::Host {
            setting: NetSetting::Off { .. }
        }
    ));
    // The interface's last choice types an address.
    focus(&mut d, ConnLine::Interface)?;
    d.key("Left");
    d.key("Enter");
    d.send(ac2_ui::state::Msg::Text("127.0.0.1".into()));
    d.key("Enter");
    let port = free_pair()?;
    focus(&mut d, ConnLine::Port)?;
    d.key("Enter");
    d.send(ac2_ui::state::Msg::Text(port.to_string()));
    d.key("Enter");
    focus(&mut d, ConnLine::Network)?;
    d.key("Enter");
    let asked = page(&d.st)
        .and_then(|p| p.notice.clone())
        .unwrap_or_default();
    assert!(
        asked.contains(&format!(
            "address 127.0.0.1 (loopback), port {port} (data {})",
            port + 1
        )),
        "{asked}"
    );
    d.key("Enter");
    let line = format!("Status: listening on 127.0.0.1:{port} (CURVE; authorized keys only)");
    d.until("listening, on the page", |s| {
        info(s).is_some_and(|i| ac2_scene::network::status_line(&i.network.status) == line)
            && s.daemon()
                .is_some_and(|st| st.server.status == ac2_proto::model::NetStatusKind::Listening)
    })?;
    assert!(
        page(&d.st).is_some_and(|p| p
            .net_line_text(&ConnLine::Network)
            .starts_with("Network: on")),
        "the switch reads on"
    );

    // An app over the network, its key authorized.
    let (ctrl, data, server_key) = match (status(&d.st), info(&d.st)) {
        (Some(NetStatus::Listening { ctrl, data }), Some(i)) => (
            ctrl.clone(),
            data.clone(),
            i.network.server_key.clone().ok_or("no server key")?,
        ),
        s => return Err(format!("not listening: {s:?}").into()),
    };
    let mut config = ClientConfig::new(Endpoints { ctrl, data }, "tablet");
    config.curve = Some(CurveClient {
        keys: app,
        server_key: PublicKey::from_z85(&server_key)?,
    });
    let mut remote = Driver::connect(config, "the rig over the network")?;
    remote.until("connected over the network", |s| {
        matches!(s.rig.conn, ConnState::Connected { .. }) && s.daemon().is_some()
    })?;

    // Off: the status line follows, and the app on the network says why it is cut.
    focus(&mut d, ConnLine::Network)?;
    d.key("Enter");
    d.until("off, on the page", |s| {
        matches!(status(s), Some(NetStatus::Off))
            && s.daemon()
                .is_some_and(|st| st.server.status == ac2_proto::model::NetStatusKind::Off)
    })?;
    remote.until("the rig read as having turned its network off", |s| {
        s.rig.conn.down() == Some(RigDown::NetworkOff)
    })?;
    assert!(
        remote
            .st
            .toasts
            .list
            .iter()
            .any(|t| t.text.contains("turned its network off")),
        "{:?}",
        remote.st.toasts.list
    );
    Ok(())
}
