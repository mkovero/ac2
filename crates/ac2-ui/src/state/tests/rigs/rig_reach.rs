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
        .map(|(rig, r)| {
            let line = r.details.last().map_or("", String::as_str);
            (rig, format!("{} / {line}", r.name))
        })
        .collect()
}

fn pick_rig(t: &mut T, rig: RigId) {
    t.st.update(Msg::RigPick(RigRow::Rig(rig)), &t.keys);
    assert_eq!(t.st.active(), rig);
}

/// A rig the app holds that the rig shown is not paired with has no heading and no rows;
/// the drop-down still lists it. Each heading says the rig's rate beside its name, then
/// its clock's standing among the rigs listed (these rigs' clocks are not measured).
#[test]
fn a_rig_held_and_not_paired_is_not_listed() {
    let (t, _) = three_rigs();
    assert_eq!(
        heads(&t),
        [
            (RIG, "test rig / 48 kHz UNKNOWN".to_owned()),
            (PUPU, "pupu / 48 kHz UNKNOWN".to_owned())
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
    assert_eq!(heads(&t), [(KETTU, "kettu / 48 kHz".to_owned())]);
    pick_rig(&mut t, PUPU);
    let listed: Vec<RigId> = heads(&t).into_iter().map(|(r, _)| r).collect();
    assert_eq!(listed, [PUPU, RIG]);
    // Plain *paired* is the tooltip's: the rig is listed because it is paired.
    let head =
        t.st.list_rows()
            .into_iter()
            .find(|(_, r)| r.key == TreeKey::RigName(RIG))
            .map(|(_, r)| r)
            .expect("test rig's heading");
    assert_eq!(head.details, ["48 kHz UNKNOWN"]);
    assert!(
        head.describe.starts_with("test rig: another rig, paired;"),
        "{}",
        head.describe
    );
}

/// A rig with nothing measured and no audio I/O says so where its rate would be, in the
/// top bar's words.
#[test]
fn a_rig_without_a_stream_says_so_beside_its_name() {
    let (mut t, _) = two_lists();
    let mut s = pupu(&t).daemon().cloned().expect("pupu's state");
    s.stream.open = None;
    to(&mut t, PUPU, mirror(s.clone()));
    assert_eq!(heads(&t)[1].1, "pupu / no audio I/O");
    s.measurements.clear();
    to(&mut t, PUPU, mirror(s));
    assert_eq!(heads(&t)[1].1, "pupu / no stream");
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

/// Each rig has its own screen, so a switch leaves no pane of the rig left on screen: with
/// kettu shown, which is paired with neither, the screen is kettu's alone, and the first
/// rig's pane is back, its own again, when that rig is.
#[test]
fn a_switch_to_a_rig_not_paired_leaves_no_pane_out_of_reach() {
    let (mut t, p) = three_rigs();
    t.st.layout.view_mut(p).expect("pane").kind = PaneKind::Spectrum;
    t.st.layout.view_mut(p).expect("pane").meas = Some(MeasId(2));
    pick_rig(&mut t, KETTU);
    assert!(t.st.layout.view(p).is_none(), "not on kettu's screen");
    let panes = t.st.visible_panes();
    assert!(
        panes
            .iter()
            .filter_map(|p| t.st.layout.view(*p))
            .all(|v| t.st.rig_in_reach(t.st.pane_rig(v))),
        "{panes:?}"
    );
    pick_rig(&mut t, RIG);
    let v = *t.st.layout.view(p).expect("pane");
    assert_eq!((v.rig, v.meas), (PaneRig::Active, Some(MeasId(2))));
    assert_eq!(t.st.pane_meas(p).map(|m| m.id), Some(MeasId(2)));
}

/// A pane showing pupu's curve (its row dragged there), then the pairing removed: the pane
/// keeps its rig and measurement, out of reach (the view says the two rigs are not paired)
/// and deaf to its keys; paired again it is in reach as it was. The same holds on a screen
/// kept for a rig not shown: it comes back as it was left, and reach is judged then.
#[test]
fn a_dragged_in_pane_whose_pairing_went_keeps_its_content_and_comes_back() {
    let (mut t, p) = three_rigs();
    drop_pupu(&mut t, p);
    let v = *t.st.layout.view(p).expect("pane");
    assert_eq!((v.rig, v.meas), (PaneRig::Rig(PUPU), Some(MeasId(2))));
    assert!(t.st.rig_in_reach(PUPU));
    to(&mut t, RIG, mirror(with_spl()));
    assert!(!t.st.rig_in_reach(PUPU), "no longer paired");
    assert_eq!(*t.st.layout.view(p).expect("pane"), v, "kept as it was");
    t.st.layout.set_focus(p);
    let r = t.st.update(Msg::Command(CommandId::Peaks), &t.keys);
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(*t.st.layout.view(p).expect("pane"), v, "kept as it was");
    // Kettu's screen and back: the pane is on the screen it was left on, still out of reach.
    pick_rig(&mut t, KETTU);
    assert!(t.st.layout.view(p).is_none());
    pick_rig(&mut t, RIG);
    assert_eq!(*t.st.layout.view(p).expect("pane"), v);
    assert!(!t.st.rig_in_reach(t.st.pane_rig(&v)));
    to(&mut t, RIG, mirror(paired(with_spl())));
    assert!(t.st.rig_in_reach(t.st.pane_rig(&v)));
    assert_eq!(*t.st.layout.view(p).expect("pane"), v);
}

/// A pane the layout prefs pin to a rig out of reach (a screen stored before the pairing
/// was removed) is kept as stored: its rig and measurement by name are written back
/// unchanged.
#[test]
fn a_stored_pane_of_a_rig_out_of_reach_is_kept() {
    let (mut t, p) = three_rigs();
    {
        let v = t.st.layout.view_mut(p).expect("pane");
        v.kind = PaneKind::Spectrum;
        v.rig = PaneRig::Rig(KETTU);
        v.meas = Some(MeasId(2));
    }
    let v = *t.st.layout.view(p).expect("pane");
    assert!(!t.st.rig_in_reach(t.st.pane_rig(&v)));
    assert_eq!(
        t.st.pane_pref_rig(&v).map(|(r, _)| r),
        Some(KETTU),
        "the prefs name its rig"
    );
    assert_eq!(t.st.pane_pref_meas(p, &v).as_deref(), Some("Kettu sub"));
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
