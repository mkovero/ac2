//! Reducer tests of list rows dragged onto the panes and of undo / redo of pane edits.

use super::*;
use ac2_proto::model::PaneLayout;
use ac2_scene::meas_list::TreeKey;
use ac2_scene::pane_edit::Edge;

/// The test daemon with an SPL meter `FOH SPL` (4) and a second transfer measurement
/// `Main R` (5).
fn rig() -> State {
    let mut s = with_spl();
    s.measurements.push(meas(5, "Main R", transfer()));
    s
}

/// The four-pane grid (transfer, spectrum, IR, SPL in reading order) on [`rig`].
fn desk() -> T {
    let mut t = T::fresh();
    t.st.layout = grid();
    t.st.pane_auto = false;
    t.connect(rig());
    t
}

/// `item` dragged and dropped on pane `pane` (near `edge`), Shift held when `compare`.
fn drop_on(
    t: &mut T,
    item: TreeKey,
    pane: PaneId,
    edge: Option<Edge>,
    compare: bool,
) -> Vec<Request> {
    let mut r = t.st.update(Msg::RowDrag(DragMsg::Start(item)), &t.keys);
    r.extend(t.st.update(
        Msg::RowDrag(DragMsg::Over {
            target: Some(DropTarget { pane, edge }),
            compare,
        }),
        &t.keys,
    ));
    r.extend(t.st.update(Msg::RowDrag(DragMsg::Drop { compare }), &t.keys));
    r
}

fn layout_targets(r: &[Request]) -> Vec<MeasId> {
    r.iter()
        .filter_map(|r| match r {
            Request::Call {
                cmd: Command::MeasLayout { meas, .. },
                ..
            } => Some(*meas),
            _ => None,
        })
        .collect()
}

fn sent_layout(r: &[Request]) -> Option<PaneLayout> {
    r.iter().find_map(|r| match r {
        Request::Call {
            cmd: Command::MeasLayout { layout, .. },
            ..
        } => layout.clone(),
        _ => None,
    })
}

/// The daemon answers this app's layout calls in `r`.
fn answer(t: &mut T, r: &[Request]) {
    for r in r {
        if let Request::Call {
            cmd: Command::MeasLayout { .. },
            what,
        } = r
        {
            t.conn(ConnEvent::Reply {
                what: what.clone(),
                result: Ok(()),
            });
        }
    }
}

fn pane_n(t: &T, n: usize) -> PaneId {
    t.st.layout.panes()[n - 1]
}

fn shows(t: &T, p: PaneId) -> Option<MeasId> {
    t.st.pane_meas(p).map(|m| m.id)
}

/// A row dropped on a pane that is not focused: that pane alone shows it, turned into the
/// kind that draws it, and takes the focus.
#[test]
fn a_drop_changes_only_the_pane_under_it() {
    let mut t = desk();
    let views = t.st.layout.views.clone();
    let spl = pane_n(&t, 4);
    assert_ne!(t.st.layout.focus, spl);
    drop_on(&mut t, TreeKey::Meas(MeasId(2)), spl, None, false);
    assert_eq!(t.st.layout.kind(spl), PaneKind::Spectrum);
    assert_eq!(shows(&t, spl), Some(MeasId(2)));
    assert_eq!(t.st.layout.focus, spl);
    for (p, v) in &views {
        if *p != spl {
            assert_eq!(t.st.layout.view(*p), Some(v), "pane {p:?} unchanged");
        }
    }
    assert_eq!(t.st.layout.panes().len(), 4);
    // A stored trace or a live curve goes the same way: the IR pane shows Main R's group.
    // A drop selects nothing: the selection stays on its row.
    let selected = (t.st.selected, t.st.selected_live);
    let ir = pane_n(&t, 3);
    drop_on(&mut t, TreeKey::Live(MeasId(5)), ir, None, false);
    assert_eq!(shows(&t, ir), Some(MeasId(5)));
    assert_eq!((t.st.selected, t.st.selected_live), selected);
    assert_eq!(
        t.st.layout.view(spl).map(|v| v.kind),
        Some(PaneKind::Spectrum)
    );
}

/// Dropped near a side, the pane splits there and the new half shows the row; on a pane
/// too small to split it is a plain drop.
#[test]
fn an_edge_drop_splits_on_that_side() {
    let mut t = desk();
    let spl = pane_n(&t, 4);
    assert_eq!(
        t.st.drop_spot(
            DropTarget {
                pane: spl,
                edge: Some(Edge::Top)
            },
            false
        ),
        Some(ac2_scene::pane_edit::DropSpot::NewPane(Edge::Top))
    );
    drop_on(
        &mut t,
        TreeKey::Meas(MeasId(4)),
        spl,
        Some(Edge::Top),
        false,
    );
    let new = t.st.layout.focus;
    assert_ne!(new, spl);
    assert_eq!(
        t.st.layout.root.above(spl),
        Some(new),
        "the new pane is on top"
    );
    assert_eq!(t.st.layout.kind(new), PaneKind::Spl);
    assert_eq!(shows(&t, new), Some(MeasId(4)));
    assert_eq!(t.st.layout.panes().len(), 5);
    // Left of the transfer pane: the new pane reads first.
    let tf = pane_n(&t, 1);
    drop_on(
        &mut t,
        TreeKey::Meas(MeasId(5)),
        tf,
        Some(Edge::Left),
        false,
    );
    let new = t.st.layout.focus;
    assert_eq!(pane_n(&t, 1), new);
    assert_eq!(pane_n(&t, 2), tf);
    assert_eq!(shows(&t, new), Some(MeasId(5)));
    assert_eq!(
        shows(&t, tf),
        Some(MeasId(1)),
        "the old half keeps its measurement"
    );
    // Below the size N refuses to split: the pane itself shows it. Halved at its bottom
    // until it is that small.
    let mut small = t.st.layout.focus;
    for _ in 0..8 {
        let target = DropTarget {
            pane: small,
            edge: Some(Edge::Bottom),
        };
        if !matches!(
            t.st.drop_spot(target, false),
            Some(ac2_scene::pane_edit::DropSpot::NewPane(_))
        ) {
            break;
        }
        drop_on(
            &mut t,
            TreeKey::Meas(MeasId(1)),
            small,
            Some(Edge::Bottom),
            false,
        );
        small = t.st.layout.focus;
    }
    let n = t.st.layout.panes().len();
    let target = DropTarget {
        pane: small,
        edge: Some(Edge::Bottom),
    };
    let spot = t.st.drop_spot(target, false);
    assert_eq!(
        spot,
        Some(ac2_scene::pane_edit::DropSpot::Pane(
            t.st.pane_number(small)
        )),
        "{spot:?}"
    );
    drop_on(
        &mut t,
        TreeKey::Meas(MeasId(2)),
        small,
        Some(Edge::Bottom),
        false,
    );
    assert_eq!(t.st.layout.panes().len(), n);
    assert_eq!(shows(&t, small), Some(MeasId(2)));
}

/// The label that follows the pointer names the row and where it lands.
#[test]
fn the_drag_label_says_where_the_row_goes() {
    let mut t = desk();
    let spl = pane_n(&t, 4);
    t.st.update(
        Msg::RowDrag(DragMsg::Start(TreeKey::Meas(MeasId(4)))),
        &t.keys,
    );
    assert_eq!(t.st.drag_label(), None, "not over a pane yet");
    let over = |t: &mut T, pane: PaneId, edge, compare| {
        t.st.update(
            Msg::RowDrag(DragMsg::Over {
                target: Some(DropTarget { pane, edge }),
                compare,
            }),
            &t.keys,
        );
        t.st.drag_label().unwrap_or_default()
    };
    let two = pane_n(&t, 2);
    assert_eq!(over(&mut t, two, None, false), "FOH SPL \u{2192} pane 2");
    assert_eq!(
        over(&mut t, spl, Some(Edge::Bottom), false),
        "FOH SPL \u{2192} new pane below"
    );
    assert_eq!(
        over(&mut t, two, None, true),
        "FOH SPL \u{2192} compare in pane 2"
    );
    assert_eq!(t.st.drop_highlight(), Some((two, None)));
}

/// Esc cancels a drag, and a drop off the panes does nothing.
#[test]
fn esc_or_a_drop_off_the_panes_does_nothing() {
    let mut t = desk();
    let before = t.st.pane_snap();
    let spl = pane_n(&t, 4);
    t.st.update(
        Msg::RowDrag(DragMsg::Start(TreeKey::Meas(MeasId(2)))),
        &t.keys,
    );
    t.st.update(
        Msg::RowDrag(DragMsg::Over {
            target: Some(DropTarget {
                pane: spl,
                edge: None,
            }),
            compare: false,
        }),
        &t.keys,
    );
    let r = t.key("Escape");
    assert!(r.is_empty(), "Esc only cancels the drag: {r:?}");
    assert!(t.st.drag_label().is_none());
    t.st.update(Msg::RowDrag(DragMsg::Drop { compare: false }), &t.keys);
    assert_eq!(t.st.pane_snap(), before);
    assert!(!t.st.row_drag_held());
    t.st.update(
        Msg::RowDrag(DragMsg::Start(TreeKey::Meas(MeasId(2)))),
        &t.keys,
    );
    t.st.update(
        Msg::RowDrag(DragMsg::Over {
            target: None,
            compare: false,
        }),
        &t.keys,
    );
    t.st.update(Msg::RowDrag(DragMsg::Drop { compare: false }), &t.keys);
    assert_eq!(t.st.pane_snap(), before);
    assert_eq!(t.st.history.undo_len(), 0);
}

/// A drop is a guest pick: Main L's row chosen, FOH SPL dropped on pane 2 turns it into an
/// SPL pane, an edit of Main L's panes stored on Main L and never on FOH SPL, as is the G
/// after it.
#[test]
fn a_drop_is_never_the_dropped_measurements_layout() {
    let mut t = desk();
    let r = t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    answer(&mut t, &r);
    let two = pane_n(&t, 2);
    let r = drop_on(&mut t, TreeKey::Meas(MeasId(4)), two, None, false);
    assert!(
        !layout_targets(&r).contains(&MeasId(4)),
        "the drop is no layout of FOH SPL's: {r:?}"
    );
    assert_eq!(layout_targets(&r), [MeasId(1)], "Main L's panes: {r:?}");
    answer(&mut t, &r);
    assert_eq!(
        t.st.selected,
        Some(MeasId(1)),
        "the selection stays on the row"
    );
    assert_eq!(t.st.layout.focus_kind(), PaneKind::Spl);
    let r = t.key("G");
    assert_eq!(layout_targets(&r), [MeasId(1)], "{r:?}");
    answer(&mut t, &r);
    // An edge drop is no edit of FOH SPL's either: the split is an edit of Main L's
    // panes.
    let r = drop_on(
        &mut t,
        TreeKey::Meas(MeasId(4)),
        two,
        Some(Edge::Right),
        false,
    );
    assert_eq!(layout_targets(&r), [MeasId(1)], "{r:?}");
    answer(&mut t, &r);
    let r = t.key("T");
    assert_eq!(layout_targets(&r), [MeasId(1)], "{r:?}");
}

/// Shift held on the drop: the row is compared into the measurement the pane shows, which
/// keeps what it shows.
#[test]
fn a_shift_drop_compares() {
    let mut t = desk();
    let tf = pane_n(&t, 1);
    let view = t.st.layout.view(tf).copied();
    t.st.update(Msg::FocusPane(pane_n(&t, 2)), &t.keys);
    let r = drop_on(&mut t, TreeKey::Meas(MeasId(5)), tf, None, true);
    t.answer(&r);
    assert_eq!(
        t.st.meas(MeasId(1)).map(|m| m.compared.clone()),
        Some(vec![Compared::Meas { meas: MeasId(5) }])
    );
    assert_eq!(
        t.st.layout.view(tf).copied(),
        view,
        "the pane keeps what it shows"
    );
    assert_eq!(shows(&t, tf), Some(MeasId(1)));
    assert!(
        t.last_toast().starts_with("Main R compared into "),
        "{}",
        t.last_toast()
    );
}

/// A Shift-drop the pane cannot draw is refused, saying why, and nothing else changes.
#[test]
fn a_wrong_kind_shift_drop_is_refused() {
    let mut t = desk();
    let before = t.st.pane_snap();
    let tf = pane_n(&t, 1);
    drop_on(&mut t, TreeKey::Meas(MeasId(4)), tf, None, true);
    assert_eq!(
        t.last_toast(),
        "FOH SPL not compared: it has no transfer curve to draw over the pane"
    );
    assert_eq!(t.st.pane_snap(), before);
    let spec = pane_n(&t, 2);
    drop_on(&mut t, TreeKey::Meas(MeasId(5)), spec, None, true);
    assert_eq!(
        t.last_toast(),
        "Main R not compared: it has no spectrum or RTA curve to draw over the pane"
    );
    assert_eq!(t.st.pane_snap(), before);
    let spl = pane_n(&t, 4);
    drop_on(&mut t, TreeKey::Meas(MeasId(4)), spl, None, true);
    assert_eq!(
        t.last_toast(),
        "FOH SPL not compared: pane 4 shows SPL; meters are compared side by side — drop it without Shift on a pane of its own"
    );
    assert_eq!(t.st.pane_snap(), before);
    assert_eq!(t.st.history.undo_len(), 0);
}

/// Each covered edit undone puts the panes back as they were, and redone as it left them.
#[test]
fn undo_and_redo_round_trip_every_pane_edit() {
    type Edit = Box<dyn Fn(&mut T)>;
    let key = |k: &'static str| -> Edit { Box::new(move |t: &mut T| drop(t.key(k))) };
    // Tab on to the next measurement row: a child row on the way only selects.
    let tab_to_meas = |k: &'static str| -> Edit {
        Box::new(move |t: &mut T| {
            let before = t.st.pane_snap();
            for _ in 0..t.st.tree_rows().len() {
                t.key(k);
                if t.st.pane_snap() != before {
                    break;
                }
            }
        })
    };
    let edits: Vec<(&str, Edit)> = vec![
        (
            "drop",
            Box::new(|t: &mut T| {
                let p = pane_n(t, 4);
                drop_on(t, TreeKey::Meas(MeasId(2)), p, None, false);
            }),
        ),
        (
            "edge drop",
            Box::new(|t: &mut T| {
                let p = pane_n(t, 4);
                drop_on(t, TreeKey::Meas(MeasId(5)), p, Some(Edge::Top), false);
            }),
        ),
        ("N", key("N")),
        ("Shift+N", key("Shift+N")),
        ("Q", key("Q")),
        (
            "chip pick",
            Box::new(|t: &mut T| {
                let p = pane_n(t, 2);
                t.st.update(Msg::PanePick(p, PaneMenuRow::Meas(MeasId(4))), &t.keys);
            }),
        ),
        ("Tab", tab_to_meas("Tab")),
        ("Shift+Tab", tab_to_meas("Shift+Tab")),
        ("G", key("G")),
        ("Shift+G", key("Shift+G")),
        ("T", key("T")),
        ("Alt+ArrowRight", key("Alt+ArrowRight")),
        (
            "split drag",
            Box::new(|t: &mut T| {
                for ratio in [0.5, 0.45, 0.4] {
                    t.st.update(
                        Msg::DragSplit {
                            path: vec![],
                            ratio,
                        },
                        &t.keys,
                    );
                }
                t.st.update(Msg::DragSplitEnd, &t.keys);
            }),
        ),
        ("W", key("W")),
        ("F", key("F")),
    ];
    for (name, edit) in edits {
        let mut t = desk();
        let before = t.st.pane_snap();
        edit(&mut t);
        let after = t.st.pane_snap();
        assert_ne!(before, after, "{name} changed nothing");
        t.key("Ctrl+Z");
        assert!(
            t.last_toast().starts_with("Undid: "),
            "{name}: {}",
            t.last_toast()
        );
        assert_eq!(t.st.pane_snap(), before, "{name} undone");
        t.key("Ctrl+Shift+Z");
        assert!(
            t.last_toast().starts_with("Redid: "),
            "{name}: {}",
            t.last_toast()
        );
        assert_eq!(t.st.pane_snap(), after, "{name} redone");
        assert_eq!(t.st.history.undo_len(), 1, "{name}");
    }
}

/// The toasts name what was undone and redone.
#[test]
fn undo_and_redo_say_what_they_did() {
    let mut t = desk();
    t.key("Ctrl+Z");
    assert_eq!(t.last_toast(), "Nothing to undo");
    let two = pane_n(&t, 2);
    drop_on(&mut t, TreeKey::Meas(MeasId(4)), two, None, false);
    t.key("Ctrl+Z");
    assert_eq!(t.last_toast(), "Undid: FOH SPL \u{2192} pane 2");
    t.key("Ctrl+Shift+Z");
    assert_eq!(t.last_toast(), "Redid: FOH SPL \u{2192} pane 2");
    t.st.update(Msg::FocusPane(pane_n(&t, 1)), &t.keys);
    t.key("N");
    t.key("Ctrl+Z");
    t.key("Ctrl+Shift+Z");
    assert_eq!(t.last_toast(), "Redid: split pane 1");
    t.key("Ctrl+Shift+Z");
    assert_eq!(t.last_toast(), "Nothing to redo");
}

/// Undoing an edit stored as the chosen measurement's own layout stores the panes put back
/// on it, and only on it: never on a guest picked into its panes.
#[test]
fn undo_restores_the_own_layout_of_the_chosen_measurement_only() {
    let mut t = desk();
    let r = t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    answer(&mut t, &r);
    let before = t.st.pane_layout();
    let r = t.key("N");
    assert_eq!(layout_targets(&r), [MeasId(1)]);
    answer(&mut t, &r);
    let r = t.key("Ctrl+Z");
    assert_eq!(layout_targets(&r), [MeasId(1)], "{r:?}");
    assert_eq!(sent_layout(&r), Some(before.clone()));
    assert_eq!(t.st.pane_layout(), before);
    answer(&mut t, &r);
    let r = t.key("Ctrl+Shift+Z");
    assert_eq!(layout_targets(&r), [MeasId(1)], "{r:?}");
    answer(&mut t, &r);
    // FOH SPL picked into pane 4 is a guest: the G after it, undone, goes to Main L.
    let four = pane_n(&t, 5);
    let r =
        t.st.update(Msg::PanePick(four, PaneMenuRow::Meas(MeasId(4))), &t.keys);
    assert_eq!(t.st.selected, Some(MeasId(1)));
    assert_eq!(layout_targets(&r), [MeasId(1)], "the pick: {r:?}");
    answer(&mut t, &r);
    let r = t.key("G");
    assert_eq!(layout_targets(&r), [MeasId(1)], "{r:?}");
    answer(&mut t, &r);
    let r = t.key("Ctrl+Z");
    assert_eq!(layout_targets(&r), [MeasId(1)], "{r:?}");
    answer(&mut t, &r);
    // The pick is part of Main L's layout: undoing it goes to Main L too.
    let r = t.key("Ctrl+Z");
    assert!(
        t.last_toast().starts_with("Undid: FOH SPL"),
        "{}",
        t.last_toast()
    );
    assert_eq!(layout_targets(&r), [MeasId(1)], "{r:?}");
}

/// With no measurement's row chosen the panes are nobody's: undo sends nothing.
#[test]
fn undo_of_panes_nobody_owns_sends_nothing() {
    let mut t = desk();
    let r = t.key("N");
    assert!(layout_targets(&r).is_empty(), "{r:?}");
    let r = t.key("Ctrl+Z");
    assert!(layout_targets(&r).is_empty(), "{r:?}");
}

/// A new edit after an undo drops what could be redone.
#[test]
fn a_new_edit_drops_the_redo_tail() {
    let mut t = desk();
    t.key("N");
    t.key("N");
    t.key("Ctrl+Z");
    assert_eq!(t.st.history.redo_len(), 1);
    let after_g = {
        t.key("G");
        t.st.pane_snap()
    };
    assert_eq!(t.st.history.redo_len(), 0);
    t.key("Ctrl+Shift+Z");
    assert_eq!(t.last_toast(), "Nothing to redo");
    assert_eq!(t.st.pane_snap(), after_g);
}

/// The history keeps the last [`HISTORY_LEN`] steps.
#[test]
fn the_history_is_capped() {
    let mut t = desk();
    for _ in 0..HISTORY_LEN + 10 {
        t.key("G");
    }
    assert_eq!(t.st.history.undo_len(), HISTORY_LEN);
    for _ in 0..HISTORY_LEN {
        t.key("Ctrl+Z");
        assert!(t.last_toast().starts_with("Undid: "));
    }
    t.key("Ctrl+Z");
    assert_eq!(t.last_toast(), "Nothing to undo");
}

/// A measurement deleted since an edit: its pane comes back empty of it, on its kind.
#[test]
fn a_deleted_measurement_comes_back_as_an_empty_pane() {
    let mut t = desk();
    let two = pane_n(&t, 2);
    drop_on(&mut t, TreeKey::Meas(MeasId(5)), two, None, false);
    t.key("Ctrl+Z");
    t.conn(mirror(with_spl()));
    assert!(t.st.meas(MeasId(5)).is_none());
    t.key("Ctrl+Shift+Z");
    assert_eq!(t.st.layout.kind(two), PaneKind::Transfer);
    assert_eq!(t.st.layout.view(two).and_then(|v| v.meas), None);
    assert_ne!(shows(&t, two), Some(MeasId(5)));
}
