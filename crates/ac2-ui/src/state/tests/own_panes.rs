//! A measurement row chosen in the list brings its own panes at once: SPL meters split.

use super::*;
use ac2_proto::model::PaneLayout;
use ac2_proto::model::layout as wire;

/// A meter over its Leq history, both panes of meter `id`, as an operator splits an SPL
/// meter.
fn split_spl(id: u32) -> PaneLayout {
    let pane = |spl_view| PaneLayout::Pane {
        pane: Box::new(wire::LayoutPane {
            kind: wire::PaneKind::Spl,
            transfer_view: wire::TransferView::Magnitude,
            spectrum_view: wire::SpectrumView::Spectrum,
            spl_view,
            sweep_view: wire::SweepView::Response,
            programme_view: wire::ProgrammeView::Meters,
            chrome: wire::PlotChrome::Full,
            content: Some(wire::PaneContent::Meas { meas: MeasId(id) }),
            levels: wire::LayoutLevels::default(),
            plates: wire::LayoutPlates::default(),
            rig: wire::RigRef::ThisRig,
        }),
    };
    PaneLayout::Split {
        axis: wire::SplitAxis::Row,
        ratio: 0.5,
        a: Box::new(pane(wire::SplView::Meter)),
        b: Box::new(pane(wire::SplView::Leq)),
    }
}

/// The test daemon with `FOH SPL` (4) and `SPL 2` (5), the second with its panes split.
fn two_meters() -> State {
    let mut s = with_spl();
    let mut second = meas(5, "SPL 2", spl_meter());
    if let MeasKind::Spl { config } = &mut second.config.kind {
        config.input = 2;
    }
    second.layout = Some(split_spl(5));
    s.measurements.push(second);
    s
}

fn click(t: &mut T, id: u32) {
    t.st.apply(Msg::SelectMeas(MeasId(id)), &t.keys);
}

/// `SPL 2`'s two panes are the layout, and both are drawn (beside `others` of another rig).
fn both_shown_beside(t: &T, others: usize, what: &str) {
    assert_eq!(t.st.pane_layout(), split_spl(5), "{what}");
    let area = PaneRect::new(0.0, 0.0, 1000.0, 600.0);
    let drawn = t.st.pane_rects(area, 0.0).len();
    assert_eq!(drawn, 2 + others, "{what}: one pane alone");
}

fn both_shown(t: &T, what: &str) {
    both_shown_beside(t, 0, what);
}

/// One rig: a click on `SPL 2`'s row puts both of its panes on screen.
#[test]
fn a_meter_row_brings_its_split_panes() {
    let mut t = T::new();
    t.conn(mirror(two_meters()));
    click(&mut t, 5);
    both_shown(&t, "SPL 2 chosen");
}

/// One rig, another meter shown first: the click still brings `SPL 2`'s two panes.
#[test]
fn a_meter_row_brings_its_panes_after_another_meter() {
    let mut t = T::new();
    t.conn(mirror(two_meters()));
    click(&mut t, 4);
    click(&mut t, 5);
    both_shown(&t, "SPL 2 chosen");
}

/// The focused pane alone on screen (W), as it was when the operator chose `SPL 2`: its row
/// brings both of its panes, not the first one alone; W again shows one alone.
#[test]
fn a_meter_row_brings_its_split_panes_out_of_one_pane_alone() {
    let mut t = T::new();
    t.conn(mirror(two_meters()));
    click(&mut t, 4);
    t.key("W");
    assert!(t.st.layout.maximized);
    click(&mut t, 5);
    both_shown(&t, "SPL 2 chosen with one pane alone");
    t.key("W");
    assert!(t.st.layout.maximized, "W still shows one pane alone");
    // The stage view (W twice) stays one, now with every pane.
    t.key("W");
    assert!(t.st.stage_view());
    click(&mut t, 4);
    click(&mut t, 5);
    both_shown(&t, "SPL 2 chosen in the stage view");
    assert!(t.st.stage_view() && t.st.panes_only);
    t.key("W");
    assert!(
        t.st.layout.maximized && t.st.stage_view(),
        "W: its pane alone"
    );
    // A measurement without panes of its own keeps the screen as it is: its meter in the
    // focused one of `SPL 2`'s panes, still alone.
    click(&mut t, 4);
    assert!(t.st.layout.maximized, "FOH SPL has no own panes");
    // `SPL 2` back in that pane makes the screen its layout already: both panes drawn.
    click(&mut t, 5);
    both_shown(&t, "SPL 2 chosen again");
}

/// The second rig, as its hello reply names it.
const PUPU: RigId = RigId(0x9e9e);

/// This computer's rig shown (the test daemon with `FOH SPL`), pupu connected in the
/// background with `SPL 2` split.
fn pupu_behind() -> T {
    let mut t = T::new();
    t.conn(mirror(with_spl()));
    let slot = t.st.add_rig("pupu".into(), None, None);
    to(
        &mut t,
        RigId(u64::from(slot)),
        ConnEvent::Connected {
            target: "pupu".into(),
            server: "ac2d test".into(),
            client_id: ClientId("c2".into()),
            rig_id: PUPU,
            name: "pupu".into(),
        },
    );
    to(&mut t, PUPU, mirror(two_meters()));
    t
}

fn to(t: &mut T, rig: RigId, e: ConnEvent) -> Vec<(RigId, Request)> {
    t.st.update(Msg::Conn(rig, Box::new(e)), &t.keys)
}

/// Pupu shown, this computer's rig behind it: a click on `SPL 2`'s row brings both panes.
#[test]
fn a_meter_row_brings_its_split_panes_on_the_rig_switched_to() {
    let mut t = pupu_behind();
    click(&mut t, 4);
    t.st.update(Msg::RigPick(RigRow::Rig(PUPU)), &t.keys);
    assert_eq!(t.st.rig.id, PUPU);
    click(&mut t, 4);
    click(&mut t, 5);
    // The pane this computer's rig showed stays beside pupu's.
    both_shown_beside(&t, 1, "SPL 2 chosen");
}

/// Pupu shown from the start, another rig behind: the click brings both panes, also out
/// of one pane alone.
#[test]
fn a_meter_row_brings_its_split_panes_with_a_rig_behind() {
    let mut t = pupu_behind();
    t.st.update(Msg::RigPick(RigRow::Rig(PUPU)), &t.keys);
    click(&mut t, 5);
    both_shown(&t, "SPL 2 chosen");
    // Another meter, one pane alone, then `SPL 2` again: both of its panes.
    click(&mut t, 4);
    t.key("W");
    click(&mut t, 5);
    both_shown(&t, "SPL 2 chosen with one pane alone");
}

/// pupu's `SPL 2` as the operator laid it out: its meter over a programme goniometer beside
/// the pair's meters.
fn field_layout(id: u32) -> PaneLayout {
    let spl = match split_spl(id) {
        PaneLayout::Split { a, .. } => a,
        _ => unreachable!(),
    };
    let prog = |programme_view| {
        Box::new(PaneLayout::Pane {
            pane: Box::new(wire::LayoutPane {
                kind: wire::PaneKind::Programme,
                transfer_view: wire::TransferView::Magnitude,
                spectrum_view: wire::SpectrumView::Spectrum,
                spl_view: wire::SplView::MeterLeq,
                sweep_view: wire::SweepView::Response,
                programme_view,
                chrome: wire::PlotChrome::Full,
                content: Some(wire::PaneContent::Pair { left: 0, right: 1 }),
                levels: wire::LayoutLevels::default(),
                plates: wire::LayoutPlates::default(),
                rig: wire::RigRef::ThisRig,
            }),
        })
    };
    PaneLayout::Split {
        axis: wire::SplitAxis::Column,
        ratio: 0.5,
        a: spl,
        b: Box::new(PaneLayout::Split {
            axis: wire::SplitAxis::Row,
            ratio: 0.5,
            a: prog(wire::ProgrammeView::Goniometer),
            b: prog(wire::ProgrammeView::Meters),
        }),
    }
}

/// The field layout chosen with one rig and with pupu shown beside another: all three
/// panes drawn.
#[test]
fn the_field_layout_comes_back_whole() {
    for behind in [false, true] {
        let mut s = two_meters();
        s.measurements.last_mut().expect("SPL 2").layout = Some(field_layout(5));
        let mut t = if behind { pupu_behind() } else { T::new() };
        if behind {
            to(&mut t, PUPU, mirror(s));
            t.st.update(Msg::RigPick(RigRow::Rig(PUPU)), &t.keys);
        } else {
            t.conn(mirror(s));
        }
        click(&mut t, 4);
        t.key("W");
        click(&mut t, 5);
        assert_eq!(t.st.pane_layout(), field_layout(5), "behind {behind}");
        let area = PaneRect::new(0.0, 0.0, 1000.0, 600.0);
        assert_eq!(t.st.pane_rects(area, 0.0).len(), 3, "behind {behind}");
    }
}
