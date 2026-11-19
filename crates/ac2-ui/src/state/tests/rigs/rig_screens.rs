//! Each rig keeps its own screen: a switch swaps to that rig's, and back restores the one
//! left.

use super::*;

/// A screen: the split, what each pane shows, the focus and whether one pane is alone
/// (the pane id counter runs on across screens).
fn panes_of(l: &Layout) -> (PaneNode, BTreeMap<PaneId, View>, PaneId, bool) {
    (l.root.clone(), l.views.clone(), l.focus, l.maximized)
}

/// Each rig keeps its own screen. A switch puts the other rig's on screen — shown for the
/// first time, the panes of its first measurement, as that row brings them, stored on it
/// — and none of the panes left; back, the screen left is there exactly: the split, the
/// focus, what each pane shows, whose own panes they are and its undo steps.
#[test]
fn a_switch_swaps_to_that_rigs_screen_and_back() {
    let mut t = two_mirrored();
    t.st.apply(Msg::SelectMeas(MeasId(2)), &t.keys);
    t.key("N");
    t.key("G");
    let left = t.st.layout.clone();
    let owner = t.st.rig.choice.own_panes;
    assert_eq!(owner, Some(MeasId(2)));
    let steps = t.st.history.undo_len();
    assert!(steps > 0);
    let r = pick(&mut t, RigRow::Rig(PUPU));
    assert_eq!(t.st.active(), PUPU);
    let panes = t.st.visible_panes();
    assert!(
        panes.iter().all(|p| !left.views.contains_key(p)),
        "{panes:?}"
    );
    assert!(panes.iter().all(|p| t.st.pane_ours(*p)));
    let first = t.st.tree_meas_order()[0];
    assert_eq!(t.st.rig.selected, Some(first));
    assert!(
        panes
            .iter()
            .all(|p| t.st.pane_meas(*p).map(|m| m.id) == Some(first)),
        "pupu's first measurement on every pane"
    );
    let sent: Vec<(RigId, MeasId)> = r
        .iter()
        .filter_map(|(rig, q)| match q {
            Request::Call {
                cmd: Command::MeasLayout { meas, .. },
                ..
            } => Some((*rig, *meas)),
            _ => None,
        })
        .collect();
    assert_eq!(sent, [(PUPU, first)], "its first panes, stored on pupu");
    let pupus = t.st.layout.clone();
    // The switch is no step of the history, and the steps made on a screen are its own:
    // undo here takes back nothing of the screen left.
    assert_eq!(t.st.history.undo_len(), 0);
    t.key("Ctrl+Z");
    assert_eq!(panes_of(&t.st.layout), panes_of(&pupus));
    pick(&mut t, RigRow::Rig(RIG));
    assert_eq!(t.st.history.undo_len(), steps);
    assert_eq!(
        panes_of(&t.st.layout),
        panes_of(&left),
        "the screen left, exactly"
    );
    assert_eq!(t.st.rig.choice.own_panes, owner);
    let r = pick(&mut t, RigRow::Rig(PUPU));
    assert_eq!(
        panes_of(&t.st.layout),
        panes_of(&pupus),
        "pupu's screen as it was left"
    );
    assert!(r.is_empty(), "nothing stored again: {r:?}");
}

/// A rig with no measurement shown for the first time: one empty pane.
#[test]
fn a_rig_without_measurements_starts_on_an_empty_pane() {
    let mut t = two_rigs();
    to(&mut t, PUPU, mirror(recorded::empty_state()));
    pick(&mut t, RigRow::Rig(PUPU));
    assert_eq!(t.st.active(), PUPU);
    let [p] = t.st.visible_panes()[..] else {
        panic!("one pane: {:?}", t.st.visible_panes())
    };
    assert!(t.st.layout.view(p).is_some_and(|v| !v.has_content()));
}

/// A rig shown before it listed anything (just connected) gets its first measurement's
/// panes when it lists one.
#[test]
fn a_rig_shown_before_its_state_gets_its_first_screen_then() {
    let mut t = two_rigs();
    pick(&mut t, RigRow::Rig(PUPU));
    assert_eq!(t.st.visible_panes().len(), 1);
    to(&mut t, PUPU, mirror(with_spl()));
    let first = t.st.tree_meas_order()[0];
    let panes = t.st.visible_panes();
    assert!(
        panes
            .iter()
            .all(|p| t.st.pane_meas(*p).map(|m| m.id) == Some(first)),
        "{panes:?}"
    );
    assert_eq!(t.st.rig.choice.own_panes, Some(first));
}

/// A pane showing a curve of another rig (its row dragged there) belongs to the screen it
/// is on: it is not on the other rig's screen, and comes back with its own, still that
/// rig's.
#[test]
fn a_dragged_in_pane_travels_with_its_screen() {
    let (mut t, p) = two_lists();
    drop_pupu(&mut t, p);
    let pinned = *t.st.layout.view(p).expect("pane");
    assert_eq!(pinned.rig, PaneRig::Rig(PUPU));
    let left = t.st.layout.clone();
    pick(&mut t, RigRow::Rig(PUPU));
    assert!(t.st.layout.view(p).is_none(), "not on pupu's screen");
    pick(&mut t, RigRow::Rig(RIG));
    assert_eq!(panes_of(&t.st.layout), panes_of(&left));
    assert_eq!(*t.st.layout.view(p).expect("pane"), pinned);
    assert_eq!(
        t.st.pane_title(p),
        format!("pupu · {}", PaneKind::Spectrum.title())
    );
}
