//! A measurement row chosen in the list brings its own panes at once: SPL meters split.
//! One with no panes of its own that no pane shows gets the panes a new one of its kind
//! starts on.

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
            spectrum_camera: ac2_proto::model::layout::SpectrumCamera::DEFAULT,
            spl_view,
            sweep_view: wire::SweepView::Response,
            programme_view: wire::ProgrammeView::Meters,
            chrome: wire::PlotChrome::Full,
            content: wire::PaneContent::Meter {
                rig: wire::RigRef::ThisRig,
                meas: MeasId(id),
            },
            levels: wire::LayoutLevels::default(),
            plates: wire::LayoutPlates::default(),
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
    let shown = split_spl(5);
    assert_eq!(t.st.pane_layout(), shown, "{what}");
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
    // `FOH SPL`'s panes are one: W still holds.
    click(&mut t, 4);
    assert!(t.st.layout.maximized, "FOH SPL has one pane");
    // `SPL 2` back in that pane makes the screen its layout already: both panes drawn.
    click(&mut t, 5);
    both_shown(&t, "SPL 2 chosen again");
}

/// The second rig, as its hello reply names it.
const PUPU: RigId = RigId(0x9e9e);

/// This computer's rig shown (the test daemon with `FOH SPL`), pupu connected in the
/// background with `SPL 2` split.
fn pupu_behind() -> T {
    // The grid's panes left unpicked: a pane showing something would stay pinned to the
    // first rig when pupu is shown.
    let mut t = T::fresh();
    t.st.layout = grid();
    t.st.pane_auto = false;
    t.connect(daemon_state());
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
    t.drop_meas(t.st.layout.focus, MeasId(4));
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
                spectrum_camera: ac2_proto::model::layout::SpectrumCamera::DEFAULT,
                spl_view: wire::SplView::MeterLeq,
                sweep_view: wire::SweepView::Response,
                programme_view,
                chrome: wire::PlotChrome::Full,
                content: wire::PaneContent::Pair {
                    rig: wire::RigRef::ThisRig,
                    left: 0,
                    right: 1,
                },
                levels: wire::LayoutLevels::default(),
                plates: wire::LayoutPlates::default(),
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
        let shown = field_layout(5);
        assert_eq!(t.st.pane_layout(), shown, "behind {behind}");
        let area = PaneRect::new(0.0, 0.0, 1000.0, 600.0);
        assert_eq!(t.st.pane_rects(area, 0.0).len(), 3, "behind {behind}");
    }
}

/// The panes on screen in reading order: kind, transfer view and the measurement shown.
fn screen(t: &T) -> Vec<(PaneKind, TransferView, Option<u32>)> {
    t.st.visible_panes()
        .into_iter()
        .map(|p| {
            let v = t.st.layout.view(p).expect("a pane");
            (v.kind, v.modes.transfer, t.st.pane_meas(p).map(|m| m.id.0))
        })
        .collect()
}

/// The `meas.layout` calls in `r`.
fn stored_on(r: &[Request]) -> Vec<(MeasId, Option<PaneLayout>)> {
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

fn choose(t: &mut T, id: u32) -> Vec<Request> {
    t.st.apply(Msg::SelectMeas(MeasId(id)), &t.keys)
}

/// A desk with no layout stored on any measurement, as a rig's `rig.json` and a salvaged
/// session leave it: the row of a transfer, an SPL meter, a spectrum and a sweep each puts
/// the panes a new one of its kind starts on on screen, showing it, and sends them as its
/// layout once.
#[test]
fn a_row_without_own_panes_brings_its_kinds_first_panes() {
    use TransferView as V;
    let mut t = T::fresh();
    let mut s = with_spl();
    let sweep = sweep_measurement();
    let sweep_id = sweep.id.0;
    s.measurements.push(sweep);
    t.connect(s);
    assert_eq!(
        screen(&t),
        [(PaneKind::Transfer, V::Magnitude, Some(1))],
        "the app's one pane starts on the transfer"
    );
    let cases: [(u32, Vec<(PaneKind, V)>); 4] = [
        (4, vec![(PaneKind::Spl, V::Magnitude)]),
        (
            1,
            vec![
                (PaneKind::Transfer, V::Magnitude),
                (PaneKind::Transfer, V::Phase),
                (PaneKind::Transfer, V::Coherence),
            ],
        ),
        (2, vec![(PaneKind::Spectrum, V::Magnitude)]),
        (sweep_id, vec![(PaneKind::Distortion, V::Magnitude)]),
    ];
    for (id, panes) in &cases {
        let r = choose(&mut t, *id);
        let want: Vec<_> = panes.iter().map(|(k, v)| (*k, *v, Some(*id))).collect();
        assert_eq!(screen(&t), want, "measurement {id}");
        assert_eq!(t.st.rig.selected, Some(MeasId(*id)));
        assert_eq!(
            stored_on(&r),
            [(MeasId(*id), Some(t.st.pane_layout()))],
            "measurement {id}: its first panes are its layout"
        );
    }
    // Each row again: the same panes, from what was sent, and nothing sent twice.
    for (id, panes) in &cases {
        let r = choose(&mut t, *id);
        let want: Vec<_> = panes.iter().map(|(k, v)| (*k, *v, Some(*id))).collect();
        assert_eq!(screen(&t), want, "measurement {id} again");
        assert!(stored_on(&r).is_empty(), "measurement {id} again: {r:?}");
    }
}

/// The child rows only select: a live curve's, a math trace's and a stored trace's row
/// change no pane and store nothing, whatever the screen shows.
#[test]
fn child_rows_bring_no_panes() {
    let mut t = T::fresh();
    let mut s = tree_state();
    s.measurements.push(meas(4, "FOH SPL", spl_meter()));
    t.connect(s);
    choose(&mut t, 4);
    let before = t.st.layout.views.clone();
    for msg in [
        Msg::SelectLive(MeasId(1)),
        Msg::SelectMath(MeasId(6)),
        Msg::SelectTrace(TraceId(3)),
    ] {
        let what = format!("{msg:?}");
        let r = t.st.apply(msg, &t.keys);
        assert_eq!(t.st.layout.views, before, "{what}");
        assert!(stored_on(&r).is_empty(), "{what}: {r:?}");
    }
    // A math trace has no screen of its own to start on: chosen as a measurement's row
    // is, it is selected and the panes stay.
    let r = choose(&mut t, 6);
    assert_eq!(t.st.rig.selected, Some(MeasId(6)));
    assert_eq!(t.st.layout.views, before);
    assert!(stored_on(&r).is_empty(), "{r:?}");
}

/// A row dragged onto a pane changes that pane alone, also for a measurement with no
/// panes of its own.
#[test]
fn a_row_dropped_on_a_pane_changes_that_pane_alone() {
    let mut t = T::new();
    t.conn(mirror(with_spl()));
    let before = screen(&t);
    let p = t.pane(PaneKind::Spectrum);
    t.drop_meas(p, MeasId(4));
    let after = screen(&t);
    assert_eq!(after.len(), before.len());
    let changed: Vec<_> = before.iter().zip(&after).filter(|(a, b)| a != b).collect();
    assert_eq!(
        changed,
        [(
            &(PaneKind::Spectrum, TransferView::Magnitude, Some(2)),
            &(PaneKind::Spl, TransferView::Magnitude, Some(4))
        )]
    );
}

/// Ctrl+Z after a row brought its first panes puts the panes before it back, as after a
/// row that brought stored ones, and stores nothing; redo shows the first panes again.
#[test]
fn undo_takes_back_the_panes_a_row_brought() {
    for stored in [false, true] {
        let mut t = T::new();
        let mut s = two_meters();
        if !stored {
            s.measurements.last_mut().expect("SPL 2").layout = None;
        }
        t.conn(mirror(s));
        let before = screen(&t);
        choose(&mut t, 5);
        let brought = screen(&t);
        assert_ne!(brought, before, "stored {stored}");
        let r = t.key("Ctrl+Z");
        assert_eq!(screen(&t), before, "stored {stored}: undone");
        assert!(stored_on(&r).is_empty(), "stored {stored}: {r:?}");
        assert!(t.last_toast().starts_with("Undid: "), "{}", t.last_toast());
        let r = t.key("Ctrl+Shift+Z");
        assert_eq!(screen(&t), brought, "stored {stored}: redone");
        assert!(stored_on(&r).is_empty(), "stored {stored}: {r:?}");
    }
}

/// This computer's rig shown, pupu in the background: a click on a row of pupu's part of
/// the list shows pupu and brings that measurement's panes — the stored ones of `SPL 2`,
/// and for `FOH SPL`, which has none, an SPL meter's first pane, sent to pupu's daemon.
#[test]
fn another_rigs_row_brings_its_panes() {
    let on_pupu = |id: u32| Msg::OnRig(PUPU, Box::new(Msg::SelectMeas(MeasId(id))));
    let mut t = pupu_behind();
    let r = t.st.update(on_pupu(5), &t.keys);
    assert_eq!(t.st.active(), PUPU);
    both_shown(&t, "pupu's SPL 2 chosen from the other rig");
    assert!(
        r.iter()
            .all(|(_, q)| stored_on(std::slice::from_ref(q)).is_empty())
    );

    let mut t = pupu_behind();
    let r = t.st.update(on_pupu(4), &t.keys);
    assert_eq!(t.st.active(), PUPU);
    assert_eq!(
        screen(&t),
        [(PaneKind::Spl, TransferView::Magnitude, Some(4))]
    );
    let p = t.st.layout.focus;
    assert_eq!(t.st.layout.view(p).map(|v| t.st.pane_rig(v)), Some(PUPU));
    let sent: Vec<_> = r
        .iter()
        .filter(|(_, q)| !stored_on(std::slice::from_ref(q)).is_empty())
        .map(|(rig, q)| (*rig, stored_on(std::slice::from_ref(q))[0].0))
        .collect();
    assert_eq!(sent, [(PUPU, MeasId(4))]);
}
