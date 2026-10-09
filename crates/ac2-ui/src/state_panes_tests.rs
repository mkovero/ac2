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
