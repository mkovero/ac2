//! A measurement row chosen in the list brings its own panes at once: SPL meters split.
//! One with no panes of its own that no pane shows gets the panes a new one of its kind
//! starts on. A child row brings the panes of the measurement it is listed under, and a
//! curve none of them draws gets a pane in that layout. The screen is that measurement's
//! alone, whichever rig it is of.

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

/// `SPL 2`'s two panes are the layout, and they are all that is drawn.
fn both_shown(t: &T, what: &str) {
    let shown = split_spl(5);
    assert_eq!(t.st.pane_layout(), shown, "{what}");
    let area = PaneRect::new(0.0, 0.0, 1000.0, 600.0);
    let drawn = t.st.pane_rects(area, 0.0).len();
    assert_eq!(drawn, 2, "{what}: its two panes and no other");
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

/// This computer's rig shown (the test daemon with `FOH SPL`, paired with pupu), pupu
/// connected in the background with `SPL 2` split.
fn pupu_behind() -> T {
    // The grid's panes left unpicked: a pane showing something would stay pinned to the
    // first rig when pupu is shown.
    let mut t = T::fresh();
    t.st.layout = grid();
    t.st.pane_auto = false;
    t.connect(daemon_state());
    t.conn(mirror(paired_with(with_spl(), PUPU, "pupu")));
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
    // The pane this computer's rig showed is not kept beside pupu's.
    both_shown(&t, "SPL 2 chosen");
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

/// Main L's transfer stack, as its row brings it.
fn stack_of_1() -> Vec<(PaneKind, TransferView, Option<u32>)> {
    use TransferView as V;
    [V::Magnitude, V::Phase, V::Coherence]
        .map(|v| (PaneKind::Transfer, v, Some(1)))
        .to_vec()
}

/// `tree_state` with `FOH SPL` (4), and under Main L a spectrum-domain math trace (7) and
/// a stored spectrum trace (8): curves no pane of a transfer's stack draws.
fn tree_with_spectra() -> State {
    let mut s = tree_state();
    s.measurements.push(meas(4, "FOH SPL", spl_meter()));
    let main = TraceOwner::Meas { meas: MeasId(1) };
    s.measurements.push(meas(
        7,
        "pre + post",
        MeasKind::Math {
            config: MathConfig::of(
                main,
                MathDomain::Spectrum,
                MathExpr::Binary {
                    a: Operand::Trace { trace: TraceId(8) },
                    op: MathOp::Divide,
                    b: Operand::Trace { trace: TraceId(8) },
                },
            ),
        },
    ));
    let mut t = stored(8, None, 2);
    t.kind = TraceKind::Spectrum {
        scale: LevelScale::Dbfs,
        level: SpecLevel::Tone,
    };
    t.edit.owner = main;
    s.traces.push(t);
    s
}

/// Every row under a measurement brings that measurement's screen, as its own row does: a
/// live curve's, a math trace's (also reached as a measurement) and a stored trace's, from
/// another measurement's screen. The first one stores the panes on it; from then on the
/// rows bring what is stored and send nothing. A row of the measurement on screen changes
/// no pane, and a row under Imported, which is no measurement's, only selects.
#[test]
fn child_rows_bring_their_measurements_screen() {
    let mut t = T::fresh();
    let mut s = tree_state();
    s.measurements.push(meas(4, "FOH SPL", spl_meter()));
    t.connect(s);
    let spl = [(PaneKind::Spl, TransferView::Magnitude, Some(4))];
    let rows = [
        (Msg::SelectLive(MeasId(1)), Some(1), None),
        (Msg::SelectMath(MeasId(6)), Some(6), None),
        (Msg::SelectMeas(MeasId(6)), Some(6), None),
        (Msg::SelectTrace(TraceId(3)), Some(1), Some(3)),
    ];
    for (i, (msg, selected, trace)) in rows.iter().enumerate() {
        choose(&mut t, 4);
        assert_eq!(screen(&t), spl, "{msg:?}: FOH SPL's screen first");
        let r = t.st.apply(msg.clone(), &t.keys);
        assert_eq!(screen(&t), stack_of_1(), "{msg:?}");
        assert_eq!(t.st.rig.selected, selected.map(MeasId), "{msg:?}");
        assert_eq!(t.st.rig.selected_trace, trace.map(TraceId), "{msg:?}");
        let sent = stored_on(&r);
        if i == 0 {
            assert_eq!(sent, [(MeasId(1), Some(t.st.pane_layout()))], "{msg:?}");
        } else {
            assert!(sent.is_empty(), "{msg:?}: {r:?}");
        }
    }
    // Main L's screen is up: its other rows change nothing.
    let before = t.st.layout.clone();
    for msg in [
        Msg::SelectLive(MeasId(1)),
        Msg::SelectTrace(TraceId(4)),
        Msg::SelectMath(MeasId(6)),
        Msg::SelectTrace(TraceId(5)),
    ] {
        let what = format!("{msg:?}");
        let r = t.st.apply(msg, &t.keys);
        assert_eq!(t.st.layout, before, "{what}");
        assert!(stored_on(&r).is_empty(), "{what}: {r:?}");
    }
    // The imported trace from another measurement's screen: selected, the screen stays.
    choose(&mut t, 4);
    t.st.apply(Msg::SelectTrace(TraceId(5)), &t.keys);
    assert_eq!(t.st.rig.selected_trace, Some(TraceId(5)));
    assert_eq!(t.st.rig.selected, Some(MeasId(4)));
    assert_eq!(screen(&t), spl);
}

/// A second click on the selected trace deselects it and changes no pane.
#[test]
fn a_second_click_on_a_trace_deselects_and_keeps_the_screen() {
    let mut t = T::fresh();
    let mut s = tree_state();
    s.measurements.push(meas(4, "FOH SPL", spl_meter()));
    t.connect(s);
    choose(&mut t, 4);
    t.st.apply(Msg::SelectTrace(TraceId(3)), &t.keys);
    let before = t.st.layout.clone();
    let r = t.st.apply(Msg::SelectTrace(TraceId(3)), &t.keys);
    assert_eq!(t.st.rig.selected_trace, None);
    assert_eq!(t.st.rig.selected, Some(MeasId(1)));
    assert_eq!(t.st.layout, before);
    assert!(stored_on(&r).is_empty(), "{r:?}");
}

/// A curve no pane of its measurement's screen draws — a spectrum-domain math trace or a
/// stored spectrum under a transfer — gets a spectrum pane in that measurement's layout,
/// stored there in one call, and the pane then draws the measurement's other spectrum
/// curves too. No other measurement's layout is sent or changed.
#[test]
fn a_curve_without_a_pane_gets_one_in_its_measurements_layout() {
    use TransferView as V;
    let math = (Msg::SelectMath(MeasId(7)), Some(7));
    let trace = (Msg::SelectTrace(TraceId(8)), None);
    for (first, second) in [(&math, &trace), (&trace, &math)] {
        let mut t = T::fresh();
        t.connect(tree_with_spectra());
        let spl = choose(&mut t, 4);
        assert_eq!(stored_on(&spl).len(), 1, "FOH SPL's first pane");
        let r = t.st.apply(first.0.clone(), &t.keys);
        let mut want = stack_of_1();
        want.push((PaneKind::Spectrum, V::Magnitude, first.1));
        assert_eq!(screen(&t), want, "{:?}", first.0);
        assert_eq!(
            stored_on(&r),
            [(MeasId(1), Some(t.st.pane_layout()))],
            "{:?}: one call, Main L's",
            first.0
        );
        let added = t.st.layout.focus;
        assert_eq!(t.st.layout.kind(added), PaneKind::Spectrum);
        // The stored spectrum alone leads no pane: it is the pane's one entry.
        if first.1.is_none() {
            let p = t.st.pane_layout();
            assert_eq!(
                p.panes().last().map(|p| p.content.clone()),
                Some(wire::PaneContent::Curves {
                    entries: vec![wire::PaneEntry {
                        rig: wire::RigRef::ThisRig,
                        what: wire::EntryRef::Trace { trace: TraceId(8) },
                    }],
                })
            );
        }
        // The other spectrum curve of Main L: the math trace's pane draws its group; the
        // stored trace's pane holds that trace alone, so the math trace gets its own.
        let before = t.st.layout.clone();
        let r = t.st.apply(second.0.clone(), &t.keys);
        if first.1.is_some() {
            assert_eq!(t.st.layout, before, "{:?}", second.0);
            assert!(stored_on(&r).is_empty(), "{r:?}");
        } else {
            assert_eq!(screen(&t).len(), 5, "{:?}", second.0);
            // Held behind the call in flight: nothing goes to another measurement.
            assert!(stored_on(&r).iter().all(|(m, _)| *m == MeasId(1)), "{r:?}");
        }
        // FOH SPL's screen is its own still.
        let r = choose(&mut t, 4);
        assert_eq!(screen(&t), [(PaneKind::Spl, V::Magnitude, Some(4))]);
        assert!(stored_on(&r).is_empty(), "{r:?}");
    }
}

/// Panes nobody's that happen to show the measurement (the remembered grid) are not
/// grown by a curve's pane: the row brings the measurement's first screen and the pane
/// is added there. A hidden curve gets its pane and stays hidden.
#[test]
fn a_curves_pane_is_never_added_to_panes_that_were_open() {
    let mut t = T::new();
    t.conn(mirror(tree_with_spectra()));
    t.st.apply(Msg::ToggleMeasShown(MeasId(7)), &t.keys);
    assert_eq!(t.st.visible_panes().len(), 4, "the grid");
    assert_eq!(t.st.rig.choice.own_panes, None);
    let r = t.st.apply(Msg::SelectMath(MeasId(7)), &t.keys);
    let mut want = stack_of_1();
    want.push((PaneKind::Spectrum, TransferView::Magnitude, Some(7)));
    assert_eq!(screen(&t), want);
    assert_eq!(stored_on(&r), [(MeasId(1), Some(t.st.pane_layout()))]);
    let hidden = t.st.meas(MeasId(7)).expect("the math trace");
    assert!(t.st.meas_hidden(hidden), "not unhidden");
}

/// A measurement with no panes of its own chosen while another measurement's screen is
/// open — its pane with an Inputs pane and a guest pane showing the chosen one: only its
/// own first pane is on screen, nothing of the other's beside it, and the other
/// measurement's layout is not sent again and comes back whole.
#[test]
fn a_row_without_own_panes_takes_nothing_of_anothers_screen() {
    let mut t = T::fresh();
    t.connect(with_spl());
    choose(&mut t, 1);
    t.key("N");
    t.drag_drop(DragItem::Input(0), t.st.layout.focus, None);
    t.key("N");
    t.drop_meas(t.st.layout.focus, MeasId(4));
    let mains = t.st.pane_layout();
    assert_eq!(mains.panes().len(), 3, "its pane, the inputs, the guest");
    assert_eq!(t.st.rig.choice.own_panes, Some(MeasId(1)));
    for (id, kind) in [(2, PaneKind::Spectrum), (4, PaneKind::Spl)] {
        let r = choose(&mut t, id);
        assert_eq!(
            screen(&t),
            [(kind, TransferView::Magnitude, Some(id))],
            "measurement {id}"
        );
        assert_eq!(
            stored_on(&r),
            [(MeasId(id), Some(t.st.pane_layout()))],
            "measurement {id}: its own pane, and nothing for Main L"
        );
        let r = choose(&mut t, 1);
        assert_eq!(t.st.pane_layout(), mains, "Main L's screen back whole");
        assert!(stored_on(&r).is_empty(), "{r:?}");
    }
}

/// Tab down a list of two measurements with two stored traces each (eight rows: header,
/// live curve, two traces, twice) changes the screen once per measurement, on the step
/// onto its header, and never between rows of one measurement; once each has its panes
/// stored, stepping sends nothing.
#[test]
fn tab_changes_the_screen_once_per_measurement() {
    let mut s = daemon_state();
    let trace = |id: u32, meas: u32| {
        let mut t = stored(id, None, 2);
        if meas == 2 {
            t.kind = TraceKind::Spectrum {
                scale: LevelScale::Dbfs,
                level: SpecLevel::Tone,
            };
        }
        t.edit.owner = TraceOwner::Meas { meas: MeasId(meas) };
        t
    };
    s.traces = vec![trace(11, 1), trace(12, 1), trace(13, 2), trace(14, 2)];
    let mut t = T::fresh();
    t.connect(s);
    let stops = 8;
    // The first round stores each measurement's first panes.
    for _ in 0..stops {
        t.key("Tab");
    }
    let mut changes = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..2 * stops {
        let before = (t.st.pane_layout(), t.st.rig.selected);
        let r = t.key("Tab");
        assert!(stored_on(&r).is_empty(), "{r:?}");
        seen.insert((
            t.st.rig.selected,
            t.st.rig.selected_trace,
            t.st.live_row_selected(),
        ));
        if t.st.pane_layout() != before.0 {
            assert_ne!(t.st.rig.selected, before.1, "within one measurement");
            assert_eq!(t.st.rig.selected_trace, None, "onto a header");
            assert!(!t.st.live_row_selected(), "onto a header");
            changes.push(t.st.rig.selected);
        }
    }
    assert_eq!(seen.len(), stops, "every row is a stop: {seen:?}");
    assert_eq!(
        changes.len(),
        4,
        "two rounds, two measurements: {changes:?}"
    );
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

/// A child row that brought its measurement's screen is one step: Ctrl+Z puts the panes
/// before it back, redo the screen again.
#[test]
fn undo_takes_back_the_screen_a_child_row_brought() {
    let mut t = T::fresh();
    t.connect(tree_with_spectra());
    choose(&mut t, 4);
    let before = screen(&t);
    for msg in [Msg::SelectTrace(TraceId(3)), Msg::SelectMath(MeasId(7))] {
        choose(&mut t, 4);
        t.st.apply(msg.clone(), &t.keys);
        let brought = screen(&t);
        assert_ne!(brought, before, "{msg:?}");
        t.key("Ctrl+Z");
        assert_eq!(screen(&t), before, "{msg:?}: undone in one step");
        assert!(t.last_toast().starts_with("Undid: "), "{}", t.last_toast());
        t.key("Ctrl+Shift+Z");
        assert_eq!(screen(&t), brought, "{msg:?}: redone");
    }
}

/// This computer's rig shown with a pane showing its meter, pupu in the background: a
/// click on a row of pupu's part of the list shows pupu and puts that measurement's
/// screen up alone — the stored panes of `SPL 2`, and for `FOH SPL`, which has none, an
/// SPL meter's first pane, sent to pupu's daemon. The first rig's pane is not kept beside
/// either: rigs are compared by a row dragged onto a pane.
#[test]
fn another_rigs_row_brings_its_screen_alone() {
    let on_pupu = |id: u32| Msg::OnRig(PUPU, Box::new(Msg::SelectMeas(MeasId(id))));
    let start = || {
        let mut t = pupu_behind();
        click(&mut t, 4);
        t.drop_meas(t.st.layout.focus, MeasId(4));
        t
    };
    let mut t = start();
    let r = t.st.update(on_pupu(5), &t.keys);
    assert_eq!(t.st.active(), PUPU);
    both_shown(&t, "pupu's SPL 2 chosen from the other rig");
    assert!(
        r.iter()
            .all(|(_, q)| stored_on(std::slice::from_ref(q)).is_empty())
    );

    let mut t = start();
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

    // A child row of pupu's list: the same screen.
    let mut t = start();
    let live = Msg::OnRig(PUPU, Box::new(Msg::SelectLive(MeasId(2))));
    t.st.update(live, &t.keys);
    assert_eq!(t.st.active(), PUPU);
    assert_eq!(
        screen(&t),
        [(PaneKind::Spectrum, TransferView::Magnitude, Some(2))]
    );
}
