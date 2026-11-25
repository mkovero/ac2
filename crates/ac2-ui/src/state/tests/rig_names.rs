//! This computer's `ac2d` is named by the host: one runs per user, so nothing on the host
//! needs telling apart.

use super::*;
use crate::prefs::RigAddr;

/// The second rig, reached as `addr` on host `kettu`, said hello as `name`.
fn on_kettu(addr: Option<RigAddr>, name: &str) -> (T, RigId) {
    let mut t = T::new();
    t.st.local_host = "kettu".into();
    let slot = t.st.add_rig("local".into(), None, addr);
    let id = RigId(0x77);
    t.st.update(
        Msg::Conn(
            RigId(u64::from(slot)),
            Box::new(ConnEvent::Connected {
                target: "local".into(),
                server: "ac2d test".into(),
                client_id: ClientId("c2".into()),
                rig_id: id,
                name: name.into(),
            }),
        ),
        &t.keys,
    );
    (t, id)
}

/// Whatever name the local daemon says hello with: the app cannot tell a default name from
/// one given.
#[test]
fn the_local_daemon_is_the_host() {
    for name in ["ac2 on kettu", "FOH"] {
        let (t, id) = on_kettu(Some(RigAddr::Local), name);
        assert_eq!(t.st.rig_name(id), "kettu");
    }
}

/// The simulated rig is not remembered and reads its hello name.
#[test]
fn the_simulated_rig_is_its_name() {
    let (t, id) = on_kettu(None, "Simulated rig");
    assert_eq!(t.st.rig_name(id), "Simulated rig");
}

#[test]
fn a_network_rig_keeps_its_own_name() {
    let addr = RigAddr::Network {
        host: "192.168.9.27".into(),
        port: 47820,
        fingerprint: None,
        mdns_host: None,
    };
    let (t, id) = on_kettu(Some(addr.clone()), "pupu");
    assert_eq!(t.st.rig_name(id), "pupu");
    let (t, id) = on_kettu(Some(addr), "ac2 on pupu");
    assert_eq!(t.st.rig_name(id), "ac2 on pupu");
}

/// The drop-down and a closed rig's toast read the same name as the pane headings.
#[test]
fn every_place_reads_the_same_name() {
    let (mut t, id) = on_kettu(Some(RigAddr::Local), "ac2 on kettu");
    assert!(
        t.st.rig_menu_rows()
            .iter()
            .any(|r| matches!(r, RigRow::Rig(r) if *r == id)),
    );
    let mut out = Vec::new();
    t.st.close_rig(id, &mut out);
    // Closed, the rig is still called by the name it went by.
    assert_eq!(t.st.rig_name(id), "kettu");
    assert!(
        t.st.toasts.list.iter().any(|x| x.text == "kettu closed"),
        "{:?}",
        t.st.toasts.list
    );
}

/// A rig reached at a typed address is shown at that address until its advert is seen;
/// from then on at the advert's host name, without the mDNS suffix. An advert of another
/// daemon (another key) names nothing. This computer's own daemon has no host to show.
#[test]
fn a_typed_address_reads_as_the_adverts_host_once_seen() {
    use crate::state::AdvertSeen;
    let addr = RigAddr::Network {
        host: "192.168.9.27".into(),
        port: 47820,
        fingerprint: Some("ab12".into()),
        mdns_host: None,
    };
    let (mut t, id) = on_kettu(Some(addr), "ac2 on pupu");
    fn rig(t: &mut T, id: RigId) -> &mut RigState {
        std::iter::once(&mut t.st.rig)
            .chain(&mut t.st.others.background)
            .find(|r| r.id == id)
            .expect("the rig")
    }
    rig(&mut t, id).daemon_addr = Some("192.168.9.27".into());
    let host = |t: &T| t.st.find_rig(id).and_then(|r| t.st.rig_host(r));
    assert_eq!(host(&t).as_deref(), Some("192.168.9.27"));
    let advert = |fingerprint: &str| AdvertSeen {
        host: "pupu.local".into(),
        fingerprint: fingerprint.into(),
        addresses: vec!["192.168.9.27".parse().expect("an address")],
        port: 47820,
    };
    t.st.adverts_seen(&[advert("ffff")]);
    assert_eq!(host(&t).as_deref(), Some("192.168.9.27"));
    t.st.adverts_seen(&[advert("ab12")]);
    assert_eq!(host(&t).as_deref(), Some("pupu"));
    assert!(matches!(
        &rig(&mut t, id).addr,
        Some(RigAddr::Network { host, mdns_host: Some(m), .. })
            if host == "192.168.9.27" && m == "pupu.local"
    ));

    // This computer: its own transport, or a loopback address.
    let (mut t, id) = on_kettu(Some(RigAddr::Local), "ac2 on kettu");
    assert_eq!(t.st.find_rig(id).and_then(|r| t.st.rig_host(r)), None);
    rig(&mut t, id).daemon_addr = Some("127.0.0.1".into());
    assert_eq!(t.st.find_rig(id).and_then(|r| t.st.rig_host(r)), None);
}

/// The rig `id` of `t`, as held.
fn held(t: &mut T, id: RigId) -> &mut RigState {
    std::iter::once(&mut t.st.rig)
        .chain(&mut t.st.others.background)
        .find(|r| r.id == id)
        .expect("held")
}

/// The rig `id`'s link went down for want of an answer.
fn link_down(t: &mut T, id: RigId) {
    t.st.update(
        Msg::Conn(
            id,
            Box::new(ConnEvent::Failed {
                target: "ac2pi".into(),
                error: "not responding".into(),
                down: ac2_scene::chrome::RigDown::Unreachable,
                retry_in: std::time::Duration::from_secs(1),
            }),
        ),
        &t.keys,
    );
}

/// A network rig at `host`, pinned to key `d4f6`, connected.
fn ac2pi(host: &str) -> (T, RigId) {
    on_kettu(
        Some(RigAddr::Network {
            host: host.into(),
            port: 47820,
            fingerprint: Some("d4f6".into()),
            mdns_host: Some("ac2pi.local".into()),
        }),
        "ac2 on ac2pi",
    )
}

fn ac2pi_advert(fingerprint: &str, addresses: &[&str]) -> crate::state::AdvertSeen {
    crate::state::AdvertSeen {
        host: "ac2pi.local".into(),
        fingerprint: fingerprint.into(),
        addresses: addresses
            .iter()
            .map(|a| a.parse().expect("an address"))
            .collect(),
        port: 47820,
    }
}

fn at(host: &str) -> RigAddr {
    RigAddr::Network {
        host: host.into(),
        port: 47820,
        fingerprint: Some("d4f6".into()),
        mdns_host: Some("ac2pi.local".into()),
    }
}

/// A rig that came back at another address after a reboot: while its link is down, its
/// advert (by the pinned key) moves it there, and the move is remembered.
#[test]
fn a_rig_down_follows_its_advert_to_a_new_address() {
    use crate::state::RigMoved;
    let (mut t, id) = ac2pi("192.168.9.35");
    assert!(!t.st.wants_adverts());
    link_down(&mut t, id);
    assert!(t.st.wants_adverts());
    t.st.prefs_dirty = false;
    let moved =
        t.st.adverts_seen(&[ac2pi_advert("d4f6", &["192.168.9.33"])]);
    let slot = held(&mut t, id).slot;
    assert_eq!(
        moved,
        [RigMoved {
            slot,
            addr: at("192.168.9.33"),
        }]
    );
    assert_eq!(held(&mut t, id).addr, Some(at("192.168.9.33")));
    let p =
        t.st.prefs
            .rigs
            .iter()
            .find(|p| p.id == id)
            .expect("remembered");
    assert_eq!(p.addr, at("192.168.9.33"));
    assert!(t.st.prefs_dirty);
    // Where it answers now is where it is: the same advert again moves nothing.
    assert!(
        t.st.adverts_seen(&[ac2pi_advert("d4f6", &["192.168.9.33"])])
            .is_empty()
    );
}

/// A rig on several networks lists all its addresses: the one it is held at, listed,
/// keeps it there. A connected rig stays where it answers whatever an advert says.
#[test]
fn a_rig_stays_while_connected_or_listed() {
    let (mut t, id) = ac2pi("192.168.9.35");
    let advert = ac2pi_advert("d4f6", &["192.168.9.33"]);
    assert!(t.st.adverts_seen(std::slice::from_ref(&advert)).is_empty());
    assert_eq!(held(&mut t, id).addr, Some(at("192.168.9.35")));

    link_down(&mut t, id);
    let both = ac2pi_advert("d4f6", &["10.0.0.4", "192.168.9.35"]);
    assert!(t.st.adverts_seen(&[both]).is_empty());
    assert_eq!(held(&mut t, id).addr, Some(at("192.168.9.35")));
}

/// Only the pinned key names the rig: an advert of another key moves nothing, nor does one
/// of IPv6 addresses alone (the app's links are IPv4 only).
#[test]
fn another_key_or_ipv6_alone_moves_nothing() {
    let (mut t, id) = ac2pi("192.168.9.35");
    link_down(&mut t, id);
    let other = ac2pi_advert("ffff", &["192.168.9.33"]);
    let v6 = ac2pi_advert("d4f6", &["fd00::33", "fe80::33"]);
    assert!(t.st.adverts_seen(&[other, v6]).is_empty());
    assert_eq!(held(&mut t, id).addr, Some(at("192.168.9.35")));
    let p =
        t.st.prefs
            .rigs
            .iter()
            .find(|p| p.id == id)
            .expect("remembered");
    assert_eq!(p.addr, at("192.168.9.35"));
    // With the key, IPv4 first: the advert ranks its addresses so.
    let mixed = ac2pi_advert("d4f6", &["192.168.9.33", "fd00::33"]);
    assert_eq!(t.st.adverts_seen(&[mixed]).len(), 1);
    assert_eq!(held(&mut t, id).addr, Some(at("192.168.9.33")));
}
