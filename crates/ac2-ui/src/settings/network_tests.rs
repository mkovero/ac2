use super::*;
use ac2_proto::model::{
    LocalServer, NetSaved, NetSettingKind, NetStatus, NetStatusView, NetworkServer, PeerNetStatus,
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

/// The sample daemon on this computer with its network off and never on, saved, with eth0
/// to bind.
fn off() -> ServerInfo {
    let mut s = server_info_off();
    s.network.setting = NetSettingView::Host {
        setting: NetSetting::Off {
            kept: None,
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
    assert_eq!(
        p.net_line_text(&ConnLine::Network),
        "Network: off · Enter turns it on"
    );
    assert_eq!(
        p.net_line_tip(&ConnLine::Network),
        Some(ac2_scene::network::network_tip(false))
    );
    // Never on: the daemon's default, said as such.
    assert_eq!(
        p.net_line_text(&ConnLine::Interface),
        "Interface: all interfaces, the default (←→ picks)"
    );
    assert_eq!(
        p.net_line_text(&ConnLine::Port),
        "Port: 47820, the default · data on 47821 (Enter types another)"
    );
    assert_eq!(
        p.net_line_tip(&ConnLine::Port),
        Some(ac2_scene::network::choice_tip(ChoiceFrom::Default))
    );
    assert_eq!(p.enter(), None);
    assert_eq!(
        p.notice.as_deref(),
        Some(
            "Enter again turns the network on at all interfaces, port 47820 (data 47821), the \
             default · apps and rigs whose keys this rig authorized can then connect"
        )
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

    // Picked: eth0 (←→ steps on from the default shown), then a port typed.
    focus(&mut p, ConnLine::Interface);
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
        p.notice.as_deref().is_some_and(|n| n
            .contains("on at eth0 (192.168.1.20, 2001:db8::20), port 48000 (data 48001) · apps")),
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

/// Off with a choice kept from when it was last on: the rows show what turning on uses, and
/// a pick here still wins.
#[test]
fn off_shows_the_kept_choice_and_a_pick_overrides_it() {
    let mut p = page(server_info_off());
    assert_eq!(
        p.net_line_text(&ConnLine::Interface),
        "Interface: address 10.0.0.2 (not on this computer now) (←→ picks)"
    );
    assert_eq!(
        p.net_line_text(&ConnLine::Port),
        "Port: 5000 · data on 5001 (Enter types another)"
    );
    assert_eq!(
        p.net_line_tip(&ConnLine::Interface),
        Some(ac2_scene::network::choice_tip(ChoiceFrom::Kept))
    );
    focus(&mut p, ConnLine::Network);
    assert_eq!(p.enter(), None);
    assert!(
        p.notice.as_deref().is_some_and(|n| n.contains(
            "on at address 10.0.0.2 (not on this computer now), port 5000 (data 5001) · apps"
        )),
        "{:?}",
        p.notice
    );
    // A port typed here goes with the turn-on; the kept interface is the rig's to keep.
    p.move_focus(1);
    focus(&mut p, ConnLine::Port);
    p.edit = Some((ConnEdit::Port, "6000".into()));
    assert_eq!(p.enter(), None);
    assert_eq!(
        p.net_line_tip(&ConnLine::Port),
        Some(ac2_scene::network::choice_tip(ChoiceFrom::Picked))
    );
    focus(&mut p, ConnLine::Network);
    assert_eq!(p.enter(), None);
    assert!(
        p.notice
            .as_deref()
            .is_some_and(|n| n.contains("(not on this computer now), port 6000 (data 6001) · ")),
        "{:?}",
        p.notice
    );
    assert_eq!(
        p.enter(),
        Some(ConnAction::Network {
            enabled: true,
            bind: None,
            port: Some(6000),
            change: NetChange::On,
        })
    );
}

#[test]
fn off_goes_at_once_and_a_network_that_is_on_moves_on_enter() {
    let mut p = page(server_info());
    focus(&mut p, ConnLine::Network);
    assert_eq!(
        p.net_line_text(&ConnLine::Network),
        "Network: on · Enter turns it off: cuts apps and rigs connected to it"
    );
    assert_eq!(
        p.net_line_tip(&ConnLine::Network),
        Some(ac2_scene::network::network_tip(true))
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

/// Whether the page offers the switch is the daemon's answer ([`NetToggle`]), nothing else.
#[test]
fn the_daemons_toggle_decides_the_switch() {
    use ac2_scene::network as words;
    let row = |s: &ServerInfo| read_only_row(s).map(|(t, tip)| (t, tip.to_owned()));
    // Fixed by --listen on this computer.
    let mut s = server_info();
    s.network.setting = NetSettingView::Host {
        setting: NetSetting::Fixed {
            listen: "tcp://eth0:47820".into(),
        },
    };
    s.toggle = NetToggle::Fixed;
    let p = page(s.clone());
    assert!(!p.lines().contains(&ConnLine::Network));
    assert_eq!(
        row(&s),
        Some((
            "Network: on, set by --listen (systemd): tcp://eth0:47820 · change it in the \
             service's command line"
                .into(),
            words::FIXED_TIP.into()
        ))
    );
    // Reached over the network: the kind only, no switch.
    let r = server_info_fixed();
    assert!(!page(r.clone()).lines().contains(&ConnLine::Network));
    assert_eq!(
        row(&r),
        Some((
            "Network: on, set by --listen (systemd) · change it on the rig's own computer".into(),
            words::PEER_TIP.into()
        ))
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
        row(&on).map(|r| r.0).as_deref(),
        Some("Network: on · change it on the rig's own computer")
    );
    // Inside its program.
    let mut e = off();
    e.local = Some(LocalServer::Embedded);
    e.toggle = NetToggle::Unavailable {
        reason: NetUnavailable::InProcess,
    };
    e.network.status = NetStatusView::Host {
        status: NetStatus::Off,
    };
    assert!(!page(e.clone()).lines().contains(&ConnLine::Network));
    assert_eq!(
        row(&e),
        Some((words::EMBEDDED_ROW.into(), words::EMBEDDED_TIP.into()))
    );
    // No config directory: nowhere to keep a server key.
    let mut k = off();
    k.toggle = NetToggle::Unavailable {
        reason: NetUnavailable::NoKeyStore,
    };
    assert!(!page(k.clone()).lines().contains(&ConnLine::Network));
    assert_eq!(
        row(&k),
        Some((
            "Network: off · cannot be turned on: no config directory for a server key".into(),
            words::NO_KEY_STORE_TIP.into()
        ))
    );
    // A local daemon told in full is still not switched unless it allows it: the toggle is
    // the one source.
    let mut n = server_info();
    n.toggle = NetToggle::Unavailable {
        reason: NetUnavailable::NotLocal,
    };
    assert!(!page(n.clone()).lines().contains(&ConnLine::Network));
    assert_eq!(
        row(&n).map(|r| r.0).as_deref(),
        Some("Network: on · change it on the rig's own computer")
    );
    assert_eq!(read_only_row(&off()), None);
    assert!(page(off()).lines().contains(&ConnLine::Network));
}
