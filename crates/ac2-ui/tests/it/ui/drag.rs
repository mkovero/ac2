//! A row of the measurement list dragged onto a pane.

use super::*;

/// Transfer over spectrum, the transfer pane focused.
fn two_panes() -> ac2_ui::prefs::UiPrefs {
    use ac2_ui::state::{Axis, PaneId, PaneKind, PaneNode};
    let mut p = common::grid_ui_prefs();
    let pane = |n, kind| ac2_ui::prefs::PanePrefs {
        id: PaneId(n),
        kind,
        measurement: None,
        curves: Vec::new(),
        meters: None,
        rig: None,
        modes: Default::default(),
        levels: Default::default(),
        plates: Default::default(),
    };
    p.layout.panes = Some(common::rig_picks(ac2_ui::prefs::PanesPrefs {
        root: PaneNode::Split {
            axis: Axis::Column,
            ratio: 0.5,
            a: Box::new(PaneNode::Leaf(PaneId(1))),
            b: Box::new(PaneNode::Leaf(PaneId(2))),
        },
        focus: PaneId(1),
        views: vec![pane(1, PaneKind::Transfer), pane(2, PaneKind::Spectrum)],
    }));
    p
}

/// `FOH SPL`'s row dragged onto the lower pane: mid-drag the pane is shaded with a dashed
/// edge and the label by the pointer says `FOH SPL → pane 2, becomes SPL`; dropped, the lower pane is
/// an SPL pane showing it, focused, and the upper pane is as it was.
#[test]
fn a_list_row_dragged_onto_the_lower_pane() {
    use egui_kittest::kittest::By;
    if !have_gpu("a_list_row_dragged_onto_the_lower_pane") {
        return;
    }
    let _rig = Rig::start();
    let mut opts = options();
    opts.prefs = two_panes();
    let mut h = harness(opts);
    // Only what the two panes draw is subscribed: the transfer and the spectrum.
    step_until(&mut h, "live frames", |a| {
        let st = &a.state;
        let have = |m: u32, s: Stream| {
            st.rig.data.as_ref().is_some_and(|d| {
                d.latest
                    .get(&Topic::Data {
                        meas: MeasId(m),
                        stream: s,
                    })
                    .is_some()
            })
        };
        matches!(st.rig.conn, ConnState::Connected { .. })
            && st.rig.mirror.as_ref().is_some_and(|m| m.synced())
            && st.measurements().len() == 4
            && have(1, Stream::Tf)
            && have(3, Stream::Spec)
    });
    let row = h
        .query_all(
            By::new().predicate(|n| n.label().is_some_and(|l| l.starts_with("SPL  FOH SPL\n"))),
        )
        .map(|n| n.rect())
        .find(|r| r.max.x < 230.0)
        .expect("FOH SPL's row in the list");
    let panes = h.state().pane_rects.clone();
    assert_eq!(panes.len(), 2);
    let (lower, lower_rect) = panes[1];
    let upper = h.state().state.layout.view(panes[0].0).copied();
    let from = row.center();
    let to = lower_rect.center();
    let button = |pos, pressed| Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    h.event(Event::PointerMoved(from));
    settle(&mut h, 2);
    h.event(button(from, true));
    settle(&mut h, 2);
    for k in 1..=8 {
        h.event(Event::PointerMoved(from + (to - from) * (k as f32 / 8.0)));
        settle(&mut h, 1);
    }
    assert_eq!(
        h.state().state.drag_label().as_deref(),
        Some("FOH SPL \u{2192} pane 2, becomes SPL")
    );
    assert_eq!(h.state().state.drop_highlight(), Some((lower, None)));
    h.state_mut().state.toasts.list.clear();
    snapshot(&mut h, "row_drag_over_pane");
    h.event(button(to, false));
    settle(&mut h, 3);
    let st = &h.state().state;
    assert!(!st.row_drag_held());
    assert_eq!(st.layout.kind(lower), PaneKind::Spl);
    assert_eq!(st.pane_meas(lower).map(|m| m.id), Some(MeasId(4)));
    assert_eq!(st.layout.focus, lower);
    assert_eq!(st.layout.view(panes[0].0).copied(), upper);
    // The SPL pane's frames, once it subscribes to them.
    step_until(&mut h, "the meter's frames", |a| {
        a.state.rig.data.as_ref().is_some_and(|d| {
            d.latest
                .get(&Topic::Data {
                    meas: MeasId(4),
                    stream: Stream::Spl,
                })
                .is_some()
        })
    });
    h.state_mut().state.toasts.list.clear();
    snapshot(&mut h, "row_dropped_on_pane");
}

/// A measurement's row in the list: one click only selects it, its edit button (shown under
/// the pointer) opens its form, and so does a double click, which starts no drag.
#[test]
fn a_measurement_row_is_edited_by_its_button_and_a_double_click() {
    use egui_kittest::kittest::By;
    if !have_gpu("a_measurement_row_is_edited_by_its_button_and_a_double_click") {
        return;
    }
    let _rig = Rig::start();
    let mut opts = options();
    opts.prefs = two_panes();
    let mut h = harness(opts);
    step_until(&mut h, "the measurements", |a| {
        a.state.rig.mirror.as_ref().is_some_and(|m| m.synced()) && a.state.measurements().len() == 4
    });
    let labelled = |h: &Harness<'_, App>, start: &'static str| {
        h.query_all(By::new().predicate(move |n| n.label().is_some_and(|l| l.starts_with(start))))
            .map(|n| n.rect())
            .find(|r| r.max.x < 260.0)
    };
    let editing = |h: &Harness<'_, App>| match &h.state().state.overlay {
        Overlay::Form(f) => f.edits(),
        _ => None,
    };
    let button = |pos, pressed| Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    // The spectrum's row is not the selected one: no button until the pointer is over it.
    let row = labelled(&h, "FFT  ").expect("the spectrum's row in the list");
    assert_eq!(labelled(&h, "Edit FFT  "), None);
    let at = row.left_center() + egui::vec2(30.0, 0.0);
    h.event(Event::PointerMoved(at));
    settle(&mut h, 2);
    let edit = labelled(&h, "Edit FFT  ").expect("the hovered row's edit button");
    assert_eq!(labelled(&h, "FFT  "), Some(row), "the row is where it was");
    // One click selects, and opens nothing.
    h.event(button(at, true));
    h.event(button(at, false));
    settle(&mut h, 2);
    assert_eq!(h.state().state.rig.selected, Some(MeasId(3)));
    assert_eq!(h.state().state.overlay, Overlay::None);
    // The button edits its row.
    h.event(Event::PointerMoved(edit.center()));
    settle(&mut h, 2);
    h.event(button(edit.center(), true));
    h.event(button(edit.center(), false));
    settle(&mut h, 2);
    assert_eq!(editing(&h), Some(MeasId(3)), "the spectrum's form");
    h.state_mut().state.overlay = Overlay::None;
    settle(&mut h, 2);
    // A double click on another row selects it and edits it. Both clicks go into one
    // frame's input: the harness gives each queued event a frame of a quarter second, near
    // the longest gap egui takes for a double click.
    let row = labelled(&h, "TF  ").expect("the transfer's row in the list");
    let at = row.left_center() + egui::vec2(30.0, 0.0);
    h.event(Event::PointerMoved(at));
    settle(&mut h, 2);
    for _ in 0..2 {
        h.input_mut().events.push(button(at, true));
        h.input_mut().events.push(button(at, false));
    }
    settle(&mut h, 3);
    assert_eq!(h.state().state.rig.selected, Some(MeasId(1)));
    assert_eq!(editing(&h), Some(MeasId(1)), "the transfer's form");
    assert!(!h.state().state.row_drag_held());
}

/// The Settings view covers the list and owns the keys: a double click where a row is and
/// the edit key reach no row, so an edit never replaces the page Settings is open on.
#[test]
fn settings_open_no_row_is_edited() {
    use ac2_ui::keys::CommandId;
    use ac2_ui::state::Msg;
    use egui_kittest::kittest::By;
    if !have_gpu("settings_open_no_row_is_edited") {
        return;
    }
    let _rig = Rig::start();
    let mut opts = options();
    opts.prefs = two_panes();
    let mut h = harness(opts);
    step_until(&mut h, "the measurements", |a| {
        a.state.rig.mirror.as_ref().is_some_and(|m| m.synced()) && a.state.measurements().len() == 4
    });
    let row = h
        .query_all(By::new().predicate(|n| n.label().is_some_and(|l| l.starts_with("FFT  "))))
        .map(|n| n.rect())
        .find(|r| r.max.x < 260.0)
        .expect("the spectrum's row in the list");
    let selected = h.state().state.rig.selected;
    assert_ne!(
        selected,
        Some(MeasId(3)),
        "the spectrum is not the selected row"
    );
    h.state_mut().dispatch(Msg::Command(CommandId::Settings));
    settle(&mut h, 2);
    // The page and the meter its SPL/Leq page is on; the key itself is the open page's.
    let open = |h: &Harness<'_, App>| match &h.state().state.overlay {
        Overlay::Settings(s) => Some((s.page, s.leq.clone())),
        _ => None,
    };
    let before = open(&h);
    assert!(before.is_some());
    let at = row.left_center() + egui::vec2(30.0, 0.0);
    h.event(Event::PointerMoved(at));
    settle(&mut h, 2);
    for _ in 0..2 {
        for pressed in [true, false] {
            h.input_mut().events.push(Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
    }
    settle(&mut h, 3);
    h.key_press(Key::F2);
    settle(&mut h, 2);
    assert_eq!(h.state().state.rig.selected, selected, "no row was clicked");
    assert_eq!(open(&h), before, "Settings on the page and meter it was");
}
