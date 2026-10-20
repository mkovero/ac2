//! The New measurement chooser (Ctrl+M) from an empty daemon: every kind listed, the kind
//! of the focused pane highlighted, the chosen kind's form, and the new measurement in a
//! pane of its kind, never in place of the focused one.

use super::spectrum_panes;
use crate::embedded::harness::{Driver, NAME, R, measure_from_empty};
use ac2_proto::model::MeasKind;
use ac2_proto::units::MeasId;
use ac2_ui::embedded::{EmbeddedBackend, Setup, start_embedded_with};
use ac2_ui::forms::FormKind;
use ac2_ui::keys::Chord;
use ac2_ui::state::{AppState, ChoicePrompt, ChoicePurpose, Msg, Overlay, PaneKind};

/// The open chooser.
fn chooser(s: &AppState) -> Option<&ChoicePrompt> {
    match &s.overlay {
        Overlay::Choose(c) if c.purpose == ChoicePurpose::NewMeasurement => Some(c),
        _ => None,
    }
}

/// First run: a session opened on an empty daemon asks what to measure with the chooser;
/// Esc leaves the empty transfer pane naming Ctrl+M; Q down to the last pane keeps it and
/// names the keys that bring more (N, Ctrl+M), never Tab.
#[test]
fn a_fresh_session_asks_what_to_measure() -> R {
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
    d.until("the session", |s| s.open_session().is_some())?;
    d.until("the chooser", |s| chooser(s).is_some())?;
    let c = chooser(&d.st).ok_or("the chooser")?;
    assert_eq!(c.index, 0, "Transfer, on the transfer pane");
    // The session is open by the time it asks: every kind can be made but math, which
    // needs two curves first.
    let blocked: Vec<bool> = c.choices.iter().map(|x| x.blocked.is_some()).collect();
    assert_eq!(blocked, [false, false, false, false, false, true]);

    d.key("Escape");
    assert_eq!(d.st.overlay, Overlay::None);
    assert!(d.st.measurements().is_empty());
    let m = Chord::parse("Ctrl+M")?.label();
    let hint =
        d.st.empty_hint(d.st.layout.focus, &d.keys)
            .map(|h| h.text)
            .unwrap_or_default();
    assert!(
        hint.starts_with(&format!("No measurements — {m} → New measurement…")),
        "{hint}"
    );

    let panes = d.st.layout.views.len();
    for _ in 0..panes {
        d.key("Q");
    }
    assert_eq!(d.st.layout.views.len(), 1);
    let toast =
        d.st.toasts
            .list
            .last()
            .map(|t| t.text.clone())
            .unwrap_or_default();
    let n = Chord::parse("N")?.label();
    assert_eq!(
        toast,
        format!("the last pane stays: {n} splits it · {m}: New measurement…")
    );
    assert!(!toast.contains("Tab"), "{toast}");
    // What it names works: Ctrl+M opens the chooser again.
    d.key("Ctrl+M");
    assert!(chooser(&d.st).is_some());
    d.key("Escape");
    Ok(())
}

#[test]
fn ctrl_m_makes_a_spectrum_from_an_empty_daemon() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    d.synced()?;
    let key = Chord::parse("Ctrl+M")?.label();
    // Nothing yet: the measurement list says where the first one is made.
    assert_eq!(
        d.st.empty_list_text(),
        format!("none — {key}: New measurement…")
    );
    measure_from_empty(&mut d)?;

    // The transfer pane focused: Ctrl+M lists every kind, Transfer highlighted.
    d.key("Alt+1");
    let tf = d.st.layout.focus;
    assert_eq!(d.st.layout.kind(tf), PaneKind::Transfer);
    let shown = d.st.pane_meas(tf).map(|m| m.id);
    let panes = crate::common::visible(&d.st);
    d.key("Ctrl+M");
    let c = chooser(&d.st).ok_or("the chooser")?;
    assert_eq!(c.title, "New measurement");
    let names: Vec<&str> = c
        .choices
        .iter()
        .map(|x| x.label.split(" — ").next().unwrap_or_default())
        .collect();
    assert_eq!(
        names,
        [
            "Transfer",
            "Spectrum",
            "RTA",
            "SPL meter",
            "Sweep",
            "Math channel"
        ]
    );
    assert_eq!(c.index, 0);

    // ↓ Enter: the spectrum's form; Enter makes it.
    d.key("ArrowDown");
    d.key("Enter");
    d.until(
        "the spectrum dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Spectrum),
    )?;
    d.key("Enter");
    d.until("the new spectrum, running", |s| {
        s.measurements()
            .iter()
            .any(|m| matches!(m.config.kind, MeasKind::Spectrum { .. }) && m.running)
    })?;
    let sp: MeasId =
        d.st.measurements()
            .iter()
            .find(|m| matches!(m.config.kind, MeasKind::Spectrum { .. }))
            .map(|m| m.id)
            .ok_or("spectrum")?;
    // On the spectrum pane already there; the transfer pane still shows its measurement.
    assert_eq!(crate::common::visible(&d.st), panes);
    assert_eq!(d.st.layout.kind(tf), PaneKind::Transfer);
    assert_eq!(d.st.pane_meas(tf).map(|m| m.id), shown);
    assert!(
        spectrum_panes(&d.st).iter().any(|(m, _, _)| *m == Some(sp)),
        "{:?}",
        spectrum_panes(&d.st)
    );

    // The spectrum pane focused: Spectrum highlighted; Esc closes.
    let spane =
        d.st.layout
            .lead(PaneKind::Spectrum)
            .ok_or("spectrum pane")?;
    d.send(Msg::FocusPane(spane));
    d.key("Ctrl+M");
    assert_eq!(chooser(&d.st).map(|c| c.index), Some(1));
    d.key("Escape");
    assert_eq!(d.st.overlay, Overlay::None);

    // The SPL pane's list ends in New measurement…, which opens the chooser on SPL meter.
    let spl = d.st.layout.lead(PaneKind::Spl).ok_or("SPL pane")?;
    d.send(Msg::PaneMenu(spl));
    let rows = d.st.pane_menu_rows(spl);
    assert_eq!(
        rows.last().map(|(_, t)| t.clone()),
        Some(format!("New measurement…  {key}"))
    );
    d.key("End");
    d.key("Enter");
    assert_eq!(chooser(&d.st).map(|c| c.index), Some(3));
    d.key("Escape");
    assert_eq!(d.st.overlay, Overlay::None);
    drop(d);
    drop(daemon);
    Ok(())
}
