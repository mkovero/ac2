//! Reducer tests of the pane tree from an empty daemon: splitting, closing, what each pane
//! shows, the focus keys and the layout remembered in `ui.toml`.

use super::*;
use ac2_scene::primitives::Viewport;
use ac2_scene::theme::Theme;

const SIZE: Viewport = Viewport {
    width: 900.0,
    height: 500.0,
};

fn now() -> crate::scenes::Now {
    crate::scenes::Now {
        instant: Instant::now(),
        wall: WallNs(0),
    }
}

/// The app as first started on a daemon with nothing measured yet.
fn empty() -> T {
    let mut t = T::fresh();
    t.connect(empty_state());
    t
}

/// The legend of pane `id`'s transfer scene, sorted.
fn legend(t: &T, id: PaneId) -> Vec<String> {
    let mut v: Vec<String> = crate::scenes::transfer(&t.st, id, &Theme::dark(), SIZE, now())
        .legend
        .iter()
        .map(|e| e.name.clone())
        .collect();
    v.sort();
    v
}

fn shows(t: &T, id: PaneId) -> Option<u32> {
    t.st.pane_meas(id).map(|m| m.id.0)
}

fn kinds(t: &T) -> Vec<PaneKind> {
    t.st.layout
        .root
        .reading_order()
        .into_iter()
        .map(|id| t.st.layout.kind(id))
        .collect()
}

/// No layout saved: one pane over the whole area, and nothing written for it until the
/// operator changes it.
#[test]
fn a_fresh_start_is_one_pane() {
    let t = empty();
    assert_eq!(t.st.layout.root, PaneNode::Leaf(t.st.layout.focus));
    assert_eq!(t.st.visible_panes(), [t.st.layout.focus]);
    let area = PaneRect::new(0.0, 0.0, 1000.0, 600.0);
    assert_eq!(t.st.pane_rects(area, 4.0), [(t.st.layout.focus, area)]);
    assert_eq!(t.st.layout_prefs().panes, None);
}

/// Ctrl+N splits the focused pane along its longer side and focuses the new half: a wide
/// window splits left | right, the tall right half then top / bottom.
#[test]
fn ctrl_n_twice_gives_three_panes() {
    let mut t = empty();
    let first = t.st.layout.focus;
    t.key("Ctrl+N");
    t.key("Ctrl+N");
    let order = t.st.layout.root.reading_order();
    assert_eq!(order.len(), 3);
    assert_eq!(order[0], first);
    assert_eq!(t.st.layout.focus, order[2]);
    let area = PaneRect::new(0.0, 0.0, 1000.0, 600.0);
    let rects: BTreeMap<_, _> = t.st.pane_rects(area, 0.0).into_iter().collect();
    assert_eq!(rects[&order[0]], PaneRect::new(0.0, 0.0, 500.0, 600.0));
    assert_eq!(rects[&order[1]], PaneRect::new(500.0, 0.0, 500.0, 300.0));
    assert_eq!(rects[&order[2]], PaneRect::new(500.0, 300.0, 500.0, 300.0));
    // Each new half shows what the split one did.
    assert_eq!(kinds(&t), [PaneKind::Transfer; 3]);
    // Now the operator's: remembered.
    assert!(t.st.layout_prefs().panes.is_some());
}

/// Ctrl+D closes the focused pane, its sibling taking the space and the focus; the last
/// pane stays, saying how to change what it shows.
#[test]
fn ctrl_d_closes_and_keeps_the_last_pane() {
    let mut t = empty();
    let first = t.st.layout.focus;
    t.key("Ctrl+D");
    assert_eq!(t.st.layout.root, PaneNode::Leaf(first));
    assert!(
        t.last_toast().contains("the last pane stays"),
        "{}",
        t.last_toast()
    );
    t.key("Ctrl+N");
    assert_ne!(t.st.layout.focus, first);
    t.key("Ctrl+D");
    assert_eq!(t.st.layout.root, PaneNode::Leaf(first));
    assert_eq!(t.st.layout.focus, first);
}

/// Ctrl+Tab changes what the focused pane shows and nothing else.
#[test]
fn ctrl_tab_changes_only_the_focused_pane() {
    let mut t = empty();
    t.conn(mirror(four()));
    t.key("Ctrl+N");
    t.key("Ctrl+N");
    t.key("Ctrl+Tab");
    assert_eq!(
        kinds(&t),
        [PaneKind::Transfer, PaneKind::Transfer, PaneKind::Spectrum]
    );
    t.key("Alt+1");
    t.key("Ctrl+Shift+Tab");
    // Back from the transfer kind: the sweep pane, where a sweep is set up.
    assert_eq!(
        kinds(&t),
        [PaneKind::Distortion, PaneKind::Transfer, PaneKind::Spectrum]
    );
    // SPL has no meter here: skipped.
    t.key("Ctrl+Shift+Tab");
    assert_eq!(kinds(&t)[0], PaneKind::Ir);
}

/// Two transfer panes keep their own measurements: N steps the focused one only, each
/// draws its own curve, and the IR pane follows the transfer pane focused last.
#[test]
fn two_transfer_panes_keep_their_measurements() {
    let mut t = empty();
    t.conn(mirror(four()));
    super::tf_group::tf_frames(&mut t, &[1, 3]);
    let a = t.st.layout.focus;
    t.key("Ctrl+N");
    let b = t.st.layout.focus;
    assert_eq!((shows(&t, a), shows(&t, b)), (Some(1), Some(1)));
    t.key("N");
    assert_eq!((shows(&t, a), shows(&t, b)), (Some(1), Some(3)));
    t.key("N");
    t.key("N");
    assert_eq!((shows(&t, a), shows(&t, b)), (Some(1), Some(3)));
    t.key("Alt+1");
    assert_eq!((shows(&t, a), shows(&t, b)), (Some(1), Some(3)));
    assert_eq!(t.st.selected, Some(MeasId(1)));
    assert_eq!(legend(&t, a), ["Main L"]);
    assert_eq!(legend(&t, b), ["Delay tower"]);

    // An IR pane under the second: it shows the IR of the transfer pane focused last.
    t.key("Alt+2");
    t.key("Ctrl+N");
    t.show(PaneKind::Ir);
    let ir = t.st.layout.focus;
    assert_eq!(shows(&t, ir), Some(3));
    t.st.update(Msg::FocusPane(a), &t.keys);
    assert_eq!(shows(&t, ir), Some(1));
    t.st.update(Msg::FocusPane(b), &t.keys);
    assert_eq!(shows(&t, ir), Some(3));
}

/// Alt+number focuses the panes in reading order; past the last one it says how many there
/// are.
#[test]
fn alt_numbers_focus_panes_in_reading_order() {
    let mut t = empty();
    t.key("Ctrl+N");
    t.key("Ctrl+N");
    let order = t.st.layout.root.reading_order();
    t.key("Alt+2");
    assert_eq!(t.st.layout.focus, order[1]);
    t.key("Alt+1");
    assert_eq!(t.st.layout.focus, order[0]);
    t.key("Alt+4");
    assert_eq!(t.st.layout.focus, order[0]);
    assert!(
        t.last_toast().contains("no pane 4: 3 on screen"),
        "{}",
        t.last_toast()
    );
}

/// The tree, each pane's kind and views and its measurement by name go through `ui.toml`:
/// the next start lays the panes out the same and, once the daemon's state is known, each
/// shows its measurement again.
#[test]
fn the_layout_comes_back_from_ui_toml() {
    let mut t = empty();
    t.conn(mirror(four()));
    t.key("Ctrl+N");
    t.key("N");
    t.key("Ctrl+N");
    t.key("Ctrl+Tab");
    t.key("N");
    t.key("G");
    let before: Vec<_> =
        t.st.layout
            .root
            .reading_order()
            .into_iter()
            .map(|id| {
                let v = t.st.layout.view(id).expect("view");
                (id, v.kind, v.modes, shows(&t, id))
            })
            .collect();
    assert_eq!(
        before.iter().map(|b| (b.1, b.3)).collect::<Vec<_>>(),
        [
            (PaneKind::Transfer, Some(1)),
            (PaneKind::Transfer, Some(3)),
            (PaneKind::Spectrum, Some(4)),
        ]
    );
    let text = t.st.prefs.to_toml();
    assert!(text.contains("measurement = \"Delay tower\""), "{text}");

    let mut u = T::fresh();
    u.st.set_prefs(crate::prefs::UiPrefs::from_toml(&text).expect("parse"));
    assert_eq!(u.st.layout.root, t.st.layout.root);
    assert_eq!(u.st.layout.focus, t.st.layout.focus);
    u.connect(four());
    let after: Vec<_> =
        u.st.layout
            .root
            .reading_order()
            .into_iter()
            .map(|id| {
                let v = u.st.layout.view(id).expect("view");
                (id, v.kind, v.modes, shows(&u, id))
            })
            .collect();
    assert_eq!(after, before);
}

/// Equal ratios give equal plots however the splits nest: the four-pane grid's bottom row
/// (ratios 1/3, then 1/2) splits into three panes of one width, gaps between.
#[test]
fn nested_splits_share_the_space_after_the_gaps() {
    let gap = 6.0;
    let r: BTreeMap<_, _> = grid()
        .root
        .rects(PaneRect::new(0.0, 0.0, 1000.0, 606.0), gap)
        .into_iter()
        .collect();
    assert!((r[&PaneId(1)].h - 600.0 * 0.62).abs() < 1e-3);
    let w = (1000.0 - 2.0 * gap) / 3.0;
    for (i, id) in [PaneId(2), PaneId(3), PaneId(4)].into_iter().enumerate() {
        assert!((r[&id].w - w).abs() < 1e-3, "{id:?}: {:?}", r[&id]);
        assert!((r[&id].x - i as f32 * (w + gap)).abs() < 1e-3, "{id:?}");
    }
}

/// A closed pane's id never comes back: the next split takes a new one, also past the ids
/// of a layout read back from `ui.toml`.
#[test]
fn closed_pane_ids_are_not_reused() {
    let mut t = empty();
    t.key("Ctrl+N");
    t.key("Ctrl+N");
    let gone = t.st.layout.focus;
    t.key("Ctrl+D");
    t.key("Ctrl+N");
    let new = t.st.layout.focus;
    assert_ne!(new, gone);
    assert!(new.0 > gone.0, "{new:?} after {gone:?}");

    let mut u = T::fresh();
    let text = "[layout]\nfocus = 7\n\n[layout.tree]\nsplit = \"row\"\nratio = 0.5\na = { pane = 2 }\nb = { pane = 7 }\n";
    u.st.set_prefs(crate::prefs::UiPrefs::from_toml(text).expect("parse"));
    u.key("Ctrl+N");
    assert_eq!(u.st.layout.focus, PaneId(8));
}

/// Closing a pane closes its list, and a pick on a pane no longer there changes nothing.
#[test]
fn a_closed_pane_takes_its_list_and_picks_with_it() {
    let mut t = empty();
    t.conn(mirror(four()));
    t.key("Ctrl+N");
    let gone = t.st.layout.focus;
    t.st.update(Msg::PaneMenu(gone), &t.keys);
    assert!(matches!(t.st.overlay, Overlay::PaneMenu(m) if m.pane == gone));
    // The menu keeps the keys: the close comes as the command.
    t.st.update(Msg::Command(crate::keys::CommandId::ClosePane), &t.keys);
    assert!(t.st.layout.view(gone).is_none());
    assert!(matches!(t.st.overlay, Overlay::None), "{:?}", t.st.overlay);
    let before = t.st.selected;
    t.st.update(Msg::PanePick(gone, PaneMenuRow::Meas(MeasId(3))), &t.keys);
    assert_eq!(t.st.selected, before);
    assert_ne!(before, Some(MeasId(3)));
    assert!(t.st.layout.view(gone).is_none());
}

/// Closing a pane focuses the pane that bordered it: closing the right half of
/// `(a / c) | b` focuses `c`, the left subtree's leaf next to it, not `a`.
#[test]
fn closing_focuses_the_neighbour_along_the_split() {
    let mut t = empty();
    let a = t.st.layout.focus;
    t.key("Ctrl+N");
    let b = t.st.layout.focus;
    t.key("Alt+1");
    t.key("Ctrl+N");
    let c = t.st.layout.focus;
    assert_eq!(t.st.layout.root.reading_order(), [a, b, c]);
    t.key("Alt+2");
    assert_eq!(t.st.layout.focus, b);
    t.key("Ctrl+D");
    assert_eq!(t.st.layout.focus, c);
    // Closing an `a` side focuses the first leaf of its sibling.
    t.key("Alt+1");
    t.key("Ctrl+D");
    assert_eq!(t.st.layout.focus, c);
}

/// The axis of the split whose `a` is leaf `id`.
fn axis_before(n: &PaneNode, id: PaneId) -> Option<Axis> {
    match n {
        PaneNode::Leaf(_) => None,
        PaneNode::Split { axis, a, b, .. } => {
            if **a == PaneNode::Leaf(id) {
                Some(*axis)
            } else {
                axis_before(a, id).or_else(|| axis_before(b, id))
            }
        }
    }
}

/// A split measures the focused pane as it is laid out on screen: with a pane left out
/// (panes following the selection), the pane it gave its place to is tall and stacks.
#[test]
fn a_split_measures_the_pane_as_laid_out() {
    let mut t = empty();
    t.conn(mirror(four()));
    t.st.pane_area = (1280.0, 760.0);
    let a = t.st.layout.focus;
    t.key("Ctrl+N");
    let b = t.st.layout.focus;
    t.key("Ctrl+N");
    t.key("Ctrl+Tab");
    assert_eq!(kinds(&t)[2], PaneKind::Spectrum);
    t.st.update(Msg::FocusPane(a), &t.keys);
    t.st.prefs.panes_follow = true;
    t.st.update(Msg::FocusPane(b), &t.keys);
    assert_eq!(t.st.laid_out_panes(), [a, b]);
    t.key("Ctrl+N");
    // In the whole tree `b` is 640 × 380 and would split side by side.
    assert_eq!(axis_before(&t.st.layout.root, b), Some(Axis::Column));
}

/// Ctrl+N refuses a split whose halves would be too small to read, saying so, and every
/// pane stays at least that share of the area.
#[test]
fn ctrl_n_refuses_halves_too_small() {
    let mut t = empty();
    let (w, h) = (1280.0, 760.0);
    t.st.pane_area = (w, h);
    for _ in 0..40 {
        t.key("Ctrl+N");
    }
    assert!(
        t.last_toast().contains("too small to split"),
        "{}",
        t.last_toast()
    );
    let n = t.st.layout.panes().len();
    assert!(n < 41, "{n}");
    for (id, r) in t.st.pane_rects(PaneRect::new(0.0, 0.0, w, h), 0.0) {
        assert!(r.w >= 0.05 * w && r.h >= 0.05 * h, "{id:?}: {r:?}");
    }
}

/// A pane that follows the selection saves no measurement, so it follows again next run;
/// a pane added while the daemon is away saves none of a closed pane's.
#[test]
fn an_unchosen_pane_saves_no_measurement() {
    let mut t = empty();
    t.conn(mirror(four()));
    t.key("Ctrl+Tab");
    t.key("Ctrl+Shift+Tab");
    let f = t.st.layout.focus;
    assert_eq!(t.st.layout.view(f).expect("view").meas, None);
    assert!(t.st.pane_meas(f).is_some());
    let saved = t.st.layout_prefs().panes.expect("panes");
    assert_eq!(saved.views[0].measurement, None);

    let mut u = T::fresh();
    let text = "[layout]\nfocus = 2\n\n[layout.tree]\nsplit = \"row\"\nratio = 0.5\na = { pane = 1 }\nb = { pane = 2 }\n\n[[layout.panes]]\nid = 1\nkind = \"transfer\"\n\n[[layout.panes]]\nid = 2\nkind = \"transfer\"\nmeasurement = \"Delay tower\"\n";
    u.st.set_prefs(crate::prefs::UiPrefs::from_toml(text).expect("parse"));
    u.key("Ctrl+D");
    u.key("Ctrl+N");
    let saved = u.st.layout_prefs().panes.expect("panes");
    assert_eq!(saved.views.len(), 2);
    assert!(
        saved.views.iter().all(|v| v.measurement.is_none()),
        "{saved:?}"
    );
}
