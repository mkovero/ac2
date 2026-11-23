//! Settings › Connection pairs the rig shown with another rig the app holds, both ways in
//! one act and without a session, and unpairs it; the *Paired rigs* rows say each half as
//! the daemons' state shows it.

use super::rig_remote::{KEY_A, KEY_B, local_only, network, server};
use super::*;
use crate::remote_inputs::admit_name;
use crate::settings::{ConnLine, Page};
use ac2_proto::model::{AuthorizedClient, RemoteInput, RigPeer};
use ac2_scene::pairing as words;

const KETUNKOLO: RigId = RigId(0x4b4b);

/// Two rigs held, neither listing nor admitting the other, both in network mode; the app
/// reaches pupu at 192.168.9.27; the Connection page shown.
fn unpaired() -> T {
    let mut t = two_mirrored();
    remirror(&mut t, |s| s.remote.peers.clear());
    if let Some(b) = t.st.others.background.iter_mut().find(|r| r.id == PUPU) {
        b.daemon_addr = Some("192.168.9.27".into());
    }
    to(&mut t, RIG, server(network(KEY_A, Vec::new())));
    to(&mut t, PUPU, server(network(KEY_B, Vec::new())));
    t.st.open_settings(Page::Connection, &mut Vec::new());
    to(&mut t, RIG, server(network(KEY_A, Vec::new())));
    t
}

fn admitted(name: &str, key: &str, rig: RigId) -> AuthorizedClient {
    AuthorizedClient {
        name: name.into(),
        key: key.into(),
        fingerprint: "ab12".into(),
        rig: Some(rig),
    }
}

fn peer(rig: RigId, name: &str, key: &str) -> RigPeer {
    RigPeer {
        rig,
        name: name.into(),
        ctrl: "tcp://192.168.9.27:7700".into(),
        data: "tcp://192.168.9.27:7701".into(),
        key: key.into(),
    }
}

/// Both daemons as they are once paired both ways.
fn paired_both(t: &mut T) {
    let me = t.st.rig_label(&t.st.rig);
    remirror(t, |s| s.remote.peers = vec![peer(PUPU, "pupu", KEY_B)]);
    to(
        t,
        RIG,
        server(network(
            KEY_A,
            vec![admitted(&admit_name("pupu"), KEY_B, PUPU)],
        )),
    );
    let mut s = pupu(t).daemon().cloned().expect("pupu's state");
    s.remote.peers = vec![peer(RIG, &me, KEY_A)];
    to(t, PUPU, mirror(s));
    to(
        t,
        PUPU,
        server(network(KEY_B, vec![admitted(&admit_name(&me), KEY_A, RIG)])),
    );
}

/// The rig shown's daemon state, changed by `f`.
fn remirror(t: &mut T, f: impl FnOnce(&mut State)) {
    let mut s = t.st.daemon().cloned().expect("a daemon state");
    f(&mut s);
    to(t, RIG, mirror(s));
}

fn conn_page(t: &T) -> &crate::settings::ConnectionPage {
    match &t.st.overlay {
        Overlay::Settings(s) => &s.connection,
        o => panic!("settings open: {o:?}"),
    }
}

/// Clicks (Enter on) the first line `l`.
fn click(t: &mut T, l: &ConnLine) -> Vec<(RigId, Request)> {
    let i = conn_page(t)
        .lines()
        .iter()
        .position(|x| x == l)
        .unwrap_or_else(|| panic!("{l:?} in {:?}", conn_page(t).lines()));
    t.st.update(Msg::Settings(SettingsMsg::Connection(i)), &t.keys)
}

fn key(t: &mut T, s: &str) -> Vec<(RigId, Request)> {
    let c = Chord::parse(s).expect(s);
    t.st.update(Msg::Key(c), &t.keys)
}

/// Delete on line `l`.
fn delete_on(t: &mut T, l: &ConnLine) -> Vec<(RigId, Request)> {
    let i = conn_page(t)
        .lines()
        .iter()
        .position(|x| x == l)
        .expect("the line");
    if let Overlay::Settings(s) = &mut t.st.overlay {
        s.connection.focus = i;
    }
    t.st.update(Msg::Key(Chord::parse("Delete").expect("Delete")), &t.keys)
}

fn admits(r: &[(RigId, Request)], on: RigId) -> Vec<(String, RigId, String, String)> {
    r.iter()
        .filter_map(|(rig, q)| match q {
            Request::ServerCall {
                cmd: Command::AuthAdmitRig { key, rig_id, name },
                what,
                ..
            } if *rig == on => Some((key.clone(), *rig_id, name.clone(), what.clone())),
            _ => None,
        })
        .collect()
}

fn lists(r: &[(RigId, Request)], on: RigId) -> Vec<(Vec<RigPeer>, String)> {
    r.iter()
        .filter_map(|(rig, q)| match q {
            Request::Act {
                cmd: Command::RigPeers { peers },
                done,
                ..
            } if *rig == on => Some((peers.clone(), done.clone())),
            _ => None,
        })
        .collect()
}

fn revokes(r: &[(RigId, Request)], on: RigId) -> Vec<String> {
    r.iter()
        .filter_map(|(rig, q)| match q {
            Request::ServerCall {
                cmd: Command::ServerRevoke { name },
                ..
            } if *rig == on => Some(name.clone()),
            _ => None,
        })
        .collect()
}

/// *Pair with rig…* lists pupu by name; picking it has each rig admit the other and list
/// the other, in one act and with no session reopened; once both daemons show it, the row
/// says paired both ways and pupu is offered no more.
#[test]
fn pair_with_rig_pairs_both_ways_in_one_act() {
    let mut t = unpaired();
    let me = t.st.rig_label(&t.st.rig);
    let r = click(&mut t, &ConnLine::PairWith);
    assert!(
        r.iter()
            .any(|(rig, q)| *rig == PUPU && matches!(q, Request::ServerInfo)),
        "asks pupu for its key: {r:?}"
    );
    let offers = conn_page(&t).pairing.offers.clone().expect("the pick open");
    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0].text, "pupu");
    assert_eq!(offers[0].refusal, None);
    assert_eq!(
        conn_page(&t).focused(),
        Some(ConnLine::PairOffer(PUPU)),
        "the pick takes the focus"
    );
    let r = key(&mut t, "Enter");
    // pupu admits this rig and lists it; this rig admits pupu and lists it.
    assert_eq!(
        admits(&r, PUPU),
        vec![(
            KEY_A.to_string(),
            RIG,
            admit_name(&me),
            ac2_scene::remote_inputs::admitted_toast("pupu", &me)
        )]
    );
    assert_eq!(
        admits(&r, RIG),
        vec![(
            KEY_B.to_string(),
            PUPU,
            admit_name("pupu"),
            ac2_scene::remote_inputs::admitted_toast(&me, "pupu")
        )]
    );
    let here = lists(&r, RIG);
    assert_eq!(here.len(), 1);
    assert_eq!(here[0].0, vec![peer(PUPU, "pupu", KEY_B)]);
    assert_eq!(here[0].1, words::listed_toast(&me, "pupu"));
    let there = lists(&r, PUPU);
    assert_eq!(there.len(), 1);
    assert_eq!(there[0].0.len(), 1);
    assert_eq!(there[0].0[0].rig, RIG);
    assert_eq!(there[0].0[0].key, KEY_A);
    assert_eq!(there[0].1, words::listed_toast("pupu", &me));
    assert!(
        !r.iter().any(|(_, q)| matches!(
            q,
            Request::RemoteSession { .. } | Request::OpenSession { .. }
        )),
        "no session touched: {r:?}"
    );
    assert!(conn_page(&t).pairing.offers.is_none(), "the pick closed");
    // This rig's half is in place, pupu's not yet: the row says what is missing.
    remirror(&mut t, |s| s.remote.peers = vec![peer(PUPU, "pupu", KEY_B)]);
    key(&mut t, "Down");
    let row = conn_page(&t)
        .pairing
        .row(PUPU)
        .cloned()
        .expect("pupu's row");
    assert_eq!(
        row.text,
        "pupu · has not admitted this rig · does not list this rig · not admitted by this rig"
    );
    assert!(row.mend && row.held);
    assert!(
        row.tip.contains("  this rig lists pupu as paired"),
        "{}",
        row.tip
    );
    // Both daemons show both halves.
    paired_both(&mut t);
    key(&mut t, "Down");
    let row = conn_page(&t)
        .pairing
        .row(PUPU)
        .cloned()
        .expect("pupu's row");
    assert_eq!(row.text, "pupu · paired both ways");
    assert!(!row.mend);
    // Paired both ways: not offered again, and the pick says so.
    click(&mut t, &ConnLine::PairWith);
    assert!(conn_page(&t).pairing.offers.is_none());
    assert_eq!(conn_page(&t).notice.as_deref(), Some(words::ALL_PAIRED));
}

/// A half missing: Enter on the row sends only that half.
#[test]
fn enter_on_a_half_paired_row_sends_the_missing_halves_only() {
    let mut t = unpaired();
    paired_both(&mut t);
    // pupu's admission of this rig is gone.
    to(&mut t, PUPU, server(network(KEY_B, Vec::new())));
    key(&mut t, "Down");
    let row = conn_page(&t)
        .pairing
        .row(PUPU)
        .cloned()
        .expect("pupu's row");
    assert_eq!(row.text, "pupu · has not admitted this rig");
    let r = click(&mut t, &ConnLine::Paired(PUPU));
    assert_eq!(admits(&r, PUPU).len(), 1, "{r:?}");
    assert!(admits(&r, RIG).is_empty(), "{r:?}");
    assert!(
        lists(&r, RIG).is_empty() && lists(&r, PUPU).is_empty(),
        "{r:?}"
    );
}

/// Refused before anything is sent: no other rig held; the other rig off the network.
#[test]
fn pairing_refusals_send_nothing() {
    let mut t = T::new();
    to(&mut t, RIG, mirror(with_spl()));
    to(&mut t, RIG, server(network(KEY_A, Vec::new())));
    t.st.open_settings(Page::Connection, &mut Vec::new());
    to(&mut t, RIG, server(network(KEY_A, Vec::new())));
    let r = click(&mut t, &ConnLine::PairWith);
    assert_eq!(conn_page(&t).error.as_deref(), Some(words::NO_OTHER_RIG));
    assert!(conn_page(&t).pairing.offers.is_none());
    assert!(
        r.iter().all(|(_, q)| matches!(q, Request::ServerInfo)),
        "{r:?}"
    );

    let mut t = unpaired();
    to(&mut t, PUPU, server(local_only()));
    click(&mut t, &ConnLine::PairWith);
    let want = words::refusal("pupu", &words::PairRefusal::LocalOnly);
    let offers = conn_page(&t).pairing.offers.clone().expect("the pick open");
    assert_eq!(offers[0].text, want);
    let r = key(&mut t, "Enter");
    assert_eq!(conn_page(&t).error.as_deref(), Some(want.as_str()));
    assert!(
        admits(&r, PUPU).is_empty() && lists(&r, RIG).is_empty(),
        "{r:?}"
    );
    // Esc closes the pick, not the page.
    key(&mut t, "Escape");
    assert!(conn_page(&t).pairing.offers.is_none());
}

/// Delete twice unpairs both ways: each rig drops the other and revokes its admission.
#[test]
fn delete_twice_unpairs_both_ways() {
    let mut t = unpaired();
    let me = t.st.rig_label(&t.st.rig);
    paired_both(&mut t);
    key(&mut t, "Down");
    let r = delete_on(&mut t, &ConnLine::Paired(PUPU));
    assert!(lists(&r, RIG).is_empty(), "first Delete asks: {r:?}");
    assert_eq!(
        conn_page(&t).notice.as_deref(),
        Some(words::unpair_confirm("pupu", true).as_str())
    );
    let r = key(&mut t, "Delete");
    assert_eq!(
        lists(&r, RIG),
        vec![(Vec::new(), words::unlisted_toast(&me, "pupu"))]
    );
    assert_eq!(
        lists(&r, PUPU),
        vec![(Vec::new(), words::unlisted_toast("pupu", &me))]
    );
    assert_eq!(revokes(&r, RIG), vec![admit_name("pupu")]);
    assert_eq!(revokes(&r, PUPU), vec![admit_name(&me)]);
}

/// A session taking an input from the other rig refuses the unpairing, naming it.
#[test]
fn unpairing_is_refused_while_a_session_takes_that_rigs_input() {
    let mut t = unpaired();
    let me = t.st.rig_label(&t.st.rig);
    paired_both(&mut t);
    remirror(&mut t, |s| {
        if let Some(o) = &mut s.session.open {
            o.config.remote_inputs = vec![RemoteInput {
                slot: 0,
                rig: PUPU,
                rig_name: "pupu".into(),
                channel: 0,
            }];
        }
    });
    key(&mut t, "Down");
    delete_on(&mut t, &ConnLine::Paired(PUPU));
    let r = key(&mut t, "Delete");
    assert!(
        lists(&r, RIG).is_empty() && revokes(&r, PUPU).is_empty(),
        "{r:?}"
    );
    assert_eq!(
        conn_page(&t).error.as_deref(),
        Some(words::unpair_inputs(&me, &["pupu in 1".into()]).as_str())
    );
}

/// A rig the app does not hold: its halves are not claimed, and unpairing does this rig's
/// side and says what remains.
#[test]
fn a_rig_not_held_is_unpaired_on_this_side_only() {
    let mut t = unpaired();
    let me = t.st.rig_label(&t.st.rig);
    remirror(&mut t, |s| {
        s.remote.peers = vec![peer(KETUNKOLO, "ketunkolo", KEY_B)]
    });
    to(
        &mut t,
        RIG,
        server(network(
            KEY_A,
            vec![admitted("rig-ketunkolo", KEY_B, KETUNKOLO)],
        )),
    );
    key(&mut t, "Down");
    let row = conn_page(&t)
        .pairing
        .row(KETUNKOLO)
        .cloned()
        .expect("ketunkolo's row");
    assert_eq!(row.text, "ketunkolo · its side: can't tell");
    assert!(!row.held && !row.mend);
    delete_on(&mut t, &ConnLine::Paired(KETUNKOLO));
    assert_eq!(
        conn_page(&t).notice.as_deref(),
        Some(words::unpair_confirm("ketunkolo", false).as_str())
    );
    let r = key(&mut t, "Delete");
    assert_eq!(
        lists(&r, RIG),
        vec![(Vec::new(), words::unlisted_toast(&me, "ketunkolo"))]
    );
    assert_eq!(revokes(&r, RIG), vec!["rig-ketunkolo".to_string()]);
    assert!(r.iter().all(|(rig, _)| *rig == RIG), "{r:?}");
    assert_eq!(
        conn_page(&t).notice.as_deref(),
        Some(words::unpair_rest("ketunkolo").as_str())
    );
}
