//! A rig on the app's own host is named by that host and how the app reaches it: two
//! daemons on one machine never read as two machines.

use super::*;
use crate::prefs::RigAddr;

/// The second rig, reached as `addr` on host `kettu`, said hello as `name`.
fn on_kettu(addr: Option<RigAddr>, embedded: bool, name: &str) -> (T, RigId) {
    let mut t = T::new();
    t.st.local_host = "kettu".into();
    let slot = t.st.add_rig("local".into(), None, addr);
    let id = RigId(0x77);
    if let Some(r) = t.st.rig_in_slot_mut(slot) {
        r.link_embedded = embedded;
    }
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
/// one given, and two daemons on one host must read as one host.
#[test]
fn the_local_daemon_is_the_host_and_ac2d() {
    for name in ["ac2 on kettu", "FOH"] {
        let (t, id) = on_kettu(Some(RigAddr::Local), false, name);
        assert_eq!(t.st.rig_name(id), "kettu · ac2d");
    }
}

#[test]
fn the_daemon_in_this_app_is_the_host_and_in_this_app() {
    let (t, id) = on_kettu(
        Some(RigAddr::Embedded("jack".into())),
        true,
        "this computer",
    );
    assert_eq!(t.st.rig_name(id), "kettu · in this app");
}

#[test]
fn a_network_rig_keeps_its_own_name() {
    let addr = RigAddr::Network {
        host: "192.168.9.27".into(),
        port: 47820,
        fingerprint: None,
    };
    let (t, id) = on_kettu(Some(addr.clone()), false, "pupu");
    assert_eq!(t.st.rig_name(id), "pupu");
    let (t, id) = on_kettu(Some(addr), false, "ac2 on pupu");
    assert_eq!(t.st.rig_name(id), "ac2 on pupu");
}

/// The drop-down and a closed rig's toast read the same name as the pane headings.
#[test]
fn every_place_reads_the_same_name() {
    let (mut t, id) = on_kettu(Some(RigAddr::Local), false, "ac2 on kettu");
    assert!(
        t.st.rig_menu_rows()
            .iter()
            .any(|r| matches!(r, RigRow::Rig(r) if *r == id)),
    );
    let mut out = Vec::new();
    t.st.close_rig(id, &mut out);
    // Closed, the rig is still called by the name it went by.
    assert_eq!(t.st.rig_name(id), "kettu · ac2d");
    assert!(
        t.st.toasts
            .list
            .iter()
            .any(|x| x.text == "kettu · ac2d closed"),
        "{:?}",
        t.st.toasts.list
    );
}
