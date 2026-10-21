//! Reducer tests of a measurement's own panes (`meas.layout`).

use super::*;
use ac2_proto::model::PaneLayout;
use ac2_proto::model::layout as wire;

/// Magnitude over phase, the way an operator lays out a transfer measurement.
fn two_transfer() -> PaneLayout {
    let pane = |transfer_view| PaneLayout::Pane {
        pane: Box::new(wire::LayoutPane {
            kind: wire::PaneKind::Transfer,
            transfer_view,
            spectrum_view: wire::SpectrumView::Spectrum,
            spl_view: wire::SplView::MeterLeq,
            sweep_view: wire::SweepView::Response,
            chrome: wire::PlotChrome::Full,
            meas: None,
            levels: wire::LayoutLevels::default(),
            plates: wire::LayoutPlates::default(),
        }),
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

/// Edits made faster than the daemon answers: one call in flight, the newest edit sent when
/// it is answered (two in flight could arrive in either order). The daemon's copies of
/// this app's earlier layouts, arriving after the replies, leave the screen as it is.
#[test]
fn quick_edits_go_one_call_at_a_time() {
    let mut t = T::new();
    select(&mut t, 1);
    let n = t.st.layout.panes().len();
    let r = t.key("N");
    let first = layout_calls(&r);
    assert_eq!(first.len(), 1, "{r:?}");
    let r = t.key("N");
    assert!(layout_calls(&r).is_empty(), "held behind the first: {r:?}");
    let r = t.key("N");
    assert!(layout_calls(&r).is_empty(), "held behind the first: {r:?}");
    let screen = t.st.pane_layout();
    assert_eq!(screen.panes().len(), n + 3);
    let r = t.conn(ConnEvent::Reply {
        what: format!("Main L: {}", ac2_scene::format::own_panes(n + 1)),
        result: Ok(()),
    });
    let second = layout_calls(&r);
    assert_eq!(
        second,
        vec![(MeasId(1), Some(screen.clone()))],
        "the newest"
    );
    let r = t.conn(ConnEvent::Reply {
        what: format!("Main L: {}", ac2_scene::format::own_panes(n + 3)),
        result: Ok(()),
    });
    assert!(layout_calls(&r).is_empty(), "{r:?}");
    // Both answered; the state events follow.
    t.conn(mirror(with_layout(1, first[0].1.clone())));
    assert_eq!(
        t.st.pane_layout(),
        screen,
        "an older layout of this app's own"
    );
    t.conn(mirror(with_layout(1, Some(screen.clone()))));
    assert_eq!(t.st.pane_layout(), screen);
    // Then another client's edit is shown.
    t.conn(mirror(with_layout(1, Some(two_transfer()))));
    assert_eq!(t.st.pane_layout(), two_transfer());
}

/// The rig with `Spectrum 2` (4) beside `Spectrum 1` (3), both laid out as
/// `three_spectrum`, and a stored spectrum trace under each (13 under `Spectrum 1`, 14 under
/// `Spectrum 2`); `Spectrum 1`'s row chosen.
fn desk_on_twin_spectra() -> T {
    let mut s = rig();
    s.measurements[2].layout = Some(three_spectrum());
    let mut m = meas(4, "Spectrum 2", spectrum());
    m.layout = Some(three_spectrum());
    s.measurements.push(m);
    let trace = |id: u32, meas: u32| {
        let mut t = stored(id, None, 2);
        t.kind = TraceKind::Spectrum {
            scale: LevelScale::Dbfs,
            level: ac2_proto::model::SpecLevel::Tone,
        };
        t.edit.owner = TraceOwner::Meas { meas: MeasId(meas) };
        t
    };
    s.traces = vec![trace(13, 3), trace(14, 4)];
    let mut t = T::fresh();
    t.connect(s);
    select(&mut t, 3);
    assert_eq!(
        t.st.pane_layout(),
        three_spectrum(),
        "Spectrum 1's own panes"
    );
    t
}

/// `Spectrum 2`'s layout as the daemon has it.
fn spectrum_2_layout(t: &T) -> Option<PaneLayout> {
    t.st.meas(MeasId(4)).and_then(|m| m.layout.clone())
}

/// The operator's sequence: `Spectrum 1`'s row chosen, C on one of its traces, then a click
/// on a `Spectrum 2` trace. Two layouts alike are still two: nothing is sent for
/// `Spectrum 2`, and a child row moves no pane.
#[test]
fn a_trace_compared_then_another_measurements_trace_chosen_leaves_its_layout() {
    let mut t = desk_on_twin_spectra();
    let mut sent = Vec::new();
    sent.extend(t.st.update(Msg::SelectTrace(TraceId(13)), &t.keys));
    sent.extend(t.key("C"));
    answer_layout_calls(&mut t, &sent);
    let before = t.st.pane_layout();
    let r = t.st.update(Msg::SelectTrace(TraceId(14)), &t.keys);
    assert_eq!(t.st.selected_trace, Some(TraceId(14)));
    assert!(layout_calls(&r).is_empty(), "a trace row: {r:?}");
    assert_eq!(t.st.pane_layout(), before, "a child row moves no pane");
    sent.extend(r);
    assert!(
        !layout_targets(&sent).contains(&MeasId(4)),
        "nothing for Spectrum 2: {sent:?}"
    );
    assert_eq!(spectrum_2_layout(&t), Some(three_spectrum()));
    select(&mut t, 4);
    assert_eq!(
        t.st.pane_layout(),
        three_spectrum(),
        "Spectrum 2's own panes"
    );
}

/// A level zoom, an arrangement edit and a chip pick in `Spectrum 1`'s panes each go to
/// `Spectrum 1` alone, `Spectrum 2`'s identical layout untouched.
#[test]
fn edits_of_one_measurements_panes_leave_an_identical_layout_alone() {
    let mut t = desk_on_twin_spectra();
    let graph = t.st.layout.root.reading_order()[1];
    type Edit = fn(&mut T, PaneId) -> Vec<Request>;
    let edits: [(&str, Edit); 3] = [
        ("zoom", |t, _| t.key("Ctrl+I")),
        ("G", |t, _| t.key("G")),
        ("pick", |t, p| {
            t.st.update(Msg::PanePick(p, PaneMenuRow::Meas(MeasId(4))), &t.keys)
        }),
    ];
    for (what, edit) in edits {
        let r = edit(&mut t, graph);
        assert_eq!(layout_targets(&r), [MeasId(3)], "{what}: {r:?}");
        answer_layout_calls(&mut t, &r);
    }
    assert_eq!(spectrum_2_layout(&t), Some(three_spectrum()));
    assert_eq!(
        t.st.selected,
        Some(MeasId(3)),
        "the selection stays on the row"
    );
}

/// Another client's edit of the layout keeps each pane's pick, as the layout carries it; a
/// layout with no picks has its panes follow the chosen measurement.
#[test]
fn another_clients_layout_carries_the_picks() {
    let three = |layout: Option<PaneLayout>| {
        let mut s = with_layout(2, layout);
        s.measurements.push(meas(3, "Sub 2", spectrum()));
        s
    };
    let mut t = T::new();
    t.conn(mirror(three(None)));
    select(&mut t, 2);
    t.key("N");
    // Sub on both halves: the new one picked for Sub 2.
    let b = t.st.layout.focus;
    t.st.layout.view_mut(b).expect("pane").meas = Some(MeasId(3));
    let picks = |t: &T| -> Vec<Option<MeasId>> {
        t.st.layout
            .root
            .reading_order()
            .iter()
            .filter(|p| t.st.layout.kind(**p) == PaneKind::Spectrum)
            .map(|p| t.st.pane_meas(*p).map(|m| m.id))
            .collect()
    };
    let before = picks(&t);
    // (The grid's own spectrum pane follows the selection too.)
    assert_eq!(before, [Some(MeasId(2)), Some(MeasId(3)), Some(MeasId(2))]);
    t.conn(ConnEvent::Reply {
        what: "Sub: 5 own panes".into(),
        result: Ok(()),
    });
    // Another client steps a pane's chrome.
    fn first_pane(l: &mut PaneLayout) -> &mut wire::LayoutPane {
        match l {
            PaneLayout::Pane { pane } => pane,
            PaneLayout::Split { a, .. } => first_pane(a),
        }
    }
    let mut l = t.st.pane_layout();
    first_pane(&mut l).chrome = wire::PlotChrome::NoGrid;
    t.conn(mirror(three(Some(l.clone()))));
    assert_eq!(t.st.pane_layout(), l, "another client's edit");
    assert_eq!(picks(&t), before, "each pane keeps its pick");
    // No picks: the panes follow the chosen measurement.
    let mut one = two_transfer();
    if let PaneLayout::Split { b, .. } = &mut one
        && let PaneLayout::Pane { pane } = b.as_mut()
    {
        pane.kind = wire::PaneKind::Spectrum;
    }
    t.conn(mirror(three(Some(one.clone()))));
    assert_eq!(t.st.pane_layout(), one);
    assert_eq!(picks(&t), [Some(MeasId(2))]);
}

/// Magnitude, phase and coherence in three columns: `TF 1`'s own panes on the rig.
fn three_transfer() -> PaneLayout {
    let pane = |transfer_view| PaneLayout::Pane {
        pane: Box::new(wire::LayoutPane {
            kind: wire::PaneKind::Transfer,
            transfer_view,
            spectrum_view: wire::SpectrumView::Spectrograph,
            spl_view: wire::SplView::Leq,
            sweep_view: wire::SweepView::Response,
            chrome: wire::PlotChrome::TracesOnly,
            meas: None,
            levels: wire::LayoutLevels::default(),
            plates: wire::LayoutPlates::default(),
        }),
    };
    PaneLayout::Split {
        axis: wire::SplitAxis::Column,
        ratio: 0.5,
        a: Box::new(pane(wire::TransferView::Magnitude)),
        b: Box::new(PaneLayout::Split {
            axis: wire::SplitAxis::Column,
            ratio: 0.5,
            a: Box::new(pane(wire::TransferView::Phase)),
            b: Box::new(pane(wire::TransferView::Coherence)),
        }),
    }
}

/// The rig's daemon: the SPL meter `FOH SPL` first, `TF 1` with three panes of its own, a
/// spectrum.
fn rig() -> State {
    let mut s = daemon_state();
    s.measurements = vec![
        meas(1, "FOH SPL", spl_meter()),
        meas(2, "TF 1", transfer()),
        meas(3, "Spectrum 1", spectrum()),
    ];
    s.measurements[1].layout = Some(three_transfer());
    s
}

/// The rig with `FOH SPL` laid out as `layout`.
fn rig_with_spl_layout(layout: PaneLayout) -> State {
    let mut s = rig();
    s.measurements[0].layout = Some(layout);
    s
}

/// The Pi kiosk as its `ui.toml` starts it: one SPL pane in its bands view, maximised and
/// full screen, remembered on `meas` (or on nothing), connected to the rig.
fn kiosk(meas: Option<&str>) -> T {
    let mut t = T::fresh();
    let mut toml = String::from(
        "[layout]\nfocus = 1\nmaximized = true\nfullscreen = true\n\n\
         [layout.tree]\npane = 1\n\n\
         [[layout.panes]]\nid = 1\nkind = \"spl\"\nspl_view = \"bands\"\n",
    );
    if let Some(m) = meas {
        toml.push_str(&format!("measurement = \"{m}\"\n"));
    }
    let prefs = crate::prefs::UiPrefs::from_toml(&toml).expect("the kiosk's ui.toml");
    t.st.set_prefs(prefs);
    t.connect(rig());
    t
}

/// The kiosk's one SPL pane, showing `FOH SPL`.
fn kiosk_on_its_meter(t: &T, what: &str) {
    let panes = t.st.layout.panes();
    assert_eq!(panes.len(), 1, "{what}: {:?}", t.st.pane_layout());
    assert_eq!(t.st.layout.kind(panes[0]), PaneKind::Spl, "{what}");
    assert_eq!(
        t.st.pane_meas(panes[0]).map(|m| m.config.name.as_str()),
        Some("FOH SPL"),
        "{what}"
    );
    let saved = t.st.layout_prefs().panes.expect("panes saved");
    assert_eq!(saved.views.len(), 1, "{what}: saves {saved:?}");
    assert_eq!(saved.views[0].kind, PaneKind::Spl, "{what}");
}

/// The Pi kiosk shows `FOH SPL` and nobody touches it. Another client laying out
/// `FOH SPL`'s own panes, a split then a view stepped, as the desk app does, leaves the
/// kiosk's one SPL pane as it is: the kiosk selected `FOH SPL` by itself (its remembered
/// pane shows it), and only a measurement's row chosen here brings its panes.
#[test]
fn a_viewer_keeps_its_pane_when_another_client_lays_out_its_measurement() {
    let mut t = kiosk(Some("FOH SPL"));
    kiosk_on_its_meter(&t, "connected");
    assert_eq!(t.st.selected, Some(MeasId(1)));
    let mut phase = three_transfer();
    if let PaneLayout::Split { b, .. } = &mut phase
        && let PaneLayout::Split { b, .. } = b.as_mut()
        && let PaneLayout::Pane { pane } = b.as_mut()
    {
        pane.transfer_view = wire::TransferView::Phase;
    }
    for l in [phase, three_transfer()] {
        let r = t.conn(mirror(rig_with_spl_layout(l)));
        assert!(layout_calls(&r).is_empty(), "{r:?}");
        kiosk_on_its_meter(&t, "FOH SPL laid out elsewhere");
    }
    // TF 1's own panes changed elsewhere too: nothing to do with the kiosk.
    let mut s = rig_with_spl_layout(three_transfer());
    s.measurements[1].layout = Some(two_transfer());
    t.conn(mirror(s));
    kiosk_on_its_meter(&t, "TF 1 laid out elsewhere");
}

/// G on the kiosk (its touch panel) steps the SPL pane's view and nothing else: the
/// measurement it selected by itself (the remembered pane's, or the stand-in `TF 1` when no
/// pane remembers one) is not given the kiosk's pane as its own panes.
#[test]
fn g_on_a_viewer_gives_no_measurement_its_pane() {
    for remembered in [Some("FOH SPL"), None] {
        let mut t = kiosk(remembered);
        let before = t.st.pane_layout();
        for _ in 0..3 {
            let r = t.key("G");
            assert!(layout_calls(&r).is_empty(), "{remembered:?}: {r:?}");
        }
        assert_ne!(t.st.pane_layout(), before, "the view stepped");
        kiosk_on_its_meter(&t, "after G");
        // TF 1 laid out elsewhere afterwards still leaves the kiosk as it is.
        let mut s = rig();
        s.measurements[1].layout = Some(two_transfer());
        t.conn(mirror(s));
        kiosk_on_its_meter(&t, "TF 1 laid out elsewhere after G");
    }
}

/// A click in a pane that shows another measurement selects nothing, and another client's
/// layout for that measurement stays off this screen. Its row chosen here, the screen
/// follows its layout.
#[test]
fn a_layout_received_for_a_measurement_picked_by_its_pane_waits_for_its_row() {
    let mut t = T::new();
    select(&mut t, 1);
    let sub = t.pane(PaneKind::Spectrum);
    t.st.update(Msg::FocusPane(sub), &t.keys);
    assert_eq!(t.st.selected, Some(MeasId(1)), "a click selects nothing");
    let grid = t.st.pane_layout();
    t.conn(mirror(with_layout(2, Some(two_transfer()))));
    assert_eq!(t.st.pane_layout(), grid, "Sub only shown by its pane");
    select(&mut t, 2);
    assert_eq!(t.st.pane_layout(), two_transfer(), "Sub's row chosen");
    let mut other = two_transfer();
    if let PaneLayout::Split { ratio, .. } = &mut other {
        *ratio = 0.25;
    }
    t.conn(mirror(with_layout(2, Some(other.clone()))));
    assert_eq!(
        t.st.pane_layout(),
        other,
        "followed once its own panes are on screen"
    );
}

/// The measurement whose row the operator chose goes for a while (a daemon restarted, a
/// session salvaged) and comes back under its name: its panes still follow its layout.
#[test]
fn a_chosen_measurement_back_after_a_restart_still_follows_its_layout() {
    let mut t = T::new();
    t.conn(mirror(with_layout(1, Some(two_transfer()))));
    select(&mut t, 1);
    assert_eq!(t.st.pane_layout(), two_transfer());
    // Another daemon: its ids are its own, the measurements come back by name.
    t.conn(mirror_of(
        with_layout(1, Some(two_transfer())),
        2,
        Some("c1"),
    ));
    assert_eq!(t.st.selected, Some(MeasId(1)));
    let mut other = two_transfer();
    if let PaneLayout::Split { ratio, .. } = &mut other {
        *ratio = 0.25;
    }
    t.conn(mirror_of(
        with_layout(1, Some(other.clone())),
        2,
        Some("c1"),
    ));
    assert_eq!(t.st.pane_layout(), other);
}

/// The rig's `TF 1` with its three transfer panes on screen, its row chosen.
fn desk_on_tf1() -> T {
    let mut t = T::fresh();
    t.connect(rig());
    select(&mut t, 2);
    assert_eq!(t.st.pane_layout(), three_transfer(), "TF 1's own panes");
    t
}

/// The measurements `r` sends a layout to.
fn layout_targets(r: &[Request]) -> Vec<MeasId> {
    layout_calls(r).into_iter().map(|(m, _)| m).collect()
}

/// Spectrum, spectrograph and spectrum in three columns: `Spectrum 2`'s own panes on the
/// rig.
fn three_spectrum() -> PaneLayout {
    let pane = |spectrum_view| PaneLayout::Pane {
        pane: Box::new(wire::LayoutPane {
            kind: wire::PaneKind::Spectrum,
            transfer_view: wire::TransferView::Coherence,
            spectrum_view,
            spl_view: wire::SplView::Leq,
            sweep_view: wire::SweepView::Response,
            chrome: wire::PlotChrome::TracesOnly,
            meas: None,
            levels: wire::LayoutLevels::default(),
            plates: wire::LayoutPlates::default(),
        }),
    };
    PaneLayout::Split {
        axis: wire::SplitAxis::Column,
        ratio: 0.5,
        a: Box::new(pane(wire::SpectrumView::Spectrum)),
        b: Box::new(PaneLayout::Split {
            axis: wire::SplitAxis::Column,
            ratio: 0.5,
            a: Box::new(pane(wire::SpectrumView::Spectrograph)),
            b: Box::new(pane(wire::SpectrumView::Spectrum)),
        }),
    }
}

/// The rig with `Spectrum 2` and its three spectrum panes, its row chosen on the desk.
fn desk_on_spectrum_2() -> T {
    let mut s = rig();
    let mut m = meas(4, "Spectrum 2", spectrum());
    m.layout = Some(three_spectrum());
    s.measurements.push(m);
    let mut t = T::fresh();
    t.connect(s);
    select(&mut t, 4);
    assert_eq!(
        t.st.pane_layout(),
        three_spectrum(),
        "Spectrum 2's own panes"
    );
    t
}

/// Every `meas.layout` call in `r` answered: the next edit goes out at once.
fn answer_layout_calls(t: &mut T, r: &[Request]) {
    for r in r {
        if let Request::Call {
            cmd: Command::MeasLayout { .. },
            what,
        } = r
        {
            t.conn(ConnEvent::Reply {
                what: what.clone(),
                result: Ok(()),
            });
        }
    }
}

/// The operator's sequence: `Spectrum 2`'s own panes on screen, the lower pane focused (a
/// click inside it), `FOH SPL` picked on that pane's title chip. The pick turns the pane
/// into an SPL pane, the selection staying on `Spectrum 2`'s row: an edit of `Spectrum 2`'s
/// panes, stored on it, as are the edits after it (G, N, T), and never `FOH SPL`'s own
/// layout.
#[test]
fn a_chip_pick_into_another_measurements_panes_leaves_its_own_layout() {
    let mut t = desk_on_spectrum_2();
    let lower = *t.st.layout.root.reading_order().last().expect("a pane");
    let r = t.st.update(Msg::FocusPane(lower), &t.keys);
    assert!(
        layout_calls(&r).is_empty(),
        "a click moves the focus: {r:?}"
    );
    let r =
        t.st.update(Msg::PanePick(lower, PaneMenuRow::Meas(MeasId(1))), &t.keys);
    assert_eq!(
        t.st.selected,
        Some(MeasId(4)),
        "the selection stays on the row"
    );
    assert_eq!(t.st.layout.kind(lower), PaneKind::Spl);
    assert_eq!(layout_targets(&r), [MeasId(4)], "the pick: {r:?}");
    answer_layout_calls(&mut t, &r);
    for k in ["G", "N", "T"] {
        let r = t.key(k);
        let to = layout_targets(&r);
        assert_eq!(to, [MeasId(4)], "{k} is Spectrum 2's edit");
        answer_layout_calls(&mut t, &r);
    }
}

/// The same with a click inside the pane: `FOH SPL` put into the lower pane, the focus
/// moved away and back by clicks (which select nothing), then G.
#[test]
fn a_click_on_a_guest_pane_then_an_edit_leaves_its_own_layout() {
    let mut t = desk_on_spectrum_2();
    let order = t.st.layout.root.reading_order();
    let lower = *order.last().expect("a pane");
    let r =
        t.st.update(Msg::PanePick(lower, PaneMenuRow::Meas(MeasId(1))), &t.keys);
    assert_eq!(layout_targets(&r), [MeasId(4)], "the pick: {r:?}");
    answer_layout_calls(&mut t, &r);
    t.st.update(Msg::FocusPane(order[0]), &t.keys);
    assert_eq!(t.st.selected, Some(MeasId(4)));
    t.st.update(Msg::FocusPane(lower), &t.keys);
    assert_eq!(t.st.selected, Some(MeasId(4)), "a click selects nothing");
    let r = t.key("G");
    assert_eq!(layout_targets(&r), [MeasId(4)], "{r:?}");
}

/// `FOH SPL`'s row chosen (it has no panes of its own: the focused pane turns into an SPL
/// pane), then that pane closed: no pane shows `FOH SPL` any more, so an edit is not its
/// layout — nothing is sent, and its own panes have ended.
#[test]
fn an_edit_with_no_pane_showing_the_measurement_is_not_its_layout() {
    let mut t = desk_on_tf1();
    select(&mut t, 1);
    assert_eq!(t.st.layout.focus_kind(), PaneKind::Spl);
    let r = t.key("Q");
    assert!(layout_calls(&r).is_empty(), "Q: {r:?}");
    assert!(
        !t.st
            .layout
            .panes()
            .iter()
            .any(|p| t.st.layout.kind(*p) == PaneKind::Spl),
        "the SPL pane closed"
    );
    for k in ["G", "N"] {
        let r = t.key(k);
        assert!(layout_calls(&r).is_empty(), "{k}: {r:?}");
    }
}

/// `FOH SPL`'s own layout already the same as `TF 1`'s (an earlier overwrite): an edit of
/// `TF 1`'s panes goes to `TF 1` alone, since none of them shows `FOH SPL`.
#[test]
fn panes_shared_with_a_measurement_none_of_them_shows_are_not_its_layout() {
    let mut t = T::fresh();
    t.connect(rig_with_spl_layout(three_transfer()));
    select(&mut t, 2);
    let r = t.key("G");
    assert_eq!(layout_targets(&r), [MeasId(2)], "{r:?}");
}

/// What each pane shows, in reading order.
fn shown(t: &T) -> Vec<Option<MeasId>> {
    t.st.layout
        .root
        .reading_order()
        .iter()
        .map(|p| t.st.pane_meas(*p).map(|m| m.id))
        .collect()
}

/// `Spectrum 2`'s own panes, `Spectrum 1` put into its spectrograph by `put`; then
/// `Spectrum 1`'s row and `Spectrum 2`'s again: the spectrograph still shows `Spectrum 1`,
/// and `Spectrum 1` (which has no panes of its own) was never given these.
fn guest_in_spectrograph_survives_the_rows(put: fn(&mut T, PaneId) -> Vec<Request>) {
    let mut t = desk_on_spectrum_2();
    let graph = t.st.layout.root.reading_order()[1];
    let s1 = MeasId(3);
    let s2 = MeasId(4);
    let r = put(&mut t, graph);
    assert!(!layout_targets(&r).contains(&s1), "the pick: {r:?}");
    assert_eq!(t.st.pane_meas(graph).map(|m| m.id), Some(s1));
    let r = select(&mut t, 3);
    assert!(layout_calls(&r).is_empty(), "Spectrum 1's row: {r:?}");
    let r = select(&mut t, 4);
    assert!(layout_calls(&r).is_empty(), "Spectrum 2's row: {r:?}");
    assert_eq!(t.st.selected, Some(s2));
    let mut picked = three_spectrum();
    picked.panes_mut()[1].meas = Some(s1);
    assert_eq!(
        t.st.pane_layout(),
        picked,
        "Spectrum 2's own panes, with the pick"
    );
    assert_eq!(
        shown(&t),
        [Some(s2), Some(s1), Some(s2)],
        "the spectrograph keeps the pick"
    );
    let at = t.st.layout.root.reading_order()[1];
    assert_eq!(t.st.layout.focus, at, "the focus stays on the spectrograph");
    assert!(t.st.meas(s1).expect("Spectrum 1").layout.is_none());
}

/// The operator's sequence: a chip pick on the spectrograph of `Spectrum 2`'s own panes.
#[test]
fn a_chip_pick_in_own_panes_is_kept_by_their_row() {
    guest_in_spectrograph_survives_the_rows(|t, p| {
        t.st.update(Msg::PanePick(p, PaneMenuRow::Meas(MeasId(3))), &t.keys)
    });
}

/// The same with `Spectrum 1`'s row dragged onto the spectrograph.
#[test]
fn a_row_dropped_in_own_panes_is_kept_by_their_row() {
    guest_in_spectrograph_survives_the_rows(|t, p| {
        use ac2_scene::meas_list::TreeKey;
        let mut r = t.st.update(
            Msg::RowDrag(DragMsg::Start(TreeKey::Meas(MeasId(3)))),
            &t.keys,
        );
        r.extend(t.st.update(
            Msg::RowDrag(DragMsg::Over {
                target: Some(DropTarget {
                    pane: p,
                    edge: None,
                }),
                compare: false,
            }),
            &t.keys,
        ));
        r.extend(t.st.update(Msg::RowDrag(DragMsg::Drop { compare: false }), &t.keys));
        r
    });
}

/// `FOH SPL` picked on the last pane of `Spectrum 2`'s own panes turns it into an SPL pane,
/// stored on `Spectrum 2`; `TF 1`'s row and `Spectrum 2`'s again bring the SPL pane back
/// showing `FOH SPL`, and `FOH SPL` was never given the panes.
#[test]
fn a_pick_turning_a_pane_into_another_kind_comes_back_with_the_row() {
    let mut t = desk_on_spectrum_2();
    let last = *t.st.layout.root.reading_order().last().expect("a pane");
    let r =
        t.st.update(Msg::PanePick(last, PaneMenuRow::Meas(MeasId(1))), &t.keys);
    assert_eq!(layout_targets(&r), [MeasId(4)], "{r:?}");
    let stored = t.st.pane_layout();
    answer_layout_calls(&mut t, &r);
    let mut s = rig();
    let mut m = meas(4, "Spectrum 2", spectrum());
    m.layout = Some(stored.clone());
    s.measurements.push(m);
    t.conn(mirror(s));
    let r = select(&mut t, 2);
    assert!(layout_calls(&r).is_empty(), "TF 1's row: {r:?}");
    assert_eq!(t.st.pane_layout(), three_transfer());
    let r = select(&mut t, 4);
    assert!(layout_calls(&r).is_empty(), "Spectrum 2's row: {r:?}");
    assert_eq!(t.st.pane_layout(), stored);
    assert_eq!(
        shown(&t),
        [Some(MeasId(4)), Some(MeasId(4)), Some(MeasId(1))]
    );
    assert!(t.st.meas(MeasId(1)).expect("FOH SPL").layout.is_none());
}

/// Three spectrum panes of `Spectrum 2`'s: `Spectrum 1` picked on the middle one changes
/// that pane alone, the others still on `Spectrum 2`, and neither the pick nor a click in
/// another pane moves the selection off `Spectrum 2`'s row.
#[test]
fn a_pick_in_the_middle_of_three_panes_changes_that_pane_alone() {
    let mut t = desk_on_spectrum_2();
    let order = t.st.layout.root.reading_order();
    let (s1, s2) = (MeasId(3), MeasId(4));
    t.st.update(Msg::PanePick(order[1], PaneMenuRow::Meas(s1)), &t.keys);
    assert_eq!(shown(&t), [Some(s2), Some(s1), Some(s2)]);
    assert_eq!(t.st.selected, Some(s2), "the selection stays on the row");
    t.st.update(Msg::FocusPane(order[2]), &t.keys);
    assert_eq!(t.st.selected, Some(s2), "a click selects nothing");
    assert_eq!(shown(&t), [Some(s2), Some(s1), Some(s2)]);
}

/// `TF 2` picked on the first of `TF 1`'s panes: a delay key acts on the measurement its
/// pane shows, whatever row is selected.
#[test]
fn a_pane_key_acts_on_the_measurement_its_pane_shows() {
    let mut s = rig();
    s.measurements.push(meas(5, "TF 2", transfer()));
    let mut t = T::fresh();
    t.connect(s);
    select(&mut t, 2);
    let order = t.st.layout.root.reading_order();
    t.st.update(
        Msg::PanePick(order[0], PaneMenuRow::Meas(MeasId(5))),
        &t.keys,
    );
    assert_eq!(t.st.selected, Some(MeasId(2)));
    let finds = |r: &[Request]| -> Vec<MeasId> {
        r.iter()
            .filter_map(|r| match r {
                Request::FindDelay { meas, .. } => Some(*meas),
                _ => None,
            })
            .collect()
    };
    t.st.update(Msg::FocusPane(order[0]), &t.keys);
    assert_eq!(finds(&t.key("X")), [MeasId(5)], "the picked pane");
    t.st.update(Msg::FocusPane(order[1]), &t.keys);
    assert_eq!(finds(&t.key("X")), [MeasId(2)], "a pane following the row");
}

/// A layout's level ranges come on screen with its panes as the operator's; a zoom sends
/// the pane's new range with the layout, the other panes still carrying none.
#[test]
fn a_layout_brings_its_level_ranges() {
    let mut l = three_transfer();
    let saved = wire::LevelRange { lo: -30.0, hi: 6.0 };
    l.panes_mut()[0].levels.transfer = Some(saved);
    let mut s = rig();
    s.measurements[1].layout = Some(l.clone());
    let mut t = T::fresh();
    t.connect(s);
    select(&mut t, 2);
    assert_eq!(t.st.pane_layout(), l);
    let first = t.st.layout.root.reading_order()[0];
    assert_eq!(
        t.st.level_range(first),
        Some(ac2_scene::axis::Range::new(-30.0, 6.0))
    );
    assert_eq!(t.st.operator_levels(first).transfer, Some(saved));
    t.st.update(Msg::FocusPane(first), &t.keys);
    let r = t.key("Ctrl+I");
    let calls = layout_calls(&r);
    assert_eq!(calls.len(), 1, "{r:?}");
    let (to, sent) = calls.into_iter().next().expect("a call");
    assert_eq!(to, MeasId(2));
    let mut sent = sent.expect("a layout");
    let panes = sent.panes_mut();
    let zoomed = panes[0].levels.transfer.expect("the zoomed range");
    assert!(zoomed.hi - zoomed.lo < saved.hi - saved.lo, "{zoomed:?}");
    assert!(panes[1..].iter().all(|p| p.levels.is_empty()));
}

/// A layout's pick of a measurement deleted since leaves that pane on the chosen one.
#[test]
fn a_pick_of_a_deleted_measurement_falls_back_to_the_chosen_one() {
    let mut l = three_spectrum();
    l.panes_mut()[1].meas = Some(MeasId(9));
    let mut s = rig();
    let mut m = meas(4, "Spectrum 2", spectrum());
    m.layout = Some(l);
    s.measurements.push(m);
    let mut t = T::fresh();
    t.connect(s);
    select(&mut t, 4);
    assert_eq!(shown(&t), [Some(MeasId(4)); 3]);
    assert_eq!(t.st.pane_layout(), three_spectrum());
}

/// A pick and a level zoom in `Spectrum 2`'s panes travel with its layout: after a lost
/// link and a reconnect the panes keep them, and a fresh app choosing the row gets them
/// from the daemon's copy.
#[test]
fn picks_and_level_ranges_survive_a_reconnect_and_reach_a_fresh_app() {
    let mut t = desk_on_spectrum_2();
    let middle = t.st.layout.root.reading_order()[1];
    let (s1, s2) = (MeasId(3), MeasId(4));
    let r =
        t.st.update(Msg::PanePick(middle, PaneMenuRow::Meas(s1)), &t.keys);
    answer_layout_calls(&mut t, &r);
    let r = t.key("Ctrl+I");
    assert_eq!(layout_targets(&r), [s2], "{r:?}");
    answer_layout_calls(&mut t, &r);
    let zoomed = t.st.level_range(middle);
    let stored = t.st.pane_layout();
    assert_eq!(stored.panes()[1].meas, Some(s1));
    assert!(
        stored.panes()[1].levels.spectrum_dbfs.is_some(),
        "{stored:?}"
    );
    let daemon = || {
        let mut s = rig();
        let mut m = meas(4, "Spectrum 2", spectrum());
        m.layout = Some(stored.clone());
        s.measurements.push(m);
        s
    };
    t.conn(mirror(daemon()));
    t.conn(ConnEvent::Failed {
        target: "local daemon".into(),
        error: "lost".into(),
        retry_in: std::time::Duration::from_secs(1),
    });
    t.connect(daemon());
    assert_eq!(t.st.selected, Some(s2));
    assert_eq!(t.st.pane_layout(), stored, "after a reconnect");
    assert_eq!(shown(&t), [Some(s2), Some(s1), Some(s2)]);
    let mut u = T::fresh();
    u.connect(daemon());
    select(&mut u, 4);
    assert_eq!(u.st.pane_layout(), stored, "a fresh app");
    assert_eq!(shown(&u), [Some(s2), Some(s1), Some(s2)]);
    let middle = u.st.layout.root.reading_order()[1];
    assert_eq!(u.st.level_range(middle), zoomed);
}
