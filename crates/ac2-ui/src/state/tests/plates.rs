//! Reducer tests of the plates dragged, scaled and put back on a pane.

use super::*;
use ac2_proto::model::PaneLayout;
use ac2_scene::legend::LegendCorner;
use ac2_scene::plate::{PlateKind, PlatePlace};

fn plates(t: &T, pane: PaneId) -> ac2_scene::plate::Plates {
    t.st.layout.view(pane).expect("pane").plates
}

fn layout_sent(r: &[Request]) -> Option<PaneLayout> {
    r.iter().find_map(|r| match r {
        Request::Call {
            cmd: Command::MeasLayout { layout, .. },
            ..
        } => layout.clone(),
        _ => None,
    })
}

fn plate(t: &mut T, m: PlateMsg) -> Vec<Request> {
    t.st.update(Msg::Plate(m), &t.keys)
}

/// A plate dragged is sent with its measurement's panes once, when it rests; the drag and
/// the grip are one undo step each, and named.
#[test]
fn a_dragged_plate_is_kept_sent_once_and_one_undo_step() {
    let mut t = T::new();
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    let pane = t.st.layout.focus;
    t.tick(1.0);
    let steps = t.st.history.undo_len();
    let kind = PlateKind::Readout;
    let mut r = Vec::new();
    for (x, y) in [(0.2, 0.1), (0.4, 0.3), (0.5, 0.6)] {
        r.extend(plate(&mut t, PlateMsg::Move { pane, kind, x, y }));
    }
    assert!(layout_sent(&r).is_none(), "not while dragged: {r:?}");
    r.extend(plate(&mut t, PlateMsg::End));
    assert_eq!(plates(&t, pane).readout.at, Some([0.5, 0.6]));
    r.extend(t.st.update(
        Msg::Tick {
            now_s: 2.0,
            dt_s: 1.0,
        },
        &t.keys,
    ));
    let calls = r
        .iter()
        .filter(|r| {
            matches!(
                r,
                Request::Call {
                    cmd: Command::MeasLayout { .. },
                    ..
                }
            )
        })
        .count();
    assert_eq!(calls, 1, "{r:?}");
    let sent = layout_sent(&r).expect("the layout, once the drag rests");
    let p = sent.panes()[0].plates.readout;
    assert_eq!(p.at.map(|a| a.map(|v| v as f32)), Some([0.5, 0.6]));
    assert_eq!(t.st.history.undo_len(), steps + 1);

    // The grip: the text scale, held to its bounds.
    for scale in [1.5, 2.5, 9.0] {
        plate(&mut t, PlateMsg::Scale { pane, kind, scale });
    }
    plate(&mut t, PlateMsg::End);
    assert_eq!(
        plates(&t, pane).readout.scale(),
        *ac2_scene::plate::SCALE_LIMITS.end()
    );
    assert_eq!(t.st.history.undo_len(), steps + 2);
    t.key("Ctrl+Z");
    let n =
        t.st.layout
            .panes()
            .iter()
            .position(|p| *p == pane)
            .expect("pane")
            + 1;
    assert_eq!(t.last_toast(), format!("Undid: plates of pane {n}"));
    assert_eq!(plates(&t, pane).readout.scale(), 1.0);
    assert_eq!(plates(&t, pane).readout.at, Some([0.5, 0.6]));
}

/// A double-click puts the plate back where the scene places it at the theme's size, and
/// leaves the pane's other plates.
#[test]
fn a_double_click_resets_only_that_plate() {
    let mut t = T::new();
    let pane = t.st.layout.focus;
    plate(
        &mut t,
        PlateMsg::Move {
            pane,
            kind: PlateKind::Legend,
            x: 0.3,
            y: 0.7,
        },
    );
    plate(
        &mut t,
        PlateMsg::Scale {
            pane,
            kind: PlateKind::Legend,
            scale: 2.0,
        },
    );
    plate(
        &mut t,
        PlateMsg::Move {
            pane,
            kind: PlateKind::Mark,
            x: 1.0,
            y: 0.0,
        },
    );
    plate(&mut t, PlateMsg::End);
    plate(
        &mut t,
        PlateMsg::Reset {
            pane,
            kind: PlateKind::Legend,
        },
    );
    plate(&mut t, PlateMsg::End);
    let p = plates(&t, pane);
    assert_eq!(p.legend, PlatePlace::default());
    assert_eq!(p.mark.at, Some([1.0, 0.0]));
}

/// A drop outside 0 … 1 (the pointer left the plot) is held inside it.
#[test]
fn a_plate_never_leaves_its_plot() {
    let mut t = T::new();
    let pane = t.st.layout.focus;
    plate(
        &mut t,
        PlateMsg::Move {
            pane,
            kind: PlateKind::Legend,
            x: -0.4,
            y: 1.8,
        },
    );
    plate(&mut t, PlateMsg::End);
    assert_eq!(plates(&t, pane).legend.at, Some([0.0, 1.0]));
}

/// The palette's legend corners move the legend of the pane the legend is drawn on, at
/// its text size.
#[test]
fn a_legend_corner_keeps_the_text_size() {
    let mut t = T::new();
    let pane = t.st.layout.focus;
    plate(
        &mut t,
        PlateMsg::Scale {
            pane,
            kind: PlateKind::Legend,
            scale: 2.0,
        },
    );
    plate(&mut t, PlateMsg::End);
    t.st.snap_legend(LegendCorner::BottomLeft);
    let p = plates(&t, pane).legend;
    assert_eq!(p.at, Some([0.0, 1.0]));
    assert_eq!(p.scale(), 2.0);
}
