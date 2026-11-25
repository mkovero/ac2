//! Settings › Connection pairs the rig shown with another rig the app holds, both ways in
//! one act and without a stream, and unpairs it; the *Paired rigs* rows say each half as
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

/// A frame tick: the pairing rows are worked out again.
fn tick(t: &mut T) {
    t.st.update(
        Msg::Tick {
            now_s: 0.0,
            dt_s: 0.0,
        },
        &t.keys,
    );
}

/// The app has lost rig `rig`: its last mirror and `server.info` stay, stale.
fn lose(t: &mut T, rig: RigId) {
    let r = if t.st.rig.id == rig {
        &mut t.st.rig
    } else {
        t.st.others
            .background
            .iter_mut()
            .find(|r| r.id == rig)
            .expect("held")
    };
    r.conn = ConnState::Connecting {
        target: "tcp://192.168.9.27:7700".into(),
    };
    tick(t);
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

fn lists(r: &[(RigId, Request)], on: RigId) -> Vec<(RigPeer, String)> {
    r.iter()
        .filter_map(|(rig, q)| match q {
            Request::Act {
                cmd: Command::RigPair { peer },
                done,
                ..
            } if *rig == on => Some((peer.clone(), done.clone())),
            _ => None,
        })
        .collect()
}

fn unlists(r: &[(RigId, Request)], on: RigId) -> Vec<(RigId, String)> {
    r.iter()
        .filter_map(|(rig, q)| match q {
            Request::Act {
                cmd: Command::RigUnpair { rig_id: gone },
                done,
                ..
            } if *rig == on => Some((*gone, done.clone())),
            _ => None,
        })
        .collect()
}

fn revokes(r: &[(RigId, Request)], on: RigId) -> Vec<RigId> {
    r.iter()
        .filter_map(|(rig, q)| match q {
            Request::ServerCall {
                cmd: Command::AuthRevokeRig { rig_id },
                ..
            } if *rig == on => Some(*rig_id),
            _ => None,
        })
        .collect()
}

/// *Pair with rig…* lists pupu by name; picking it has each rig admit the other and list
/// the other, in one act and with no stream reopened; once both daemons show it, the row
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
    assert_eq!(here[0].0, peer(PUPU, "pupu", KEY_B));
    assert_eq!(here[0].1, words::listed_toast(&me, "pupu"));
    let there = lists(&r, PUPU);
    assert_eq!(there.len(), 1);
    assert_eq!(there[0].0.rig, RIG);
    assert_eq!(there[0].0.key, KEY_A);
    assert_eq!(there[0].1, words::listed_toast("pupu", &me));
    assert!(
        !r.iter()
            .any(|(_, q)| matches!(q, Request::RemoteStream { .. } | Request::OpenStream { .. })),
        "no stream touched: {r:?}"
    );
    assert!(conn_page(&t).pairing.offers.is_none(), "the pick closed");
    // This rig's half is in place, pupu's not yet: the row says what is missing.
    remirror(&mut t, |s| s.remote.peers = vec![peer(PUPU, "pupu", KEY_B)]);
    tick(&mut t);
    let row = conn_page(&t)
        .pairing
        .row(PUPU)
        .cloned()
        .expect("pupu's row");
    assert_eq!(row.text, "pupu · not paired · 1 of 4 halves in place");
    assert!(row.mend() && row.unreached.is_none());
    assert!(
        row.tip.contains("  this rig lists pupu as paired"),
        "{}",
        row.tip
    );
    // Both daemons show both halves.
    paired_both(&mut t);
    tick(&mut t);
    let row = conn_page(&t)
        .pairing
        .row(PUPU)
        .cloned()
        .expect("pupu's row");
    assert_eq!(row.text, "pupu · paired both ways");
    assert!(!row.mend());
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
    tick(&mut t);
    let row = conn_page(&t)
        .pairing
        .row(PUPU)
        .cloned()
        .expect("pupu's row");
    assert_eq!(
        row.text,
        "pupu · it takes this rig's inputs · not the other way"
    );
    assert!(
        row.tip
            .ends_with("Enter pairs the missing halves · Delete twice unpairs"),
        "{}",
        row.tip
    );
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
    tick(&mut t);
    let r = delete_on(&mut t, &ConnLine::Paired(PUPU));
    assert!(lists(&r, RIG).is_empty(), "first Delete asks: {r:?}");
    assert_eq!(
        conn_page(&t).notice.as_deref(),
        Some("Delete again unpairs pupu both ways")
    );
    let r = key(&mut t, "Delete");
    assert_eq!(
        unlists(&r, RIG),
        vec![(PUPU, words::unlisted_toast(&me, "pupu"))]
    );
    assert_eq!(
        unlists(&r, PUPU),
        vec![(RIG, words::unlisted_toast("pupu", &me))]
    );
    // By the rig's identity, never by a name an app could hold.
    assert_eq!(revokes(&r, RIG), vec![PUPU]);
    assert_eq!(revokes(&r, PUPU), vec![RIG]);
    assert_eq!(conn_page(&t).notice, None);
}

/// A stream taking an input from the other rig refuses the unpairing, naming it.
#[test]
fn unpairing_is_refused_while_a_stream_takes_that_rigs_input() {
    let mut t = unpaired();
    let me = t.st.rig_label(&t.st.rig);
    paired_both(&mut t);
    remirror(&mut t, |s| {
        if let Some(o) = &mut s.stream.open {
            o.config.remote_inputs = vec![RemoteInput {
                slot: 0,
                rig: PUPU,
                rig_name: "pupu".into(),
                channel: 0,
            }];
        }
    });
    tick(&mut t);
    delete_on(&mut t, &ConnLine::Paired(PUPU));
    let r = key(&mut t, "Delete");
    assert!(
        unlists(&r, RIG).is_empty() && revokes(&r, PUPU).is_empty(),
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
    tick(&mut t);
    let row = conn_page(&t)
        .pairing
        .row(KETUNKOLO)
        .cloned()
        .expect("ketunkolo's row");
    // This rig's own entries are there; only the key they pin cannot be checked.
    assert_eq!(
        row.text,
        "ketunkolo · listed and admitted by this rig · can't tell its side"
    );
    assert_eq!(row.unreached, Some(words::Unseen::NotHeld));
    assert!(!row.mend());
    assert!(
        row.tip.contains(
            "  this rig lists ketunkolo as paired (can't check its key: the app does not hold \
             ketunkolo"
        ),
        "{}",
        row.tip
    );
    assert!(
        !row.tip.contains("can't tell whether this rig"),
        "{}",
        row.tip
    );
    assert!(row.tip.ends_with("\nDelete twice unpairs"), "{}", row.tip);
    // Enter claims nothing: it says what cannot be told.
    click(&mut t, &ConnLine::Paired(KETUNKOLO));
    assert_eq!(
        conn_page(&t).notice.as_deref(),
        Some(words::cant_tell("ketunkolo", &row.halves).as_str())
    );
    delete_on(&mut t, &ConnLine::Paired(KETUNKOLO));
    let ask = words::unpair_confirm("ketunkolo", Some(words::Unseen::NotHeld), &row.halves);
    assert!(
        ask.contains("Can't tell whether ketunkolo's stream"),
        "{ask}"
    );
    assert_eq!(conn_page(&t).notice.as_deref(), Some(ask.as_str()));
    let r = key(&mut t, "Delete");
    assert_eq!(
        unlists(&r, RIG),
        vec![(KETUNKOLO, words::unlisted_toast(&me, "ketunkolo"))]
    );
    assert_eq!(revokes(&r, RIG), vec![KETUNKOLO]);
    assert!(r.iter().all(|(rig, _)| *rig == RIG), "{r:?}");
    assert_eq!(
        conn_page(&t).notice.as_deref(),
        Some(words::unpair_rest("ketunkolo", words::Unseen::NotHeld).as_str())
    );
}

/// A rig held but not connected now is one the app cannot reach: its stale state is not
/// read, its halves are not told, and unpairing does this rig's side only and says so.
#[test]
fn a_rig_held_but_lost_is_not_read_and_unpaired_on_this_side_only() {
    let mut t = unpaired();
    paired_both(&mut t);
    lose(&mut t, PUPU);
    let row = conn_page(&t)
        .pairing
        .row(PUPU)
        .cloned()
        .expect("pupu's row");
    assert_eq!(
        row.text,
        "pupu · listed and admitted by this rig · can't tell its side"
    );
    assert_eq!(row.unreached, Some(words::Unseen::NotConnected));
    assert!(
        row.tip.contains(
            "can't tell whether pupu has admitted this rig (the app is not connected to pupu now)"
        ),
        "{}",
        row.tip
    );
    delete_on(&mut t, &ConnLine::Paired(PUPU));
    assert_eq!(
        conn_page(&t).notice.as_deref(),
        Some(
            words::unpair_confirm("pupu", Some(words::Unseen::NotConnected), &row.halves).as_str()
        )
    );
    let r = key(&mut t, "Delete");
    assert_eq!(unlists(&r, RIG).len(), 1, "{r:?}");
    assert_eq!(revokes(&r, RIG), vec![PUPU]);
    assert!(r.iter().all(|(rig, _)| *rig == RIG), "{r:?}");
    assert_eq!(
        conn_page(&t).notice.as_deref(),
        Some(words::unpair_rest("pupu", words::Unseen::NotConnected).as_str())
    );
}

/// The rig shown not connected: its halves are not told, and unpairing is refused.
#[test]
fn unpairing_is_refused_while_the_rig_shown_is_lost() {
    let mut t = unpaired();
    let me = t.st.rig_label(&t.st.rig);
    paired_both(&mut t);
    lose(&mut t, RIG);
    let row = conn_page(&t)
        .pairing
        .row(PUPU)
        .cloned()
        .expect("pupu's row");
    assert_eq!(
        row.text,
        "pupu · lists this rig · has admitted this rig · can't tell"
    );
    delete_on(&mut t, &ConnLine::Paired(PUPU));
    let r = key(&mut t, "Delete");
    assert!(
        unlists(&r, RIG).is_empty() && unlists(&r, PUPU).is_empty(),
        "{r:?}"
    );
    assert_eq!(
        conn_page(&t).error.as_deref(),
        Some(words::refusal(&me, &words::PairRefusal::NotConnected).as_str())
    );
}

/// This rig's own network off: its admissions are not listed, so its half is said as
/// this rig's that cannot be told; Enter says so, and unpairing says the admission stays.
#[test]
fn this_rigs_unseen_admission_is_said_as_this_rigs() {
    let mut t = unpaired();
    paired_both(&mut t);
    // Off the network a rig keeps its key: only its admitted rigs go unlisted.
    let mut off = local_only();
    off.network.server_key = Some(KEY_A.into());
    to(&mut t, RIG, server(off));
    let row = conn_page(&t)
        .pairing
        .row(PUPU)
        .cloned()
        .expect("pupu's row");
    assert_eq!(
        row.text,
        "pupu · this rig takes its inputs · other way: can't tell"
    );
    assert!(
        row.tip
            .contains("can't tell whether this rig has admitted pupu (this rig's network is off)"),
        "{}",
        row.tip
    );
    assert!(!row.tip.contains("pupu's network"), "{}", row.tip);
    click(&mut t, &ConnLine::Paired(PUPU));
    assert_eq!(
        conn_page(&t).notice.as_deref(),
        Some(words::cant_tell("pupu", &row.halves).as_str())
    );
    delete_on(&mut t, &ConnLine::Paired(PUPU));
    let ask = conn_page(&t).notice.clone().unwrap_or_default();
    assert!(!ask.contains("both ways"), "{ask}");
    let r = key(&mut t, "Delete");
    assert_eq!(unlists(&r, RIG).len(), 1, "{r:?}");
    assert!(revokes(&r, RIG).is_empty(), "{r:?}");
    assert_eq!(revokes(&r, PUPU), vec![RIG]);
    assert_eq!(
        conn_page(&t).notice.as_deref(),
        Some(
            words::unpair_admission_unseen("pupu", words::Side::Here, words::Unseen::NetworkOff)
                .as_str()
        )
    );
}

/// The other rig's admissions unseen: the confirm does not promise both ways, its
/// admission is not revoked blind, and the page says it may stay.
#[test]
fn the_other_rigs_unseen_admission_is_said_and_not_promised() {
    let mut t = unpaired();
    paired_both(&mut t);
    to(
        &mut t,
        PUPU,
        ConnEvent::Server {
            what: None,
            result: Err("timed out".into()),
        },
    );
    tick(&mut t);
    delete_on(&mut t, &ConnLine::Paired(PUPU));
    let ask = conn_page(&t).notice.clone().unwrap_or_default();
    assert_eq!(
        ask,
        "Delete again unpairs pupu as far as the app sees. Can't tell whether pupu admits \
         this rig (pupu did not answer the app): an admission not seen stays"
    );
    let r = key(&mut t, "Delete");
    assert_eq!(unlists(&r, RIG).len(), 1, "{r:?}");
    assert_eq!(unlists(&r, PUPU).len(), 1, "{r:?}");
    assert_eq!(revokes(&r, RIG), vec![PUPU]);
    assert!(revokes(&r, PUPU).is_empty(), "{r:?}");
    assert_eq!(
        conn_page(&t).notice.as_deref(),
        Some(
            "Can't tell whether pupu admits this rig (pupu did not answer the app): if it \
             does, that stays; revoke it there under Authorized clients once seen"
        )
    );
}

/// A rig the taker already pairs under the other rig's name is refused before anything
/// is sent: the daemon would refuse it, and the operator is told which rig to unpair.
#[test]
fn pairing_under_a_name_another_rig_holds_is_refused() {
    let mut t = unpaired();
    let me = t.st.rig_label(&t.st.rig);
    remirror(&mut t, |s| {
        s.remote.peers = vec![peer(KETUNKOLO, "pupu", KEY_B)]
    });
    click(&mut t, &ConnLine::PairWith);
    let offers = conn_page(&t).pairing.offers.clone().expect("the pick open");
    let want = format!(
        "{me} already pairs another rig named pupu: unpair that one on {me} first (Settings › \
         Connection)"
    );
    assert_eq!(offers[0].refusal.as_deref(), Some(want.as_str()));
    let r = key(&mut t, "Enter");
    assert!(
        admits(&r, PUPU).is_empty() && lists(&r, RIG).is_empty(),
        "{r:?}"
    );
}
