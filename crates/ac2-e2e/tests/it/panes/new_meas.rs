//! The New measurement chooser (Ctrl+M) from an empty daemon: every kind listed, the kind
//! of the focused pane highlighted, the chosen kind's form, and the new measurement in a
//! pane of its kind, never in place of the focused one.

use crate::harness::{Driver, NAME, R, measure_from_empty};
use ac2_e2e::{Setup, sim_daemon};
use ac2_proto::model::MeasKind;
use ac2_proto::units::MeasId;
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
    let daemon = sim_daemon(Setup::Empty)?;
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
    // The session is open by the time it asks: every kind can be made but math,
    // alignment and the spatial average, which need two curves first; its inputs can be
    // metered.
    let blocked: Vec<bool> = c.choices.iter().map(|x| x.blocked.is_some()).collect();
    assert_eq!(
        blocked,
        [false, false, false, false, false, true, true, true, false]
    );

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
    let daemon = sim_daemon(Setup::Empty)?;
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
            "Math channel",
            "Alignment",
            "Spatial average",
            "Programme meter"
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
    // On a screen of its own: one spectrum pane showing it, focused.
    assert_eq!(crate::common::visible(&d.st), [PaneKind::Spectrum]);
    let f = d.st.layout.focus;
    assert_eq!(d.st.pane_meas(f).map(|m| m.id), Some(sp));
    // The transfer measurement's row brings its own panes back, its pane showing it.
    let tf_meas = shown.ok_or("the transfer measurement")?;
    d.send(Msg::SelectMeas(tf_meas));
    assert_eq!(crate::common::visible(&d.st), panes);

    // The spectrum pane focused: Spectrum highlighted; Esc closes.
    d.send(Msg::SelectMeas(sp));
    let spane =
        d.st.layout
            .lead(PaneKind::Spectrum)
            .ok_or("spectrum pane")?;
    d.send(Msg::FocusPane(spane));
    d.key("Ctrl+M");
    assert_eq!(chooser(&d.st).map(|c| c.index), Some(1));
    d.key("Escape");
    assert_eq!(d.st.overlay, Overlay::None);

    drop(d);
    drop(daemon);
    Ok(())
}
