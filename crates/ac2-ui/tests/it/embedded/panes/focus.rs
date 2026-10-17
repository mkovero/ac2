//! Focus: the key hints follow the focused pane, it takes any pick and child row, and Tab
//! steps through the measurements.

use super::{hint_line, new_spectrum, spectrum_panes};
use crate::embedded::harness::{Driver, NAME, R, measure_from_empty};
use ac2_proto::model::MeasKind;
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::MeasId;
use ac2_scene::view::SpectrumMode;
use ac2_ui::embedded::{EmbeddedBackend, Setup, start_embedded_with};
use ac2_ui::forms::FormKind;
use ac2_ui::state::{AppState, Msg, Overlay};

/// From an empty daemon: the hint line follows the focused pane as the operator moves
/// around, H opens and closes every key, Shift+H turns the hints off and the palette turns
/// them on again.
#[test]
fn key_hints_follow_the_panes_from_an_empty_daemon() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    d.synced()?;
    // Nothing set up yet: the transfer pane has the focus and its hints.
    let tf = hint_line(&d.st);
    assert_eq!(tf.first().map(String::as_str), Some("V select trace"));
    assert_eq!(tf.last().map(String::as_str), Some("H all keys"));
    // A session and its transfer measurement, from the app.
    d.key("Shift+O");
    d.send(Msg::Text("O".into()));
    d.until("the device list", |s| {
        s.overlay
            .settings()
            .is_some_and(|x| x.session.device_info().is_some())
    })?;
    d.key("Enter");
    d.until("the measurement offer", |s| {
        matches!(s.overlay, Overlay::Offer(_))
    })?;
    d.key("Enter");
    d.until("the measurement", |s| s.selected_meas().is_some())?;
    assert_eq!(hint_line(&d.st), tf);
    // Each pane its own line.
    d.key("Alt+2");
    let sp = hint_line(&d.st);
    assert!(sp.contains(&"P peak hold".to_owned()), "{sp:?}");
    d.key("Alt+3");
    let ir = hint_line(&d.st);
    assert_eq!(ir.first().map(String::as_str), Some("G/Shift+G view"));
    d.key("Alt+4");
    let spl = hint_line(&d.st);
    assert_eq!(spl.first().map(String::as_str), Some("G/Shift+G view"));
    // A fifth pane, turned into the sweep pane from its list.
    d.key("N");
    let f = d.st.layout.focus;
    d.send(Msg::PanePick(
        f,
        ac2_ui::state::PaneMenuRow::Kind(ac2_ui::state::PaneKind::Distortion),
    ));
    assert_eq!(
        d.st.layout.focus_kind(),
        ac2_ui::state::PaneKind::Distortion
    );
    let sw = hint_line(&d.st);
    assert_eq!(sw.first().map(String::as_str), Some("Shift+S new sweep"));
    // The view keys are the same pair in the sweep's IR view (Shift+I).
    assert!(sw.contains(&"G/Shift+G view".to_owned()), "{sw:?}");
    d.key("Shift+I");
    let sw = hint_line(&d.st);
    assert!(sw.contains(&"G/Shift+G view".to_owned()), "{sw:?}");
    // H: every key, and closed again.
    d.key("H");
    assert_eq!(d.st.overlay, Overlay::Help);
    d.key("H");
    assert_eq!(d.st.overlay, Overlay::None);
    // Key hints off from the palette: no line anywhere, remembered.
    d.key("Ctrl+K");
    d.send(Msg::Text("key hints".into()));
    d.key("Enter");
    assert!(!d.st.prefs.key_hints);
    assert!(d.st.prefs_dirty);
    assert!(hint_line(&d.st).is_empty());
    d.key("Alt+1");
    assert!(hint_line(&d.st).is_empty());
    // The palette brings it back.
    d.key("Ctrl+K");
    d.send(Msg::Text("key hints".into()));
    d.key("Enter");
    assert!(d.st.prefs.key_hints);
    assert_eq!(hint_line(&d.st), tf);
    drop(d);
    drop(daemon);
    Ok(())
}

/// From an empty daemon: a transfer measurement and a spectrum; what is picked goes to the
/// focused pane whatever its kind, the focus staying and every other pane as it was.
#[test]
fn the_focused_pane_takes_any_pick_from_an_empty_daemon() -> R {
    use ac2_ui::state::PaneKind::{Spectrum, Spl, Transfer};
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    d.synced()?;
    d.key("Shift+O");
    d.send(Msg::Text("O".into()));
    d.until("the device list", |s| {
        s.overlay
            .settings()
            .is_some_and(|x| x.session.device_info().is_some())
    })?;
    d.key("Enter");
    d.until("the measurement offer", |s| {
        matches!(s.overlay, Overlay::Offer(_))
    })?;
    d.key("Enter");
    d.until("the measurement", |s| s.selected_meas().is_some())?;
    let tf = d.st.selected_meas().map(|m| m.id).ok_or("transfer")?;
    d.key("Ctrl+K");
    d.send(Msg::Text("new spectrum".into()));
    d.key("Enter");
    d.until(
        "the spectrum dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Spectrum),
    )?;
    d.key("Enter");
    d.until("the spectrum measurement", |s| {
        s.measurements()
            .iter()
            .any(|m| matches!(m.config.kind, MeasKind::Spectrum { .. }))
    })?;
    let sp =
        d.st.measurements()
            .iter()
            .find(|m| matches!(m.config.kind, MeasKind::Spectrum { .. }))
            .map(|m| m.id)
            .ok_or("spectrum")?;
    // Picked with the transfer pane focused: it lands there.
    d.key("Alt+1");
    d.send(Msg::SelectMeas(tf));
    assert_eq!(
        crate::common::visible(&d.st),
        [Transfer, Spectrum, Transfer, Spl]
    );

    // The second transfer pane focused: the spectrum picked from the list turns it into a
    // spectrum pane, the transfer pane before it unchanged.
    d.key("Alt+3");
    let f = d.st.layout.focus;
    d.send(Msg::SelectMeas(sp));
    assert_eq!(
        crate::common::visible(&d.st),
        [Transfer, Spectrum, Spectrum, Spl]
    );
    assert_eq!(d.st.layout.focus, f);
    assert_eq!(d.st.pane_meas(f).map(|m| m.id), Some(sp));
    // Tab on to the transfer measurement: the same pane turns back.
    for _ in 0..d.st.tree_meas_order().len() {
        if d.st.layout.kind(f) == Transfer {
            break;
        }
        d.key("Tab");
    }
    assert_eq!(
        crate::common::visible(&d.st),
        [Transfer, Spectrum, Transfer, Spl]
    );
    assert_eq!(d.st.layout.focus, f);
    assert_eq!(d.st.pane_meas(f).map(|m| m.id), Some(tf));
    drop(d);
    drop(daemon);
    Ok(())
}

/// From an empty daemon: a transfer measurement, a spectrum and an SPL meter made from the
/// app; Tab / Shift+Tab select them in the tree's order, wrapping, and the focused pane
/// shows each, turning into its kind.
#[test]
fn tab_steps_through_the_measurements_from_an_empty_daemon() -> R {
    use ac2_ui::state::PaneKind::{Spectrum, Spl, Transfer};
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    for (words, kind, is) in [
        (
            "new spectrum",
            FormKind::Spectrum,
            (|k: &MeasKind| matches!(k, MeasKind::Spectrum { .. })) as fn(&MeasKind) -> bool,
        ),
        ("new spl", FormKind::Spl, |k| {
            matches!(k, MeasKind::Spl { .. })
        }),
    ] {
        d.key("Ctrl+K");
        d.send(Msg::Text(words.into()));
        d.key("Enter");
        d.until(
            words,
            |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == kind),
        )?;
        d.key("Enter");
        d.until("the new measurement", |s| {
            s.measurements().iter().any(|m| is(&m.config.kind))
        })?;
    }
    let id_of = |s: &AppState, is: fn(&MeasKind) -> bool| {
        s.measurements()
            .iter()
            .find(|m| is(&m.config.kind))
            .map(|m| m.id)
            .ok_or("measurement")
    };
    let tf = id_of(&d.st, |k| matches!(k, MeasKind::Transfer { .. }))?;
    let sp = id_of(&d.st, |k| matches!(k, MeasKind::Spectrum { .. }))?;
    let spl = id_of(&d.st, |k| matches!(k, MeasKind::Spl { .. }))?;
    // The tree lists the measurements in the order they were made.
    let order = [(tf, Transfer), (sp, Spectrum), (spl, Spl)];
    assert_eq!(d.st.tree_meas_order(), [tf, sp, spl]);

    d.send(Msg::SelectMeas(tf));
    let f = d.st.layout.focus;
    // A transfer or spectrum measurement is two stops, its header and its live curve; the
    // SPL meter, with no live curve row, one.
    let steps = [
        (tf, true),
        (sp, false),
        (sp, true),
        (spl, false),
        (tf, false),
        (tf, true),
        (sp, false),
    ];
    for (i, (id, live)) in steps.into_iter().enumerate() {
        d.key("Tab");
        let pane = order.iter().find(|(m, _)| *m == id).expect("listed").1;
        assert_eq!(d.st.selected, Some(id), "Tab {i}");
        assert_eq!(d.st.live_row_selected(), live, "Tab {i}");
        assert_eq!(d.st.selected_trace, None);
        assert_eq!(d.st.layout.focus, f, "Tab {i}");
        assert_eq!(d.st.layout.focus_kind(), pane, "Tab {i}");
        assert_eq!(d.st.pane_meas(f).map(|m| m.id), Some(id), "Tab {i}");
    }
    // On the spectrum now; back round once, by the SPL meter.
    for (want, live) in [
        (tf, true),
        (tf, false),
        (spl, false),
        (sp, true),
        (sp, false),
    ] {
        d.key("Shift+Tab");
        assert_eq!(d.st.selected, Some(want));
        assert_eq!(d.st.live_row_selected(), live);
    }
    assert_eq!(d.st.layout.focus_kind(), Spectrum);
    drop(d);
    drop(daemon);
    Ok(())
}

/// A child row of the list — a live curve, a stored trace — only selects, from an empty
/// daemon: Spectrum 1 on both halves of a split spectrum pane (the left one its
/// spectrograph), Spectrum 2 with a capture and panes of its own. Pane B (the right half)
/// clicked, then Spectrum 2's live row: no pane changes, the focus stays on B. Then
/// Spectrum 2's stored trace row: the same, the trace selected. A pane gets Spectrum 2 from
/// its chip; Spectrum 2's header row brings its own panes.
#[test]
fn a_child_row_moves_no_pane_from_an_empty_daemon() -> R {
    use ac2_ui::state::PaneMenuRow;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let s1 = new_spectrum(&mut d, false)?;
    let s2 = new_spectrum(&mut d, true)?;
    let shown = |s: &AppState| -> Vec<(Option<MeasId>, SpectrumMode)> {
        spectrum_panes(s)
            .into_iter()
            .map(|(m, v, _)| (m, v))
            .collect()
    };
    let half = |s: &AppState, i: usize| {
        s.visible_panes()
            .into_iter()
            .filter(|p| s.layout.kind(*p) == ac2_ui::state::PaneKind::Spectrum)
            .nth(i)
    };
    // Spectrum 1 split side by side (its own panes now).
    d.key("Alt+2");
    d.send(Msg::SelectMeas(s1));
    d.key("N");
    let b = d.st.layout.focus;
    // Spectrum 2 on B: captured (with its row chosen: a capture is of the selected
    // measurement), and given panes of its own (B to its spectrum + spectrograph and back).
    d.send(Msg::PanePick(b, PaneMenuRow::Meas(s2)));
    let topic = Topic::Data {
        meas: s2,
        stream: Stream::Spec,
    };
    d.until("Spectrum 2's spectrum", |s| {
        s.data
            .as_ref()
            .is_some_and(|x| x.latest.get(&topic).is_some())
    })?;
    d.send(Msg::SelectMeas(s2));
    assert_eq!(d.st.layout.focus, b, "Spectrum 2's row: B keeps the focus");
    d.key("Ctrl+1");
    d.until("Spectrum 2's capture", |s| {
        s.daemon().is_some_and(|x| {
            x.traces.iter().any(|t| {
                matches!(t.edit.owner, ac2_proto::model::TraceOwner::Meas { meas } if meas == s2)
            })
        })
    })?;
    let trace = d
        .st
        .daemon()
        .ok_or("the daemon")?
        .traces
        .iter()
        .find(|t| matches!(t.edit.owner, ac2_proto::model::TraceOwner::Meas { meas } if meas == s2))
        .map(|t| t.id)
        .ok_or("the capture")?;
    d.key("G");
    d.key("Shift+G");
    d.until("Spectrum 2's own panes", |s| {
        s.meas(s2).is_some_and(|m| m.layout.is_some())
    })?;
    // B back on Spectrum 1; A (the left half) stepped to its spectrograph.
    d.send(Msg::PanePick(b, PaneMenuRow::Meas(s1)));
    let a = half(&d.st, 0).ok_or("pane A")?;
    d.send(Msg::FocusPane(a));
    d.key("G");
    d.key("G");
    let before = [
        (Some(s1), SpectrumMode::Spectrograph),
        (Some(s1), SpectrumMode::Spectrum),
    ];
    assert_eq!(shown(&d.st), before);
    // Pane B clicked, then Spectrum 2's live row: nothing moves.
    let b = half(&d.st, 1).ok_or("pane B")?;
    d.send(Msg::FocusPane(b));
    let panes = d.st.pane_layout();
    d.send(Msg::SelectLive(s2));
    d.pump();
    assert_eq!(d.st.selected_live, Some(s2));
    assert_eq!(shown(&d.st), before, "the live row");
    assert_eq!(d.st.layout.focus, b, "the focus stays on B");
    assert_eq!(d.st.pane_layout(), panes, "no pane changed");

    // Spectrum 2's stored trace row: the trace selected, nothing moves.
    d.send(Msg::SelectTrace(trace));
    d.pump();
    assert_eq!(d.st.selected_trace, Some(trace));
    assert_eq!(shown(&d.st), before, "the trace row");
    assert_eq!(d.st.layout.focus, b, "the focus stays on B");
    assert_eq!(d.st.pane_layout(), panes, "no pane changed");

    // B's chip puts Spectrum 2 there.
    d.send(Msg::PanePick(b, PaneMenuRow::Meas(s2)));
    assert_eq!(
        shown(&d.st),
        [
            (Some(s1), SpectrumMode::Spectrograph),
            (Some(s2), SpectrumMode::Spectrum),
        ],
        "the chip"
    );

    // Spectrum 2's header row: selected, its own panes on screen.
    d.send(Msg::SelectMeas(s2));
    assert_eq!(d.st.selected, Some(s2));
    assert_eq!(
        Some(d.st.pane_layout()),
        d.st.meas(s2).and_then(|m| m.layout.clone()),
        "the header row brings its panes"
    );
    drop(d);
    drop(daemon);
    Ok(())
}
