//! Reducer tests of a measurement's own panes (`meas.layout`).

use super::*;
use ac2_proto::model::PaneLayout;
use ac2_proto::model::layout as wire;

/// Magnitude over phase, the way an operator lays out a transfer measurement.
fn two_transfer() -> PaneLayout {
    let pane = |transfer_view| PaneLayout::Pane {
        pane: wire::LayoutPane {
            kind: wire::PaneKind::Transfer,
            transfer_view,
            spectrum_view: wire::SpectrumView::Spectrum,
            spl_view: wire::SplView::MeterLeq,
            sweep_view: wire::SweepView::Response,
            chrome: wire::PlotChrome::Full,
        },
    };
    PaneLayout::Split {
        axis: wire::SplitAxis::Column,
        ratio: 0.5,
        a: Box::new(pane(wire::TransferView::Magnitude)),
        b: Box::new(pane(wire::TransferView::Phase)),
    }
}

/// The test daemon with measurement `id` carrying `layout`.
fn with_layout(id: u32, layout: Option<PaneLayout>) -> State {
    let mut s = daemon_state();
    for m in &mut s.measurements {
        if m.id == MeasId(id) {
            m.layout = layout.clone();
        }
    }
    s
}

fn layout_calls(r: &[Request]) -> Vec<(MeasId, Option<PaneLayout>)> {
    r.iter()
        .filter_map(|r| match r {
            Request::Call {
                cmd: Command::MeasLayout { meas, layout },
                ..
            } => Some((*meas, layout.clone())),
            _ => None,
        })
        .collect()
}

fn select(t: &mut T, id: u32) -> Vec<Request> {
    t.st.update(Msg::SelectMeas(MeasId(id)), &t.keys)
}

/// Selecting a measurement with its own panes puts them on screen, the focus in the pane
/// of its kind; selecting one without keeps them; neither sends anything.
#[test]
fn selecting_a_measurement_brings_its_panes() {
    let mut t = T::new();
    assert!(layout_calls(&select(&mut t, 2)).is_empty());
    let grid = t.st.pane_layout();
    t.conn(mirror(with_layout(1, Some(two_transfer()))));
    assert_eq!(t.st.pane_layout(), grid, "Sub selected: its panes stay");
    let r = select(&mut t, 1);
    assert!(layout_calls(&r).is_empty(), "{r:?}");
    assert_eq!(t.st.pane_layout(), two_transfer());
    assert_eq!(t.st.layout.focus_kind(), PaneKind::Transfer);
    assert_eq!(
        t.st.pane_meas(t.st.layout.focus).map(|m| m.id),
        Some(MeasId(1))
    );
    // Sub has none of its own: the screen keeps Main L's panes, the focused one turned
    // into a spectrum pane to show it (as selecting always does), and that is no edit.
    let r = select(&mut t, 2);
    assert!(layout_calls(&r).is_empty(), "{r:?}");
    let kinds: Vec<_> = t.st.pane_layout().panes().iter().map(|p| p.kind).collect();
    assert_eq!(kinds, [wire::PaneKind::Spectrum, wire::PaneKind::Transfer]);
    // Back to Main L: its own panes again.
    select(&mut t, 1);
    assert_eq!(t.st.pane_layout(), two_transfer());
}

/// An edit while a measurement is selected is sent once as its layout; the daemon's echo
/// changes nothing on screen and sends nothing back.
#[test]
fn editing_panes_stores_them_on_the_selected_measurement() {
    let mut t = T::new();
    select(&mut t, 1);
    let before = t.st.layout.panes().len();
    let r = t.key("N");
    assert_eq!(t.st.layout.panes().len(), before + 1, "split");
    let calls = layout_calls(&r);
    assert_eq!(calls.len(), 1, "{r:?}");
    let (meas, sent) = calls[0].clone();
    assert_eq!(meas, MeasId(1));
    let sent = sent.expect("a layout");
    assert_eq!(sent, t.st.pane_layout());
    assert_eq!(sent.panes().len(), before + 1);
    let focus = t.st.layout.focus;
    let r = t.conn(mirror(with_layout(1, Some(sent.clone()))));
    assert!(
        layout_calls(&r).is_empty(),
        "the echo is not sent back: {r:?}"
    );
    assert_eq!(t.st.layout.focus, focus, "the echo leaves the focus");
    // The first layout a measurement gets is said; later edits are not.
    let r = t.conn(ConnEvent::Reply {
        what: "Main L: 5 own panes".into(),
        result: Ok(()),
    });
    assert!(r.is_empty());
    assert_eq!(t.last_toast(), "Main L: 5 own panes");
    // No measurement selected: nothing is sent.
    let mut t = T::new();
    t.st.selected = None;
    let r = t.key("N");
    assert!(layout_calls(&r).is_empty(), "{r:?}");
}

/// Another client's layout for the selected measurement is put on screen and not sent
/// back; while this app's own call is unanswered the daemon's older copy is not.
#[test]
fn a_layout_received_for_the_selected_measurement_applies() {
    let mut t = T::new();
    select(&mut t, 1);
    let r = t.conn(mirror(with_layout(1, Some(two_transfer()))));
    assert!(layout_calls(&r).is_empty(), "{r:?}");
    assert_eq!(t.st.pane_layout(), two_transfer());
    // Received for a measurement not selected: the screen stays.
    let mut other = two_transfer();
    if let PaneLayout::Split { ratio, .. } = &mut other {
        *ratio = 0.25;
    }
    let mut s = with_layout(1, Some(two_transfer()));
    s.measurements[0].layout = Some(other.clone());
    t.conn(mirror(s));
    assert_eq!(t.st.pane_layout(), two_transfer());
    // An edit in flight: the daemon's copy from before it does not undo it.
    let r = t.key("N");
    assert_eq!(layout_calls(&r).len(), 1);
    let edited = t.st.pane_layout();
    t.conn(mirror(with_layout(1, Some(other.clone()))));
    assert_eq!(t.st.pane_layout(), edited, "this app's call is unanswered");
    // Cleared elsewhere: the panes stay.
    t.conn(ConnEvent::Reply {
        what: "Main L: 3 own panes".into(),
        result: Ok(()),
    });
    let r = t.conn(mirror(with_layout(1, None)));
    assert!(layout_calls(&r).is_empty(), "{r:?}");
    assert_eq!(t.st.pane_layout(), edited);
}

/// A split being dragged is sent once, when it rests, not at every step of the drag.
#[test]
fn a_dragged_split_is_sent_when_it_rests() {
    let mut t = T::new();
    select(&mut t, 1);
    t.tick(1.0);
    for ratio in [0.5, 0.55, 0.6] {
        let r = t.st.update(
            Msg::DragSplit {
                path: vec![],
                ratio,
            },
            &t.keys,
        );
        assert!(layout_calls(&r).is_empty(), "{r:?}");
    }
    let r = t.st.update(
        Msg::Tick {
            now_s: 1.1,
            dt_s: 0.1,
        },
        &t.keys,
    );
    assert!(layout_calls(&r).is_empty(), "{r:?}");
    let r = t.st.update(
        Msg::Tick {
            now_s: 2.0,
            dt_s: 0.9,
        },
        &t.keys,
    );
    let calls = layout_calls(&r);
    assert_eq!(calls.len(), 1, "{r:?}");
    match calls[0].1.as_ref() {
        Some(PaneLayout::Split { ratio, .. }) => assert!((ratio - 0.6).abs() < 1e-6),
        other => panic!("{other:?}"),
    }
}
