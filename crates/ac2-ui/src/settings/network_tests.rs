use super::*;
use ac2_proto::model::{
    NetSaved, NetSettingKind, NetStatus, NetStatusView, NetworkServer, PeerNetStatus,
};
use ac2_proto::samples::{server_info, server_info_fixed, server_info_off};

fn page(info: ServerInfo) -> ConnectionPage {
    ConnectionPage {
        server: Some(Ok(info)),
        ..ConnectionPage::default()
    }
}

fn focus(p: &mut ConnectionPage, l: ConnLine) {
    p.focus = p.lines().iter().position(|x| *x == l).expect("listed");
}

/// The sample daemon on this computer with its network off, saved, with eth0 to bind.
fn off() -> ServerInfo {
    let mut s = server_info_off();
    s.network.setting = NetSettingView::Host {
        setting: NetSetting::Off {
            saved: NetSaved::Saved,
        },
    };
    s.interfaces = server_info().interfaces;
    s
}

#[test]
fn turning_on_asks_first_and_sends_what_was_picked() {
    let mut p = page(off());
    focus(&mut p, ConnLine::Network);
    assert!(
        p.net_line_text(&ConnLine::Network)
            .starts_with("Network: off · this rig still")
    );
    assert_eq!(
        p.net_line_text(&ConnLine::Interface),
        "Interface: as last set on this rig (←→ picks)"
    );
    // Nothing picked: the rig's own last choice, said as such.
    assert_eq!(p.enter(), None);
    assert!(
        p.notice
            .as_deref()
            .is_some_and(|n| n.contains("where it was last set on this rig")),
        "{:?}",
        p.notice
    );
    assert_eq!(
        p.enter(),
        Some(ConnAction::Network {
            enabled: true,
            bind: None,
            port: None,
            change: NetChange::On,
        })
    );

    // Picked: eth0 (←→), then a port typed.
    focus(&mut p, ConnLine::Interface);
    assert_eq!(p.toggle(1), None);
    assert_eq!(p.net.iface, Some(IfacePick::Bind(NetBind::All)));
    assert_eq!(p.toggle(1), None);
    let eth0 = NetBind::Interface {
        name: "eth0".into(),
    };
    assert_eq!(p.net.iface, Some(IfacePick::Bind(eth0.clone())));
    assert_eq!(
        p.net_line_text(&ConnLine::Interface),
        "Interface: eth0 (192.168.1.20, 2001:db8::20) (←→ picks)"
    );
    focus(&mut p, ConnLine::Port);
    assert_eq!(p.enter(), None);
    p.type_text("0");
    assert_eq!(p.enter(), None);
    assert_eq!(
        p.error.as_deref(),
        Some("type a port from 1 to 65534 (data takes the next one)")
    );
    p.backspace();
    p.type_text("48000");
    assert_eq!(p.enter(), None, "picked while off: sent with the switch");
    assert_eq!(
        p.net_line_text(&ConnLine::Port),
        "Port: 48000 · data on 48001 (Enter types another)"
    );
    focus(&mut p, ConnLine::Network);
    assert_eq!(p.toggle(1), None, "←/→ on the switch asks as Enter does");
    assert!(
        p.notice.as_deref().is_some_and(
            |n| n.contains("eth0 (192.168.1.20, 2001:db8::20), port 48000 (data 48001)")
        ),
        "{:?}",
        p.notice
    );
    // Moving away forgets the question.
    p.move_focus(1);
    p.move_focus(-1);
    assert_eq!(p.enter(), None);
    assert_eq!(
        p.enter(),
        Some(ConnAction::Network {
            enabled: true,
            bind: Some(eth0),
            port: Some(48000),
            change: NetChange::On,
        })
    );
}

#[test]
fn off_goes_at_once_and_a_network_that_is_on_moves_on_enter() {
    let mut p = page(server_info());
    focus(&mut p, ConnLine::Network);
    assert!(
        p.net_line_text(&ConnLine::Network)
            .contains("every app and rig connected to this rig over the network is cut")
    );
    assert_eq!(
        p.enter(),
        Some(ConnAction::Network {
            enabled: false,
            bind: None,
            port: None,
            change: NetChange::Off,
        })
    );
    // The interface in force first; ←/→ pick another, Enter moves it there.
    focus(&mut p, ConnLine::Interface);
    assert_eq!(p.enter(), None, "nothing new picked");
    assert_eq!(p.toggle(-1), None);
    assert_eq!(p.net.iface, Some(IfacePick::Bind(NetBind::All)));
    assert!(
        p.net_line_text(&ConnLine::Interface)
            .contains("Enter moves the network here")
    );
    assert_eq!(
        p.enter(),
        Some(ConnAction::Network {
            enabled: true,
            bind: Some(NetBind::All),
            port: None,
            change: NetChange::Moved,
        })
    );
    // The last choice types an address, checked before it goes.
    assert_eq!(p.toggle(-1), None);
    assert_eq!(p.net.iface, Some(IfacePick::TypeAddress));
    assert_eq!(p.enter(), None);
    assert!(p.typing());
    p.type_text("eth0");
    assert_eq!(p.enter(), None);
    assert!(p.error.is_some());
    p.edit = Some((ConnEdit::Address, "192.168.1.20".into()));
    assert_eq!(
        p.enter(),
        Some(ConnAction::Network {
            enabled: true,
            bind: Some(NetBind::Addr {
                addr: [192, 168, 1, 20].into()
            }),
            port: None,
            change: NetChange::Moved,
        })
    );
    // The port in force is not sent again; another one is.
    focus(&mut p, ConnLine::Port);
    p.edit = Some((ConnEdit::Port, "47820".into()));
    assert_eq!(p.enter(), None);
    p.edit = Some((ConnEdit::Port, "47830".into()));
    assert_eq!(
        p.enter(),
        Some(ConnAction::Network {
            enabled: true,
            bind: None,
            port: Some(47830),
            change: NetChange::Moved,
        })
    );
}

#[test]
fn windows_local_ports_are_refused_before_sending() {
    let mut s = off();
    s.local = Some(LocalServer::Local {
        ctrl: "tcp://127.0.0.1:47822".into(),
        data: "tcp://127.0.0.1:47823".into(),
    });
    let mut p = page(s);
    focus(&mut p, ConnLine::Port);
    p.edit = Some((ConnEdit::Port, "47822".into()));
    assert_eq!(p.enter(), None);
    assert!(
        p.error.as_deref().is_some_and(
            |e| e.starts_with("port 47822 (data 47823) would take TCP 47822 and 47823")
        ),
        "{:?}",
        p.error
    );
    assert!(p.typing(), "the typed port stays to be corrected");
}

#[test]
fn fixed_remote_and_in_process_rigs_are_read_only() {
    // Fixed by --listen on this computer.
    let mut s = server_info();
    s.network.setting = NetSettingView::Host {
        setting: NetSetting::Fixed {
            listen: "tcp://eth0:47820".into(),
        },
    };
    let p = page(s.clone());
    assert!(!p.lines().contains(&ConnLine::Network));
    assert_eq!(
        read_only_row(&s).as_deref(),
        Some(
            "Network: on, set by --listen (systemd): tcp://eth0:47820 · change it in the \
             service's command line"
        )
    );
    // Reached over the network: the kind only, no switch.
    let r = server_info_fixed();
    assert!(!page(r.clone()).lines().contains(&ConnLine::Network));
    assert_eq!(
        read_only_row(&r).as_deref(),
        Some("Network: on, set by --listen (systemd) · change it on the rig's own computer")
    );
    let mut on = r;
    on.network = NetworkServer {
        setting: NetSettingView::Peer {
            setting: NetSettingKind::On,
        },
        status: NetStatusView::Peer {
            status: PeerNetStatus::Off,
        },
        ..on.network
    };
    assert_eq!(
        read_only_row(&on).as_deref(),
        Some("Network: on · change it on the rig's own computer")
    );
    // Inside its program.
    let mut e = off();
    e.local = Some(LocalServer::Embedded);
    e.network.status = NetStatusView::Host {
        status: NetStatus::Off,
    };
    assert!(!page(e.clone()).lines().contains(&ConnLine::Network));
    assert_eq!(
        read_only_row(&e).as_deref(),
        Some(ac2_scene::network::EMBEDDED_ROW)
    );
    assert_eq!(read_only_row(&off()), None);
}
