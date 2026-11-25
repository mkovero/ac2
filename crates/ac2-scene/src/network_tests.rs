use super::*;
use ac2_proto::model::{NetReason, NetReasonKind, NetSetting, NetSettingView};
use ac2_proto::samples::{server_info, server_info_fixed, server_info_off};

fn host(status: NetStatus) -> NetStatusView {
    NetStatusView::Host { status }
}

#[test]
fn the_switch_says_its_state_and_its_tip_what_it_means() {
    assert_eq!(network_row(false), "Network: off · Enter turns it on");
    // Off acts at once: the row itself says what it cuts.
    assert_eq!(
        network_row(true),
        "Network: on · Enter turns it off: cuts apps and rigs connected to it"
    );
    assert_eq!(
        network_tip(false),
        "Off: nothing can connect to this rig. Its own links stay."
    );
    assert_eq!(
        network_tip(true),
        "On: authorized apps and rigs can connect to this rig"
    );
}

#[test]
fn a_switch_the_page_cannot_use_says_why() {
    assert_eq!(
        fixed_row(Some("tcp://eth0:47820")),
        "Network: on, set by --listen (systemd): tcp://eth0:47820 · change it in the service's \
         command line"
    );
    assert_eq!(
        fixed_row(None),
        "Network: on, set by --listen (systemd) · change it in the service's command line"
    );
    assert_eq!(FIXED_TIP, "Set by ac2d's command line: read only here");
    assert_eq!(
        EMBEDDED_ROW,
        "Network: none · this daemon runs inside its program"
    );
    assert_eq!(EMBEDDED_TIP, "Serves its own program only");
    assert_eq!(
        no_key_store_row(NetSettingKind::Off),
        "Network: off · cannot be turned on: no config directory for a server key"
    );
    assert_eq!(
        NO_KEY_STORE_TIP,
        "Start ac2d with a config directory to turn it on"
    );
    assert_eq!(
        peer_row(NetSettingKind::Fixed),
        "Network: on, set by --listen (systemd) · change it on the rig's own computer"
    );
    assert_eq!(
        peer_row(NetSettingKind::Off),
        "Network: off · change it on the rig's own computer"
    );
    assert_eq!(PEER_TIP, "Read only from another computer");
}

#[test]
fn interfaces_read_as_the_daemon_lists_them() {
    let s = server_info();
    let i = &s.interfaces;
    assert_eq!(
        interface_row(IfaceRow::Bind(&NetBind::All, ChoiceFrom::InForce), i, false),
        "Interface: all interfaces (←→ picks)"
    );
    let eth0 = NetBind::Interface {
        name: "eth0".into(),
    };
    assert_eq!(
        interface_row(IfaceRow::Bind(&eth0, ChoiceFrom::Kept), i, false),
        "Interface: eth0 (192.168.1.20, 2001:db8::20) (←→ picks)"
    );
    assert_eq!(
        interface_row(IfaceRow::Bind(&eth0, ChoiceFrom::Picked), i, true),
        "Interface: eth0 (192.168.1.20, 2001:db8::20) · Enter moves the network here (←→ picks)"
    );
    let wlan = NetBind::Interface {
        name: "wlan0".into(),
    };
    assert_eq!(bind_text(&wlan, i), "wlan0 (no address, down)");
    let gone = NetBind::Interface {
        name: "usb0".into(),
    };
    assert_eq!(bind_text(&gone, i), "usb0 (not on this computer now)");
    let addr = NetBind::Addr {
        addr: IpAddr::from([192, 168, 1, 20]),
    };
    assert_eq!(bind_text(&addr, i), "address 192.168.1.20 (eth0)");
    let away = NetBind::Addr {
        addr: IpAddr::from([10, 0, 0, 9]),
    };
    assert_eq!(
        bind_text(&away, i),
        "address 10.0.0.9 (not on this computer now)"
    );
    let lo = NetBind::Addr {
        addr: IpAddr::from([127, 0, 0, 1]),
    };
    assert_eq!(bind_text(&lo, i), "address 127.0.0.1 (loopback)");
    // Off and never on: the daemon's default, said as such.
    assert_eq!(
        interface_row(IfaceRow::Bind(&NetBind::All, ChoiceFrom::Default), i, false),
        "Interface: all interfaces, the default (←→ picks)"
    );
    assert_eq!(
        interface_row(IfaceRow::TypeAddress, i, false),
        "Interface: an address… (Enter types it · ←→ picks)"
    );
    assert_eq!(
        port_row(5000, ChoiceFrom::Kept),
        "Port: 5000 · data on 5001 (Enter types another)"
    );
    assert_eq!(
        port_row(47820, ChoiceFrom::Default),
        "Port: 47820, the default · data on 47821 (Enter types another)"
    );
    assert_eq!(
        choice_tip(ChoiceFrom::InForce),
        "In use now: picking another moves it"
    );
    assert_eq!(
        choice_tip(ChoiceFrom::Picked),
        "Picked: used when the network goes on or moves"
    );
    assert_eq!(
        choice_tip(ChoiceFrom::Kept),
        "Kept from when the network was last on"
    );
    assert_eq!(
        choice_tip(ChoiceFrom::Default),
        "Default: the network has not been on yet"
    );
}

#[test]
fn status_follows_the_daemon() {
    assert_eq!(
        status_line(&server_info().network.status),
        "Status: listening on 192.168.1.20:47820 (CURVE; authorized keys only)"
    );
    assert_eq!(
        status_line(&host(NetStatus::Listening {
            ctrl: "tcp://0.0.0.0:47820".into(),
            data: "tcp://0.0.0.0:47821".into(),
        })),
        "Status: listening on all interfaces, port 47820 (CURVE; authorized keys only)"
    );
    assert_eq!(
        status_line(&host(NetStatus::Listening {
            ctrl: "tcp://[2001:db8::20]:47820".into(),
            data: "tcp://[2001:db8::20]:47821".into(),
        })),
        "Status: listening on [2001:db8::20]:47820 (CURVE; authorized keys only)"
    );
    assert_eq!(status_line(&host(NetStatus::Off)), "Status: off");
    assert_eq!(
        status_line(&host(NetStatus::Waiting {
            reason: NetReason::AddressInUse
        })),
        "Status: waiting: the port is in use · retrying"
    );
    assert_eq!(
        status_line(&host(NetStatus::Failed {
            reason: NetReason::Other {
                detail: "Too many open files".into()
            }
        })),
        "Status: failed: Too many open files · retrying"
    );
    // A network client is told why by kind only: the host's own words stay there.
    assert_eq!(
        status_line(&server_info_fixed().network.status),
        "Status: waiting: the address is not on this computer · retrying"
    );
    assert_eq!(
        status_line(&NetStatusView::Peer {
            status: PeerNetStatus::Failed {
                reason: NetReasonKind::Other
            }
        }),
        "Status: failed: an error on the rig · retrying"
    );
}

#[test]
fn a_setting_not_kept_says_so() {
    let NetSettingView::Host {
        setting: NetSetting::Off { saved, .. },
    } = &server_info_off().network.setting
    else {
        panic!("the sample is a local daemon, off");
    };
    assert_eq!(
        not_saved_line(saved).as_deref(),
        Some(
            "Not saved: writing the settings file failed (read-only file system) · ac2d starts \
             without this setting next time"
        )
    );
    assert_eq!(not_saved_line(&NetSaved::Saved), None);
    assert_eq!(
        not_saved_line(&NetSaved::NotSaved {
            reason: NotSavedReason::NoStore
        })
        .as_deref(),
        Some(
            "Not saved: this ac2d keeps no settings file · ac2d starts without this setting next time"
        )
    );
}

#[test]
fn turning_on_names_where() {
    let i = &server_info().interfaces;
    let eth0 = NetBind::Interface {
        name: "eth0".into(),
    };
    assert_eq!(
        turn_on_confirm(&eth0, 5000, false, i),
        "Enter again turns the network on at eth0 (192.168.1.20, 2001:db8::20), port 5000 (data \
         5001) · apps and rigs whose keys this rig authorized can then connect"
    );
    assert_eq!(
        turn_on_confirm(&NetBind::All, 47820, true, i),
        "Enter again turns the network on at all interfaces, port 47820 (data 47821), the \
         default · apps and rigs whose keys this rig authorized can then connect"
    );
}

#[test]
fn ports_are_checked_before_sending() {
    assert_eq!(parse_port(" 47820 "), Ok(47820));
    for bad in ["0", "65535", "x", ""] {
        assert_eq!(
            parse_port(bad),
            Err("type a port from 1 to 65534 (data takes the next one)".into()),
            "{bad}"
        );
    }
    assert_eq!(
        parse_address("192.168.1.20"),
        Ok(IpAddr::from([192, 168, 1, 20]))
    );
    assert!(parse_address("eth0").is_err());
}

#[test]
fn a_rig_that_turned_its_network_off_is_not_a_fault() {
    use crate::chrome::{RigDown, rig_pane_down, rig_row};
    assert_eq!(
        rig_row("pupu", Some(RigDown::NetworkOff)),
        "pupu · turned its network off · retrying"
    );
    assert_eq!(
        rig_pane_down("pupu", RigDown::NetworkOff),
        "pupu turned its network off · retrying"
    );
    assert_eq!(
        network_off_toast("pupu"),
        "pupu turned its network off: the app connects again once it is back on (Settings › \
         Connection on pupu's own computer)"
    );
    assert_eq!(change_done(NetChange::Off), "network turned off");
    assert_eq!(change_failed(NetChange::On), "cannot turn the network on");
}
