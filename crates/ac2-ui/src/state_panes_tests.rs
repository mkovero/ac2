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

/// N splits the focused pane along its longer side and focuses the new half: a wide
/// window splits left | right, the tall right half then top / bottom.
#[test]
fn n_twice_gives_three_panes() {
    let mut t = empty();
    let first = t.st.layout.focus;
    t.key("N");
    t.key("N");
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

/// Q closes the focused pane, its sibling taking the space and the focus; the last pane
/// stays, saying how to change what it shows.
#[test]
fn q_closes_and_keeps_the_last_pane() {
    let mut t = empty();
    let first = t.st.layout.focus;
    t.key("Q");
    assert_eq!(t.st.layout.root, PaneNode::Leaf(first));
    assert!(
        t.last_toast().contains("the last pane stays"),
        "{}",
        t.last_toast()
    );
    t.key("N");
    assert_ne!(t.st.layout.focus, first);
    t.key("Q");
    assert_eq!(t.st.layout.root, PaneNode::Leaf(first));
    assert_eq!(t.st.layout.focus, first);
}

/// The views of every pane but the focused one.
fn others(t: &T) -> Vec<(PaneId, View)> {
    let l = &t.st.layout;
    l.views
        .iter()
        .filter(|(id, _)| **id != l.focus)
        .map(|(id, v)| (*id, *v))
        .collect()
}

/// Tab puts the next measurement of the tree in the focused pane, which turns into that
/// measurement's kind of pane; Shift+Tab goes back. No other pane changes.
#[test]
fn tab_puts_the_next_measurement_in_the_focused_pane() {
    let mut t = empty();
    t.key("Tab");
    assert!(
        t.last_toast().contains("no measurements"),
        "{}",
        t.last_toast()
    );
    let mut s = four();
    s.measurements.push(meas(5, "FOH SPL", spl_meter()));
    t.conn(mirror(s));
    t.key("N");
    t.key("N");
    // The meter in the focused pane: an SPL pane.
    t.st.update(Msg::SelectMeas(MeasId(5)), &t.keys);
    let f = t.st.layout.focus;
    assert_eq!(t.focus_kind(), PaneKind::Spl);
    let before = others(&t);
    let order = t.st.tree_meas_order();
    let at = order.iter().position(|m| *m == MeasId(5)).expect("listed");
    let next = order[(at + 1) % order.len()];
    let kind_of = |t: &T, id: MeasId| PaneKind::for_kind(&t.st.meas(id).expect("meas").config.kind);
    assert_eq!(kind_of(&t, next), PaneKind::Transfer, "{order:?}");
    t.key("Tab");
    assert_eq!(t.st.layout.focus, f);
    assert_eq!(t.focus_kind(), PaneKind::Transfer);
    assert_eq!(t.st.pane_meas(f).map(|m| m.id), Some(next));
    assert_eq!(t.st.selected, Some(next));
    assert_eq!(others(&t), before);
    // Every measurement of the tree in turn, each in its kind of pane.
    for _ in 0..order.len() {
        t.key("Tab");
        let m = t.st.pane_meas(f).map(|m| m.id).expect("shown");
        assert_eq!(t.focus_kind(), kind_of(&t, m));
        assert_eq!(t.st.layout.focus, f);
        assert_eq!(others(&t), before);
    }
    // Back: the meter again, in an SPL pane.
    t.key("Shift+Tab");
    assert_eq!(t.st.pane_meas(f).map(|m| m.id), Some(MeasId(5)));
    assert_eq!(t.focus_kind(), PaneKind::Spl);
    assert_eq!(others(&t), before);
}

/// Two transfer panes and a spectrum measurement: Tab, a list pick and the pane's own list
/// each put what is picked in the focused pane, which changes kind to draw it; the focus
/// stays and the other pane keeps what it shows.
#[test]
fn the_focused_pane_takes_a_measurement_of_any_kind() {
    let mut t = empty();
    t.conn(mirror(four()));
    let a = t.st.layout.focus;
    t.key("N");
    let b = t.st.layout.focus;
    assert_eq!(kinds(&t), [PaneKind::Transfer, PaneKind::Transfer]);
    t.key("Alt+1");
    t.key("Alt+2");
    assert_eq!(t.st.layout.focus, b);
    let a_shows = shows(&t, a);
    assert!(a_shows.is_some());
    let a_view = t.st.layout.views[&a];
    let is_spectrum = |t: &T| t.st.layout.kind(b) == PaneKind::Spectrum;
    for _ in 0..t.st.tree_meas_order().len() {
        if is_spectrum(&t) {
            break;
        }
        t.key("Tab");
    }
    assert!(is_spectrum(&t), "{:?}", kinds(&t));
    assert_eq!(t.st.layout.focus, b);
    assert!(matches!(shows(&t, b), Some(2 | 4)), "{:?}", shows(&t, b));
    assert_eq!(t.st.layout.views[&a], a_view);
    assert_eq!(shows(&t, a), a_shows);
    // Tab on to a transfer measurement: the same pane turns back, the focus does not jump
    // to the transfer pane already on screen.
    for _ in 0..t.st.tree_meas_order().len() {
        if !is_spectrum(&t) {
            break;
        }
        t.key("Tab");
    }
    assert_eq!(kinds(&t), [PaneKind::Transfer, PaneKind::Transfer]);
    assert_eq!(t.st.layout.focus, b);
    assert!(matches!(shows(&t, b), Some(1 | 3)), "{:?}", shows(&t, b));
    assert_eq!(t.st.layout.views[&a], a_view);
    // A pick from the list.
    t.st.update(Msg::SelectMeas(MeasId(2)), &t.keys);
    assert_eq!(kinds(&t), [PaneKind::Transfer, PaneKind::Spectrum]);
    assert_eq!((t.st.layout.focus, shows(&t, b)), (b, Some(2)));
    assert_eq!(t.st.layout.views[&a], a_view);
    // The pane's list has every measurement, in the tree's order; a transfer one picked
    // there turns the spectrum pane back.
    let rows: Vec<MeasId> =
        t.st.pane_menu_rows(b)
            .into_iter()
            .filter_map(|(r, _)| match r {
                PaneMenuRow::Meas(m) => Some(m),
                PaneMenuRow::Kind(_) => None,
            })
            .collect();
    assert_eq!(rows, t.st.tree_meas_order());
    t.st.update(Msg::PaneMenu(b), &t.keys);
    let Overlay::PaneMenu(menu) = t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    let want = rows.iter().position(|m| *m == MeasId(3)).expect("listed");
    t.key("Home");
    for _ in 0..want {
        t.key("Down");
    }
    assert_eq!(menu.pane, b);
    t.key("Enter");
    assert_eq!(kinds(&t), [PaneKind::Transfer, PaneKind::Transfer]);
    assert_eq!((t.st.layout.focus, shows(&t, b)), (b, Some(3)));
    assert_eq!(t.st.selected, Some(MeasId(3)));
    assert_eq!(t.st.layout.views[&a], a_view);
}

/// G steps the focused transfer pane through response → phase → coherence → impulse
/// response → response; the phase and coherence views are that plot alone, and the IR view
/// has the IR's keys (Shift+G its mode).
#[test]
fn g_steps_the_transfer_views() {
    use ac2_scene::tf::TfPaneKind as P;
    let mut t = empty();
    t.conn(mirror(four()));
    let f = t.st.layout.focus;
    let plots = |t: &T| -> Vec<P> {
        crate::scenes::transfer(&t.st, f, &Theme::dark(), SIZE, now())
            .panes
            .iter()
            .map(|p| p.kind)
            .collect()
    };
    let view = |t: &T| t.st.layout.focused().modes.transfer;
    assert_eq!(view(&t), TransferView::Response);
    assert_eq!(plots(&t).first(), Some(&P::Magnitude));
    t.key("G");
    assert_eq!(view(&t), TransferView::Phase);
    assert_eq!(plots(&t), [P::Phase]);
    t.key("G");
    assert_eq!(view(&t), TransferView::Coherence);
    assert_eq!(plots(&t), [P::Coherence]);
    t.key("G");
    assert_eq!(view(&t), TransferView::Ir);
    assert_eq!(t.st.scope(), crate::keys::Scope::Ir);
    assert!(crate::scenes::ir(&t.st, f, &t.keys, &Theme::dark(), SIZE, now()).is_some());
    t.key("Shift+G");
    assert_eq!(t.st.layout.focused().modes.ir, IrMode::Log);
    t.key("G");
    assert_eq!(view(&t), TransferView::Response);
    assert_eq!(t.st.scope(), crate::keys::Scope::Transfer);
    assert_eq!(plots(&t).first(), Some(&P::Magnitude));
    // The phase and coherence views draw no magnitude: the level keys say so.
    t.key("G");
    t.key("Ctrl+I");
    assert!(
        t.last_toast().contains("no level axis"),
        "{}",
        t.last_toast()
    );
}

/// Two transfer panes keep their own measurements and their own views: picking in the list
/// changes the focused one only, each draws its own curve, and G steps the focused one.
#[test]
fn two_transfer_panes_keep_their_measurements_and_views() {
    let mut t = empty();
    t.conn(mirror(four()));
    super::tf_group::tf_frames(&mut t, &[1, 3]);
    let a = t.st.layout.focus;
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    t.key("N");
    let b = t.st.layout.focus;
    assert_eq!((shows(&t, a), shows(&t, b)), (Some(1), Some(1)));
    t.st.update(Msg::SelectMeas(MeasId(3)), &t.keys);
    assert_eq!((shows(&t, a), shows(&t, b)), (Some(1), Some(3)));
    t.key("Alt+1");
    assert_eq!((shows(&t, a), shows(&t, b)), (Some(1), Some(3)));
    assert_eq!(t.st.selected, Some(MeasId(1)));
    assert_eq!(legend(&t, a), ["Main L"]);
    assert_eq!(legend(&t, b), ["Delay tower"]);

    // The second in its IR view, the first still drawing the response.
    t.key("Alt+2");
    t.key("G");
    t.key("G");
    t.key("G");
    let mode = |t: &T, id: PaneId| t.st.layout.view(id).expect("view").modes.transfer;
    assert_eq!(
        (mode(&t, a), mode(&t, b)),
        (TransferView::Response, TransferView::Ir)
    );
    assert_eq!(shows(&t, b), Some(3));
    // A split copies the view; the halves then step apart.
    t.key("N");
    let c = t.st.layout.focus;
    assert_eq!((mode(&t, c), shows(&t, c)), (TransferView::Ir, Some(3)));
    t.key("G");
    assert_eq!(
        (mode(&t, b), mode(&t, c)),
        (TransferView::Ir, TransferView::Response)
    );
    t.st.update(Msg::FocusPane(a), &t.keys);
    assert_eq!((shows(&t, b), shows(&t, c)), (Some(3), Some(3)));
}

/// Alt+number focuses the panes in reading order; past the last one it says how many there
/// are.
#[test]
fn alt_numbers_focus_panes_in_reading_order() {
    let mut t = empty();
    t.key("N");
    t.key("N");
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
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    t.key("N");
    t.st.update(Msg::SelectMeas(MeasId(3)), &t.keys);
    // The second transfer pane on its phase view.
    t.key("G");
    t.key("N");
    t.st.update(Msg::SelectMeas(MeasId(4)), &t.keys);
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
    assert_eq!(before[1].2.transfer, TransferView::Phase);
    let text = t.st.prefs.to_toml();
    assert!(text.contains("measurement = \"Delay tower\""), "{text}");
    assert!(text.contains("transfer_view = \"phase\""), "{text}");

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
    t.key("N");
    t.key("N");
    let gone = t.st.layout.focus;
    t.key("Q");
    t.key("N");
    let new = t.st.layout.focus;
    assert_ne!(new, gone);
    assert!(new.0 > gone.0, "{new:?} after {gone:?}");

    let mut u = T::fresh();
    let text = "[layout]\nfocus = 7\n\n[layout.tree]\nsplit = \"row\"\nratio = 0.5\na = { pane = 2 }\nb = { pane = 7 }\n";
    u.st.set_prefs(crate::prefs::UiPrefs::from_toml(text).expect("parse"));
    u.key("N");
    assert_eq!(u.st.layout.focus, PaneId(8));
}

/// Closing a pane closes its list, and a pick on a pane no longer there changes nothing.
#[test]
fn a_closed_pane_takes_its_list_and_picks_with_it() {
    let mut t = empty();
    t.conn(mirror(four()));
    t.key("N");
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
    t.key("N");
    let b = t.st.layout.focus;
    t.key("Alt+1");
    t.key("N");
    let c = t.st.layout.focus;
    assert_eq!(t.st.layout.root.reading_order(), [a, b, c]);
    t.key("Alt+2");
    assert_eq!(t.st.layout.focus, b);
    t.key("Q");
    assert_eq!(t.st.layout.focus, c);
    // Closing an `a` side focuses the first leaf of its sibling.
    t.key("Alt+1");
    t.key("Q");
    assert_eq!(t.st.layout.focus, c);
}

/// N refuses a split whose halves would be too small to read, saying so, and every pane
/// stays at least that share of the area.
#[test]
fn n_refuses_halves_too_small() {
    let mut t = empty();
    let (w, h) = (1280.0, 760.0);
    t.st.pane_area = (w, h);
    for _ in 0..40 {
        t.key("N");
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
    let f = t.st.layout.focus;
    assert_eq!(t.st.layout.view(f).expect("view").meas, None);
    assert!(t.st.pane_meas(f).is_some());
    let saved = t.st.layout_prefs().panes.expect("panes");
    assert_eq!(saved.views[0].measurement, None);

    let mut u = T::fresh();
    let text = "[layout]\nfocus = 2\n\n[layout.tree]\nsplit = \"row\"\nratio = 0.5\na = { pane = 1 }\nb = { pane = 2 }\n\n[[layout.panes]]\nid = 1\nkind = \"transfer\"\n\n[[layout.panes]]\nid = 2\nkind = \"transfer\"\nmeasurement = \"Delay tower\"\n";
    u.st.set_prefs(crate::prefs::UiPrefs::from_toml(text).expect("parse"));
    u.key("Q");
    u.key("N");
    let saved = u.st.layout_prefs().panes.expect("panes");
    assert_eq!(saved.views.len(), 2);
    assert!(
        saved.views.iter().all(|v| v.measurement.is_none()),
        "{saved:?}"
    );
}
