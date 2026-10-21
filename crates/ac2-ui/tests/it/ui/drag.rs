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
        modes: Default::default(),
        levels: Default::default(),
        plates: Default::default(),
    };
    p.layout.panes = Some(ac2_ui::prefs::PanesPrefs {
        root: PaneNode::Split {
            axis: Axis::Column,
            ratio: 0.5,
            a: Box::new(PaneNode::Leaf(PaneId(1))),
            b: Box::new(PaneNode::Leaf(PaneId(2))),
        },
        focus: PaneId(1),
        views: vec![pane(1, PaneKind::Transfer), pane(2, PaneKind::Spectrum)],
    });
    p
}

/// `FOH SPL`'s row dragged onto the lower pane: mid-drag the pane is shaded with a dashed
/// edge and the label by the pointer says `FOH SPL → pane 2`; dropped, the lower pane is
/// an SPL pane showing it, focused, and the upper pane is as it was.
#[test]
fn a_list_row_dragged_onto_the_lower_pane() {
    use egui_kittest::kittest::By;
    if !have_gpu("a_list_row_dragged_onto_the_lower_pane") {
        return;
    }
    let rig = common::Rig::start();
    let mut opts = options(Some(&rig));
    opts.prefs = two_panes();
    let mut h = harness(opts);
    // Only what the two panes draw is subscribed: the transfer and the spectrum.
    step_until(&mut h, "live frames", |a| {
        let st = &a.state;
        let have = |m: u32, s: Stream| {
            st.data.as_ref().is_some_and(|d| {
                d.latest
                    .get(&Topic::Data {
                        meas: MeasId(m),
                        stream: s,
                    })
                    .is_some()
            })
        };
        matches!(st.conn, ConnState::Connected { .. })
            && st.mirror.as_ref().is_some_and(|m| m.synced())
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
        Some("FOH SPL \u{2192} pane 2")
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
        a.state.data.as_ref().is_some_and(|d| {
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
