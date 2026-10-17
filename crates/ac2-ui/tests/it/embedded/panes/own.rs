//! Every pane draws its own measurement's group: another measurement's curves only when
//! compared (tagged `cmp`), Imported traces nowhere until moved under a measurement, an SPL
//! pane its own meter only.

use super::new_spectrum;
use crate::embedded::harness::{Driver, NAME, R, measure_from_empty, scene_texts};
use ac2_proto::model::{MeasKind, TraceOwner};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::{MeasId, TraceId};
use ac2_scene::meas_list::TreeKey;
use ac2_scene::primitives::Viewport;
use ac2_scene::theme::Theme;
use ac2_ui::embedded::{EmbeddedBackend, Setup, start_embedded_with};
use ac2_ui::forms::FormKind;
use ac2_ui::keys::Keymap;
use ac2_ui::state::{AppState, DragMsg, DropTarget, Msg, Overlay, PaneId, PaneKind, PaneMenuRow};
use std::time::Instant;

const SIZE: Viewport = Viewport {
    width: 2400.0,
    height: 900.0,
};

fn now() -> ac2_ui::scenes::Now {
    ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    }
}

/// The spectrum panes on screen, in reading order.
fn spectrum_panes(s: &AppState) -> Vec<PaneId> {
    s.visible_panes()
        .into_iter()
        .filter(|p| s.layout.kind(*p) == PaneKind::Spectrum)
        .collect()
}

/// The legend of spectrum pane `pane` as drawn.
fn spectrum_legend(s: &AppState, pane: PaneId) -> Vec<String> {
    ac2_ui::scenes::spectrum(s, pane, &Theme::dark(), SIZE, now()).legend_shown
}

/// The trace `meas` owns, once the daemon lists one.
fn owned_trace(s: &AppState, meas: MeasId) -> Option<TraceId> {
    s.daemon()?
        .traces
        .iter()
        .find(|t| t.edit.owner == TraceOwner::Meas { meas })
        .map(|t| t.id)
}

/// `trace` selected (a click on its row; a click on the selected row unselects it).
fn select(d: &mut Driver, trace: TraceId) {
    if d.st.selected_trace != Some(trace) {
        d.send(Msg::SelectTrace(trace));
    }
}

/// The selected trace moved under `to` with the move dialog (Shift+F2).
fn move_selected(d: &mut Driver, trace: TraceId, to: TraceOwner) -> R {
    select(d, trace);
    d.key("Shift+F2");
    let Overlay::Choose(c) = &mut d.st.overlay else {
        return Err(format!(
            "no move dialog: {:?}, selected {:?}, toasts {:?}",
            d.st.overlay, d.st.selected_trace, d.st.toasts
        )
        .into());
    };
    c.index = c
        .owners
        .iter()
        .position(|o| *o == to)
        .ok_or("the owner among the choices")?;
    d.key("Enter");
    d.until("the trace moved", |s| {
        s.trace_meta(trace).is_ok_and(|m| m.edit.owner == to)
    })
}

/// A capture of `meas` with Ctrl+`slot`, its pane focused and the measurement selected,
/// once its spectrum has arrived.
fn capture(d: &mut Driver, meas: MeasId, slot: u8) -> R<TraceId> {
    let topic = Topic::Data {
        meas,
        stream: Stream::Spec,
    };
    d.until("a spectrum to capture", |s| {
        s.data
            .as_ref()
            .is_some_and(|x| x.latest.get(&topic).is_some())
    })?;
    d.send(Msg::SelectMeas(meas));
    d.key(&format!("Ctrl+{slot}"));
    d.until("the capture", |s| owned_trace(s, meas).is_some())?;
    owned_trace(&d.st, meas).ok_or_else(|| "the capture".into())
}

/// From an empty daemon, two spectrum measurements side by side, a capture each: each pane
/// draws its own measurement's curves only. C on the right one's capture asks where; ticked
/// into the left one's measurement, the left pane draws it tagged `cmp`. Moved under Imported it is drawn on no pane (C refused, the
/// pane's hint says it waits there); moved back it is drawn again.
#[test]
fn two_spectra_each_draw_their_own_from_an_empty_daemon() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let s1 = new_spectrum(&mut d, false)?;
    let s2 = new_spectrum(&mut d, true)?;
    // Spectrum 1 on the spectrum pane, split: Spectrum 2 on the right half.
    d.key("Alt+2");
    d.send(Msg::SelectMeas(s1));
    d.key("N");
    let right = d.st.layout.focus;
    d.send(Msg::PanePick(right, PaneMenuRow::Meas(s2)));
    let [left, r] = spectrum_panes(&d.st)[..] else {
        return Err(format!("{:?}", spectrum_panes(&d.st)).into());
    };
    assert_eq!(r, right);
    let name =
        |s: &AppState, m: MeasId| s.meas(m).map(|m| m.config.name.clone()).unwrap_or_default();
    let (n1, n2) = (name(&d.st, s1), name(&d.st, s2));

    d.send(Msg::FocusPane(left));
    let t1 = capture(&mut d, s1, 1)?;
    d.send(Msg::FocusPane(right));
    let t2 = capture(&mut d, s2, 2)?;
    d.send(Msg::PanePick(left, PaneMenuRow::Meas(s1)));
    d.send(Msg::PanePick(right, PaneMenuRow::Meas(s2)));
    let label = |s: &AppState, t: TraceId| {
        s.trace_meta(t)
            .map(|m| m.edit.name.clone())
            .unwrap_or_default()
    };
    let (l1, l2) = (label(&d.st, t1), label(&d.st, t2));
    let names = |s: &AppState, p: PaneId| -> Vec<String> {
        spectrum_legend(s, p)
            .into_iter()
            .map(|e| e.split(" · ").next().unwrap_or_default().to_owned())
            .collect()
    };
    // Each pane its own live curve and capture (drawn once its data has arrived).
    d.until("each pane its own curves", |s| {
        names(s, left) == [n1.clone(), l1.clone()] && names(s, right) == [n2.clone(), l2.clone()]
    })?;

    // C on Spectrum 2's capture asks where to compare it, and nothing is drawn until a
    // measurement is ticked; then Spectrum 1's pane draws it, tagged, and Spectrum 2's
    // pane is as it was.
    select(&mut d, t2);
    d.key("C");
    let Overlay::Choose(c) = &d.st.overlay else {
        return Err(format!("no chooser: {:?}", d.st.overlay).into());
    };
    assert_eq!(c.title, format!("Compare {} with…", c.label));
    assert_eq!(
        c.choices
            .iter()
            .map(|c| c.label.clone())
            .collect::<Vec<_>>(),
        [format!("  {n1} · on screen")]
    );
    assert_eq!(names(&d.st, left), [n1.clone(), l1.clone()]);
    d.key("Enter");
    d.until("the capture compared into Spectrum 1", |s| {
        s.meas(s1).is_some_and(|m| !m.compared.is_empty())
    })?;
    d.key("Escape");
    d.send(Msg::PanePick(left, PaneMenuRow::Meas(s1)));
    let legend = spectrum_legend(&d.st, left);
    assert!(
        legend.iter().any(|e| e.starts_with(&format!("{l2} · cmp"))),
        "{legend:?}"
    );
    assert!(
        legend
            .iter()
            .all(|e| e.split(" · ").next() != Some(n2.as_str())),
        "Spectrum 2's live curve not compared: {legend:?}"
    );
    assert_eq!(names(&d.st, right), [n2.clone(), l2.clone()]);
    // Enter on its ticked row takes it out again.
    select(&mut d, t2);
    d.key("C");
    d.key("Enter");
    d.until("compare off", |s| {
        s.meas(s1).is_some_and(|m| m.compared.is_empty())
    })?;
    d.key("Escape");

    // Under Imported: on no pane, C refused, the hint says where it waits.
    move_selected(&mut d, t2, TraceOwner::Imported)?;
    d.send(Msg::PanePick(left, PaneMenuRow::Meas(s1)));
    d.send(Msg::PanePick(right, PaneMenuRow::Meas(s2)));
    for p in [left, right] {
        assert!(
            !names(&d.st, p).contains(&l2),
            "pane {p:?}: {:?}",
            names(&d.st, p)
        );
    }
    select(&mut d, t2);
    d.key("C");
    assert_eq!(
        d.st.overlay,
        Overlay::None,
        "an Imported trace is not compared"
    );
    assert!(
        d.st.toasts
            .iter()
            .any(|t| t.text.ends_with(&format!(
                "({l2}) is under Imported: it is no measurement's to compare — select it, then Move to measurement… (Shift+F2)"
            ))),
        "{:?}",
        d.st.toasts
    );
    d.send(Msg::PanePick(right, PaneMenuRow::Meas(s2)));
    let hint =
        d.st.empty_hint(right, &d.keys)
            .map(|h| h.text)
            .unwrap_or_default();
    assert!(
        hint.starts_with("1 trace under Imported is not on this pane"),
        "{hint}"
    );

    // Moved back under Spectrum 2: its pane draws it again.
    move_selected(&mut d, t2, TraceOwner::Meas { meas: s2 })?;
    d.send(Msg::PanePick(right, PaneMenuRow::Meas(s2)));
    d.until("the capture on its pane again", |s| {
        names(s, right).contains(&l2)
    })?;
    assert!(
        !names(&d.st, left).contains(&l2),
        "{:?}",
        names(&d.st, left)
    );
    drop(d);
    drop(daemon);
    Ok(())
}

/// From an empty daemon, a transfer capture, then the measurement deleted keeping it: the
/// capture is under Imported, and the transfer pane, with no measurement to show, draws
/// nothing of it and says to make a measurement.
#[test]
fn a_transfer_pane_does_not_draw_imported_from_an_empty_daemon() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let tf =
        d.st.selected_meas()
            .map(|m| m.id)
            .ok_or("the measurement")?;
    d.key("Alt+1");
    let topic = Topic::Data {
        meas: tf,
        stream: Stream::Tf,
    };
    d.until("a transfer frame to capture", |s| {
        s.data
            .as_ref()
            .is_some_and(|x| x.latest.get(&topic).is_some())
    })?;
    d.key("Ctrl+1");
    d.until("the capture", |s| owned_trace(s, tf).is_some())?;
    let trace = owned_trace(&d.st, tf).ok_or("the capture")?;
    d.send(Msg::SelectMeas(tf));
    d.key("Delete");
    let Overlay::Choose(c) = &d.st.overlay else {
        return Err(format!("{:?}", d.st.overlay).into());
    };
    assert_eq!(c.index, ac2_scene::meas_list::KEEP);
    d.key("Enter");
    d.until("the measurement gone, its capture under Imported", |s| {
        s.meas(tf).is_none()
            && s.trace_meta(trace)
                .is_ok_and(|m| m.edit.owner == TraceOwner::Imported)
    })?;
    let pane = crate::common::pane(&d.st, PaneKind::Transfer);
    assert!(
        !d.st.pane_shows_stored(pane),
        "the Imported capture is not drawn"
    );
    let legend: Vec<String> = ac2_ui::scenes::transfer(&d.st, pane, &Theme::dark(), SIZE, now())
        .legend
        .iter()
        .map(|e| e.name.clone())
        .collect();
    assert!(legend.is_empty(), "{legend:?}");
    // With no measurement left, the hint says to make one (a capture is moved under one).
    let hint =
        d.st.empty_hint(pane, &d.keys)
            .map(|h| h.text)
            .unwrap_or_default();
    assert!(hint.starts_with("No measurements — "), "{hint}");
    drop(d);
    drop(daemon);
    Ok(())
}

/// An SPL meter from the palette's dialog (on the dialog's input or, `other`, the one
/// before it): its id once it runs.
fn new_spl(d: &mut Driver, other: bool) -> R<MeasId> {
    let before: Vec<MeasId> = d.st.measurements().iter().map(|m| m.id).collect();
    d.key("Ctrl+K");
    d.send(Msg::Text("new spl".into()));
    d.key("Enter");
    d.until(
        "the SPL dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Spl),
    )?;
    if other {
        d.key("ArrowLeft");
    }
    d.key("Enter");
    d.until("the new SPL meter, running", |s| {
        s.measurements().iter().any(|m| {
            !before.contains(&m.id) && matches!(m.config.kind, MeasKind::Spl { .. }) && m.running
        })
    })?;
    d.st.measurements()
        .iter()
        .find(|m| !before.contains(&m.id))
        .map(|m| m.id)
        .ok_or_else(|| "the new SPL meter".into())
}

/// The texts SPL pane `pane` draws, or its placeholder when it draws nothing.
fn spl_texts(s: &AppState, pane: PaneId) -> Vec<String> {
    let keys = Keymap::default();
    ac2_ui::scenes::spl_pane(s, pane, &keys, &Theme::dark(), SIZE, now()).map_or_else(
        || vec![ac2_ui::scenes::spl_placeholder(s, pane, &keys)],
        |(scene, _)| scene_texts(&scene),
    )
}

/// From an empty daemon, two SPL meters: the SPL pane shows its own meter only — hidden it
/// says so, stopped it keeps its own last reading, never the other meter's. Two meters side
/// by side: a new pane (N) and the other meter dragged onto it.
#[test]
fn two_spl_meters_side_by_side_by_drag_from_an_empty_daemon() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let m1 = new_spl(&mut d, false)?;
    let m2 = new_spl(&mut d, true)?;
    let name =
        |s: &AppState, m: MeasId| s.meas(m).map(|m| m.config.name.clone()).unwrap_or_default();
    let (n1, n2) = (name(&d.st, m1), name(&d.st, m2));
    assert_ne!(n1, n2);
    let has = |t: &[String], n: &str| t.iter().any(|l| l.contains(n));
    d.key("Alt+4");
    let pane = d.st.layout.focus;
    assert_eq!(d.st.layout.kind(pane), PaneKind::Spl);
    d.send(Msg::PanePick(pane, PaneMenuRow::Meas(m1)));
    d.until("meter 1 reading on its pane", |s| {
        let t = spl_texts(s, pane);
        has(&t, &n1) && !has(&t, &n2)
    })?;

    // Hidden: the pane says so, and does not show meter 2 instead.
    d.send(Msg::SelectMeas(m1));
    d.key("A");
    d.until("meter 1 hidden", |s| {
        s.meas(m1).is_some_and(|m| s.meas_hidden(m))
    })?;
    assert_eq!(
        spl_texts(&d.st, pane),
        [format!("{n1} hidden — A shows it")]
    );
    d.key("A");
    d.until("meter 1 shown again", |s| {
        s.meas(m1).is_some_and(|m| !s.meas_hidden(m)) && has(&spl_texts(s, pane), &n1)
    })?;

    // Stopped: still its own meter (its last reading, stopped), never meter 2's.
    d.send(Msg::FocusPane(pane));
    d.key("S");
    d.until("meter 1 stopped", |s| {
        s.meas(m1).is_some_and(|m| !m.running)
    })?;
    d.pump();
    let t = spl_texts(&d.st, pane);
    assert!(!has(&t, &n2), "{t:?}");
    assert!(
        has(&t, &n1) || t == [format!("{n1} stopped — S starts it")],
        "{t:?}"
    );
    d.key("S");
    d.until("meter 1 running", |s| s.meas(m1).is_some_and(|m| m.running))?;

    // A new pane beside it, meter 2 dropped on it (no Shift): both read side by side.
    d.send(Msg::FocusPane(pane));
    d.key("N");
    let other = d.st.layout.focus;
    assert_ne!(other, pane);
    assert_eq!(d.st.layout.kind(other), PaneKind::Spl);
    d.send(Msg::RowDrag(DragMsg::Start(TreeKey::Meas(m2))));
    d.send(Msg::RowDrag(DragMsg::Over {
        target: Some(DropTarget {
            pane: other,
            edge: None,
        }),
        compare: false,
    }));
    d.send(Msg::RowDrag(DragMsg::Drop { compare: false }));
    d.until("each pane its own meter", |s| {
        s.pane_meas(pane).map(|m| m.id) == Some(m1)
            && s.pane_meas(other).map(|m| m.id) == Some(m2)
            && {
                let (a, b) = (spl_texts(s, pane), spl_texts(s, other));
                has(&a, &n1) && !has(&a, &n2) && has(&b, &n2) && !has(&b, &n1)
            }
    })?;
    assert!(d.st.visible_panes().contains(&pane) && d.st.visible_panes().contains(&other));
    drop(d);
    drop(daemon);
    Ok(())
}

/// From an empty daemon, two sweep measurements with a run each: the sweep pane shows its
/// own sweep's run even with the other's selected, and a Shift-drop (compare) of the other
/// run overlays it, listed `· cmp` in the legend.
#[test]
fn slow_the_distortion_pane_shows_its_own_sweep_from_an_empty_daemon() -> R {
    use crate::embedded::harness::sweep_from_the_dialog;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    d.st.set_prefs(crate::common::grid_with_sweep_ui_prefs());
    measure_from_empty(&mut d)?;
    let run_a = sweep_from_the_dialog(&mut d)?;
    let sweep_a = d.st.sweep_meas().map(|m| m.id).ok_or("sweep A")?;
    let run_b = sweep_from_the_dialog(&mut d)?;
    let sweep_b = d.st.sweep_meas().map(|m| m.id).ok_or("sweep B")?;
    assert_ne!(sweep_a, sweep_b);
    let pane = crate::common::pane(&d.st, PaneKind::Distortion);
    d.send(Msg::FocusPane(pane));
    d.send(Msg::PanePick(pane, PaneMenuRow::Meas(sweep_a)));
    // Sweep B's run selected with the transfer pane focused (a child row goes to the
    // focused pane only): the sweep pane keeps sweep A.
    d.send(Msg::FocusPane(crate::common::pane(
        &d.st,
        PaneKind::Transfer,
    )));
    select(&mut d, run_b);
    assert_eq!(d.st.selected_trace, Some(run_b));
    assert_eq!(d.st.pane_meas(pane).map(|m| m.id), Some(sweep_a));
    assert_eq!(
        d.st.pane_sweep(pane).map(|(t, _)| t.meta.id),
        Some(run_a),
        "sweep A's pane: its own run, not the selected one of sweep B"
    );
    assert!(d.st.compared_sweeps(pane).is_empty());

    // Sweep B's run Shift-dropped on the pane: compared over sweep A's run.
    d.send(Msg::RowDrag(DragMsg::Start(TreeKey::Trace(run_b))));
    d.send(Msg::RowDrag(DragMsg::Over {
        target: Some(DropTarget { pane, edge: None }),
        compare: true,
    }));
    d.send(Msg::RowDrag(DragMsg::Drop { compare: true }));
    assert_eq!(
        d.st.pane_meas(pane).map(|m| m.id),
        Some(sweep_a),
        "the pane keeps sweep A"
    );
    let compared: Vec<TraceId> =
        d.st.compared_sweeps(pane)
            .iter()
            .map(|(t, _)| t.meta.id)
            .collect();
    assert_eq!(compared, [run_b]);
    let name_b = d.st.trace_meta(run_b).map(|m| m.edit.name.clone())?;
    let ac2_ui::scenes::SweepPane::Distortion(x) =
        ac2_ui::scenes::sweep(&d.st, pane, &Theme::dark(), SIZE, now())
    else {
        return Err("not the distortion view".into());
    };
    let legend: Vec<&str> = x.legend.iter().map(|e| e.name.as_str()).collect();
    assert!(
        legend.contains(&format!("{name_b} · cmp").as_str()),
        "{legend:?}"
    );
    drop(d);
    drop(daemon);
    Ok(())
}

/// From an empty daemon: `Spectrum 2`'s row chosen and its spectrum pane split, so it has
/// panes of its own with two spectrum panes; `Spectrum 1` picked on the right one's chip. `Spectrum 1`'s row, then
/// `Spectrum 2`'s again: its panes come back with the right one still on `Spectrum 1`, and
/// `Spectrum 1` was never given them.
#[test]
fn a_chip_pick_in_own_panes_comes_back_with_their_row_from_an_empty_daemon() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let s1 = new_spectrum(&mut d, false)?;
    let s2 = new_spectrum(&mut d, true)?;
    d.key("Alt+2");
    d.send(Msg::SelectMeas(s2));
    d.key("N");
    d.until("Spectrum 2's own panes", |s| {
        s.meas(s2)
            .and_then(|m| m.layout.as_ref())
            .is_some_and(|l| *l == s.pane_layout())
    })?;
    let [_, right] = spectrum_panes(&d.st)[..] else {
        return Err(format!("{:?}", spectrum_panes(&d.st)).into());
    };
    d.send(Msg::PanePick(right, PaneMenuRow::Meas(s1)));
    let shown = |s: &AppState| -> Vec<Option<MeasId>> {
        spectrum_panes(s)
            .into_iter()
            .map(|p| s.pane_meas(p).map(|m| m.id))
            .collect()
    };
    assert_eq!(
        shown(&d.st),
        [Some(s2), Some(s1)],
        "the pick: that pane alone"
    );
    assert_eq!(d.st.selected, Some(s1));
    d.send(Msg::SelectMeas(s1));
    d.send(Msg::SelectMeas(s2));
    assert_eq!(d.st.selected, Some(s2));
    assert_eq!(spectrum_panes(&d.st).len(), 2);
    assert_eq!(
        shown(&d.st),
        [Some(s2), Some(s1)],
        "the right pane keeps the pick"
    );
    assert_eq!(d.st.layout.focus, right, "the focus stays");
    assert!(
        d.st.meas(s1).is_some_and(|m| m.layout.is_none()),
        "Spectrum 1 has no panes of its own"
    );
    drop(d);
    drop(daemon);
    Ok(())
}

/// From an empty daemon, two spectra side by side: B on the left changes only Spectrum 1's
/// style, the offset keys on the right only Spectrum 2's offset; Spectrum 2 compared into
/// Spectrum 1 draws in the left pane at its own offset. A client connecting afresh finds
/// the styles, the compare list and the offset kept by the daemon.
#[test]
fn display_settings_and_compare_lists_live_on_the_daemon_from_an_empty_daemon() -> R {
    use ac2_proto::model::{Compared, SpectrumStyle};
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let s1 = new_spectrum(&mut d, false)?;
    let s2 = new_spectrum(&mut d, true)?;
    d.key("Alt+2");
    d.send(Msg::SelectMeas(s1));
    d.key("N");
    let right = d.st.layout.focus;
    d.send(Msg::PanePick(right, PaneMenuRow::Meas(s2)));
    let [left, _] = spectrum_panes(&d.st)[..] else {
        return Err(format!("{:?}", spectrum_panes(&d.st)).into());
    };
    let style = |s: &AppState, m: MeasId| {
        s.meas(m)
            .and_then(|m| m.config.kind.display().map(|d| d.style))
    };
    let name =
        |s: &AppState, m: MeasId| s.meas(m).map(|m| m.config.name.clone()).unwrap_or_default();
    let (n1, n2) = (name(&d.st, s1), name(&d.st, s2));
    let before = style(&d.st, s2);

    d.send(Msg::FocusPane(left));
    d.send(Msg::PanePick(left, PaneMenuRow::Meas(s1)));
    d.key("B");
    d.until("Spectrum 1 drawn as a line", |s| {
        style(s, s1) == Some(SpectrumStyle::Line)
    })?;
    assert_eq!(style(&d.st, s2), before, "Spectrum 2 keeps its style");

    d.send(Msg::FocusPane(right));
    d.key("Alt+Up");
    d.until("Spectrum 2 offset +1 dB", |s| s.meas_offset(s2) == 1.0)?;
    assert_eq!(d.st.meas_offset(s1), 0.0);

    d.send(Msg::SelectMeas(s2));
    d.key("C");
    let Overlay::Choose(c) = &mut d.st.overlay else {
        return Err(format!("no chooser: {:?}", d.st.overlay).into());
    };
    c.index = c
        .choices
        .iter()
        .position(|c| c.label.trim_start().starts_with(&n1))
        .ok_or("Spectrum 1 among the targets")?;
    d.key("Enter");
    d.key("Escape");
    let compared = vec![Compared::Meas { meas: s2 }];
    d.until("Spectrum 2 compared into Spectrum 1", |s| {
        s.meas(s1).is_some_and(|m| m.compared == compared)
    })?;
    let at_offset = |s: &AppState, p: PaneId| {
        spectrum_legend(s, p)
            .iter()
            .any(|e| e.starts_with(&n2) && e.contains("offset +1.0 dB"))
    };
    d.send(Msg::PanePick(left, PaneMenuRow::Meas(s1)));
    d.until("Spectrum 2 at +1 dB in Spectrum 1's pane", |s| {
        at_offset(s, left)
    })?;
    drop(d);

    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    d.synced()?;
    assert_eq!(style(&d.st, s1), Some(SpectrumStyle::Line));
    assert_eq!(style(&d.st, s2), before);
    assert_eq!(d.st.meas(s1).map(|m| m.compared.clone()), Some(compared));
    assert_eq!(d.st.meas_offset(s2), 1.0);
    d.key("Alt+2");
    let p = spectrum_panes(&d.st)
        .first()
        .copied()
        .ok_or("a spectrum pane")?;
    d.send(Msg::PanePick(p, PaneMenuRow::Meas(s1)));
    d.until("Spectrum 2 at +1 dB after reconnecting", |s| {
        at_offset(s, p)
    })?;
    drop(d);
    drop(daemon);
    Ok(())
}
