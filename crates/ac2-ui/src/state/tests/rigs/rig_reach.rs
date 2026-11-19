//! What of another rig reaches the screen: the list is the rig shown and the rigs its
//! daemon is paired with, and only those rigs' rows and curves are offered, taken and
//! drawn. A rig the app merely holds is the drop-down's.

use super::*;
use ac2_scene::meas_list::TreeKey;

/// A rig the app holds that no rig is paired with.
const KETTU: RigId = RigId(0x4e77);

/// Rig `id` named `name` added in the background, connected and mirroring `state`.
fn add(t: &mut T, id: RigId, name: &str, state: State) {
    let slot = t.st.add_rig(name.into(), None, None);
    to(
        t,
        RigId(u64::from(slot)),
        ConnEvent::Connected {
            target: name.into(),
            server: "ac2d test".into(),
            client_id: ClientId("c3".into()),
            rig_id: id,
            name: name.into(),
        },
    );
    to(t, id, mirror(state));
}

/// The rig shown paired with pupu, and kettu held beside them, paired with neither; the
/// focused pane.
fn three_rigs() -> (T, PaneId) {
    let (mut t, p) = two_lists();
    let mut s = with_spl();
    s.measurements[0].config.name = "Kettu sub".into();
    add(&mut t, KETTU, "kettu", s);
    (t, p)
}

/// The list's headings: each rig with its first line.
fn heads(t: &T) -> Vec<(RigId, String)> {
    t.st.list_rows()
        .into_iter()
        .filter(|(_, r)| matches!(r.key, TreeKey::RigName(_)))
        .map(|(rig, r)| (rig, r.name))
        .collect()
}

fn pick_rig(t: &mut T, rig: RigId) {
    t.st.update(Msg::RigPick(RigRow::Rig(rig)), &t.keys);
    assert_eq!(t.st.active(), rig);
}

/// A rig the app holds that the rig shown is not paired with has no heading and no rows;
/// the drop-down still lists it. Each heading says the rig's rate beside its name.
#[test]
fn a_rig_held_and_not_paired_is_not_listed() {
    let (t, _) = three_rigs();
    assert_eq!(
        heads(&t),
        [
            (RIG, "test rig · 48 kHz".to_owned()),
            (PUPU, "pupu · 48 kHz".to_owned())
        ]
    );
    assert!(t.st.list_rows().iter().all(|(r, _)| *r != KETTU));
    assert!(t.st.rig_menu_rows().contains(&RigRow::Rig(KETTU)));
    assert!(t.st.rig_in_reach(RIG) && t.st.rig_in_reach(PUPU));
    assert!(!t.st.rig_in_reach(KETTU));
}

/// The drop-down changes the rig shown and with it the whole list: kettu shown lists kettu
/// alone; pupu shown lists pupu and the rig paired with it, whichever side took the other.
#[test]
fn switching_the_rig_shown_changes_the_list() {
    let (mut t, _) = three_rigs();
    pick_rig(&mut t, KETTU);
    assert_eq!(heads(&t), [(KETTU, "kettu · 48 kHz".to_owned())]);
    pick_rig(&mut t, PUPU);
    let listed: Vec<RigId> = heads(&t).into_iter().map(|(r, _)| r).collect();
    assert_eq!(listed, [PUPU, RIG]);
    let pairing =
        t.st.list_rows()
            .into_iter()
            .find(|(_, r)| r.key == TreeKey::RigName(RIG))
            .and_then(|(_, r)| r.details.last().cloned());
    assert_eq!(pairing.as_deref(), Some("paired"));
}

/// A rig with nothing measured and no audio I/O says so where its rate would be, in the
/// top bar's words.
#[test]
fn a_rig_without_a_session_says_so_beside_its_name() {
    let (mut t, _) = two_lists();
    let mut s = pupu(&t).daemon().cloned().expect("pupu's state");
    s.session.open = None;
    to(&mut t, PUPU, mirror(s.clone()));
    assert_eq!(heads(&t)[1].1, "pupu · no audio I/O");
    s.measurements.clear();
    to(&mut t, PUPU, mirror(s));
    assert_eq!(heads(&t)[1].1, "pupu · no session");
}

/// Tab steps through the list as shown: past the rig shown's rows onto pupu's, never onto
/// kettu's, which the list does not show.
#[test]
fn tab_stays_within_the_rigs_listed() {
    let (mut t, _) = three_rigs();
    let tab = Chord::parse("Tab").expect("Tab");
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..40 {
        t.st.update(Msg::Key(tab), &t.keys);
        seen.insert(t.st.active());
    }
    assert_eq!(seen.into_iter().collect::<Vec<_>>(), [RIG, PUPU]);
    // Kettu shown: nothing paired, so Tab wraps within kettu.
    pick_rig(&mut t, KETTU);
    for _ in 0..40 {
        t.st.update(Msg::Key(tab), &t.keys);
        assert_eq!(t.st.active(), KETTU);
    }
}

/// A row of a rig not paired with the rig shown is no drop: the drag offers nothing, the
/// pane keeps what it shows, and a click on such a row (the pairing gone under the pointer)
/// shows no other rig.
#[test]
fn a_rig_not_paired_gives_no_curve_to_a_pane() {
    let (mut t, p) = three_rigs();
    let before = *t.st.layout.view(p).expect("pane");
    let key = TreeKey::Meas(MeasId(2));
    t.st.apply(
        Msg::RowDrag(DragMsg::Start(DragItem::Row { rig: KETTU, key })),
        &t.keys,
    );
    assert!(t.st.row_drag().is_none(), "nothing to drag");
    t.drag_drop(DragItem::Row { rig: KETTU, key }, p, None);
    assert_eq!(*t.st.layout.view(p).expect("pane"), before);
    t.st.pane_row_from(p, KETTU, key);
    assert_eq!(*t.st.layout.view(p).expect("pane"), before);
    t.st.update(
        Msg::OnRig(KETTU, Box::new(Msg::SelectMeas(MeasId(2)))),
        &t.keys,
    );
    assert_eq!(t.st.active(), RIG);
    // Pupu's, paired, is taken.
    drop_pupu(&mut t, p);
    assert_eq!(t.st.layout.view(p).expect("pane").rig, PaneRig::Rig(PUPU));
}

/// A pane given a measurement on the rig shown, then kettu shown: the pane keeps its rig
/// and measurement, out of reach (the view says the two rigs are not paired) and deaf to
/// its keys; back on the first rig it shows its measurement as before. The same pane with
/// pupu shown is in reach: the two are paired.
#[test]
fn a_pane_out_of_reach_keeps_its_content_and_comes_back() {
    let (mut t, p) = three_rigs();
    t.st.layout.view_mut(p).expect("pane").kind = PaneKind::Spectrum;
    t.st.layout.view_mut(p).expect("pane").meas = Some(MeasId(2));
    pick_rig(&mut t, KETTU);
    let v = *t.st.layout.view(p).expect("pane");
    assert_eq!((v.rig, v.meas), (PaneRig::Rig(RIG), Some(MeasId(2))));
    assert!(!t.st.rig_in_reach(t.st.pane_rig(&v)));
    t.st.layout.set_focus(p);
    let r = t.st.update(Msg::Command(CommandId::Peaks), &t.keys);
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(*t.st.layout.view(p).expect("pane"), v, "kept as it was");
    pick_rig(&mut t, PUPU);
    assert!(t.st.rig_in_reach(RIG), "pupu and the first rig are paired");
    pick_rig(&mut t, RIG);
    let v = *t.st.layout.view(p).expect("pane");
    assert_eq!((v.rig, v.meas), (PaneRig::Active, Some(MeasId(2))));
    assert_eq!(t.st.pane_meas(p).map(|m| m.id), Some(MeasId(2)));
}

/// A pane the layout prefs pin to a rig out of reach is kept as stored: its rig and
/// measurement by name are written back unchanged.
#[test]
fn a_stored_pane_of_a_rig_out_of_reach_is_kept() {
    let (mut t, p) = three_rigs();
    t.st.layout.view_mut(p).expect("pane").kind = PaneKind::Spectrum;
    t.st.layout.view_mut(p).expect("pane").meas = Some(MeasId(2));
    pick_rig(&mut t, KETTU);
    let v = *t.st.layout.view(p).expect("pane");
    assert_eq!(
        t.st.pane_pref_rig(&v).map(|(r, _)| r),
        Some(RIG),
        "the prefs name its rig"
    );
    assert_eq!(t.st.pane_pref_meas(p, &v).as_deref(), Some("Sub"));
}

/// A rig the rig shown is paired with and this app does not hold: its heading alone, by
/// the name the rig shown's daemon has for it, saying the pairing and that this app is not
/// connected; nothing to fold, no rows, and a click on it shows no rig.
#[test]
fn a_paired_rig_not_held_is_a_heading_alone() {
    const CM4: RigId = RigId(0xc4);
    let mut t = T::new();
    t.conn(mirror(paired_with(with_spl(), CM4, "cm4")));
    let rows: Vec<_> =
        t.st.list_rows()
            .into_iter()
            .filter(|(r, _)| *r == CM4)
            .map(|(_, r)| r)
            .collect();
    let [head] = &rows[..] else {
        panic!("a heading alone: {rows:?}")
    };
    assert_eq!((head.name.as_str(), head.collapsed), ("cm4", None));
    assert_eq!(
        head.details,
        [
            "paired · this app is not connected to it",
            "paired · not connected"
        ]
    );
    assert!(
        t.st.find_rig(CM4).is_none(),
        "the app connects to no rig on its own"
    );
    t.st.update(Msg::ShowRig(CM4), &t.keys);
    assert_eq!(t.st.active(), RIG);
}

/// Pupu paired and held, its link down: its heading stays, saying so, with no rows; its
/// rows are no drop meanwhile.
#[test]
fn a_paired_rig_whose_link_is_down_is_a_heading_alone() {
    let (mut t, p) = two_lists();
    to(
        &mut t,
        PUPU,
        ConnEvent::Failed {
            target: "pupu".into(),
            error: "gone".into(),
            down: ac2_scene::chrome::RigDown::Unreachable,
            retry_in: std::time::Duration::from_secs(2),
        },
    );
    let rows: Vec<_> =
        t.st.list_rows()
            .into_iter()
            .filter(|(r, _)| *r == PUPU)
            .map(|(_, r)| r)
            .collect();
    let [head] = &rows[..] else {
        panic!("a heading alone: {rows:?}")
    };
    assert_eq!(head.name, "pupu");
    assert_eq!(head.details, ["paired · unreachable · retrying"]);
    let before = *t.st.layout.view(p).expect("pane");
    drop_pupu(&mut t, p);
    assert_eq!(*t.st.layout.view(p).expect("pane"), before);
}
