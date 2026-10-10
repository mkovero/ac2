//! Panes and keys: hints, zoom, delete, export, subscriptions, spectrograph, record and replay.

use super::harness::{Driver, NAME, R, measure_from_empty};
use ac2_client::{ClientConfig, Endpoints};
use ac2_proto::FrameData;
use ac2_proto::model::MeasKind;
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::MeasId;
use ac2_scene::theme::Theme;
use ac2_scene::view::SpectrumMode;
use ac2_ui::embedded::{EmbeddedBackend, Setup, start_embedded_with};
use ac2_ui::forms::FormKind;
use ac2_ui::keys::{CommandId, Keymap};
use ac2_ui::state::{AppState, Msg, Overlay, PromptKind, Severity, StimPhase};
use std::time::{Duration, Instant};

/// The hint line of the focused pane, as the app draws it (PC labels).
fn hint_line(s: &AppState) -> Vec<String> {
    let focus = s.layout.focus;
    let keys = Keymap::default();
    let style = ac2_ui::keys::LabelStyle::Pc;
    // Only the focused pane has one.
    for p in s.layout.panes().into_iter().filter(|p| *p != focus) {
        assert_eq!(s.key_hint_line(&keys, p, style), None);
    }
    s.key_hint_line(&keys, focus, style)
        .map(|v| v.iter().map(ac2_ui::hints::KeyHint::text).collect())
        .unwrap_or_default()
}

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

/// From an empty daemon: two captures, one spread by +3 dB with the keys (its legend says
/// so and its curve moves, the other stays), a spectrum whose level axis goes down to its
/// low levels with the keys, a capture deleted with Delete and a confirmation, and a
/// measurement picked from the list with the layout maximised brings up its pane.
#[test]
fn spread_zoom_and_delete_from_an_empty_daemon() -> R {
    use ac2_proto::units::TraceId;
    use ac2_scene::trace::TraceKey;
    use ac2_ui::state::PaneKind;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let tf = d.st.selected_meas().map(|m| m.id).ok_or("measurement")?;

    // Two captures, their data fetched.
    d.key("Ctrl+1");
    d.until("slot 1", |s| s.slots()[0].is_some())?;
    d.key("Ctrl+2");
    d.until("both captures with their data", |s| {
        s.slots()[1].is_some() && s.traces.len() == 2
    })?;
    let id = |s: &AppState, slot: usize| s.slots()[slot].map(|t| t.id);
    let (a, b) = (id(&d.st, 0).ok_or("slot 1")?, id(&d.st, 1).ok_or("slot 2")?);
    let theme = Theme::dark();
    let size = ac2_scene::primitives::Viewport {
        width: 1200.0,
        height: 600.0,
    };
    let now = || ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let curve = |s: &AppState, t: TraceId| {
        let scene = ac2_ui::scenes::transfer(
            s,
            crate::common::pane(s, ac2_ui::state::PaneKind::Transfer),
            &theme,
            size,
            now(),
        );
        let legend = scene
            .legend
            .iter()
            .find(|e| e.key == TraceKey::Stored(t))
            .map(|e| e.text.clone())
            .unwrap_or_default();
        let mag = scene
            .traces
            .iter()
            .find(|x| x.key == TraceKey::Stored(t))
            .map(|x| x.magnitude_db.clone())
            .unwrap_or_default();
        (legend, mag)
    };
    let (legend_b, before_b) = curve(&d.st, b);
    let (_, before_a) = curve(&d.st, a);
    assert!(!legend_b.contains("dB"), "{legend_b}");

    // V to slot 2, Alt+Shift+↑: +3 dB, said in the legend.
    d.key("V");
    d.key("V");
    assert_eq!(d.st.selected_trace, Some(b));
    d.key("Alt+Shift+Up");
    d.until("the offset held by the daemon", |s| {
        s.traces
            .get(&b)
            .is_some_and(|(t, _)| t.meta.edit.offset.0 == 3.0)
    })?;
    let (legend_b, after_b) = curve(&d.st, b);
    assert!(legend_b.contains("+3.0 dB"), "{legend_b}");
    let moved: Vec<f64> = before_b
        .iter()
        .zip(&after_b)
        .filter(|(x, y)| x.is_finite() && y.is_finite())
        .map(|(x, y)| y - x)
        .collect();
    assert!(!moved.is_empty());
    assert!(moved.iter().all(|m| (m - 3.0).abs() < 1e-9), "{moved:?}");
    let after_a = curve(&d.st, a).1;
    assert_eq!(after_a.len(), before_a.len());
    assert!(
        after_a
            .iter()
            .zip(&before_a)
            .all(|(x, y)| x == y || (x.is_nan() && y.is_nan())),
        "the other capture stays"
    );

    // A spectrum from the palette; its pane's level axis down to the low levels.
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
    d.until("spectrum frames", |s| {
        ac2_ui::scenes::frame(s, sp, Stream::Spec).is_some()
    })?;
    d.key("Alt+2");
    // Its levels are per FFT bin: the axis names the bin width, the tooltip what it means.
    let pane = ac2_ui::scenes::spectrum(
        &d.st,
        crate::common::pane(&d.st, ac2_ui::state::PaneKind::Spectrum),
        &theme,
        size,
        now(),
    );
    assert!(
        pane.unit.starts_with("dBFS per ") && pane.unit.ends_with(" Hz bin (tone)"),
        "{}",
        pane.unit
    );
    assert!(
        pane.unit_help.as_deref().is_some_and(|h| h.contains("RTA")),
        "{:?}",
        pane.unit_help
    );
    let level = |s: &AppState| s.view.spectrum.level;
    // The new spectrum's first frame framed the level axis (as Shift+Home).
    let default = ac2_scene::view::ViewState::default().spectrum.level;
    assert_ne!(level(&d.st), default);
    d.key("Ctrl+Home");
    let start = level(&d.st);
    assert_eq!(start, default);
    for _ in 0..4 {
        d.key("Ctrl+Down");
    }
    assert_eq!(level(&d.st), ac2_scene::axis::Range::new(-140.0, -40.0));
    d.key("Ctrl+I");
    let r = level(&d.st);
    assert!(r.span() < start.span() && r.lo < -100.0, "{r:?}");
    let labels: Vec<String> = ac2_ui::scenes::spectrum(
        &d.st,
        crate::common::pane(&d.st, ac2_ui::state::PaneKind::Spectrum),
        &theme,
        size,
        now(),
    )
    .y_axis
    .labels()
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert!(labels.contains(&"\u{2212}120".to_owned()), "{labels:?}");
    // Shift+Home frames what is shown; Ctrl+Home is the default again.
    d.key("Shift+Home");
    let fit = level(&d.st);
    assert!(fit.is_valid() && fit != r, "{fit:?}");
    assert!(
        d.st.toasts
            .iter()
            .any(|t| t.text.starts_with("Spectrum / RTA: level"))
    );
    d.key("Ctrl+Home");
    assert_eq!(level(&d.st), start);

    // Delete slot 1: asked first, then gone; the selection moves to slot 2.
    d.key("Alt+1");
    d.key("V");
    assert_eq!(d.st.selected_trace, Some(a));
    d.key("Delete");
    assert!(
        matches!(&d.st.overlay, Overlay::Delete(p) if p.target == ac2_ui::state::DeleteTarget::Trace(a))
    );
    d.key("Delete");
    assert_eq!(d.st.selected_trace, Some(b));
    d.until("slot 1 deleted", |s| {
        s.daemon()
            .is_some_and(|x| x.traces.iter().all(|t| t.id != a))
    })?;
    assert!(!d.st.traces.contains_key(&a));
    assert_eq!(d.st.trace_rows().len(), 1);

    // Maximised: a measurement picked in the list brings up its pane.
    d.key("W");
    assert!(d.st.layout.maximized);
    d.send(Msg::SelectMeas(sp));
    assert_eq!(crate::common::visible(&d.st), [PaneKind::Spectrum]);
    assert_eq!(d.st.kind_meas(PaneKind::Spectrum).map(|m| m.id), Some(sp));
    d.send(Msg::SelectMeas(tf));
    assert_eq!(crate::common::visible(&d.st), [PaneKind::Transfer]);
    assert!(d.st.layout.maximized);
    drop(d);
    drop(daemon);
    Ok(())
}

/// A level axis remembered from the last run is where the pane starts; a spectrum that
/// starts still fits the axis on its first frame.
/// The keys act on what was selected last: from an empty daemon, two transfer
/// measurements; the first selected, A hides its curve from the transfer pane while it keeps
/// running and measuring, A shows it again; Backspace asks before it goes and Enter deletes
/// it, the other measuring on.
#[test]
fn a_hides_and_backspace_deletes_the_selected_measurement() -> R {
    use ac2_ui::state::{DeleteTarget, PaneKind};
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let first = d.st.selected_meas().cloned().ok_or("measurement")?;

    // A second transfer measurement from the palette's dialog, as it comes.
    d.key("Ctrl+K");
    d.send(Msg::Text("new transfer".into()));
    d.key("Enter");
    assert!(matches!(&d.st.overlay, Overlay::Form(f) if f.kind == FormKind::Transfer));
    d.key("Enter");
    d.until("the second measurement, running and selected", |s| {
        s.measurements().len() == 2
            && s.selected_meas()
                .is_some_and(|m| m.id != first.id && m.running)
    })?;
    let second = d.st.selected_meas().cloned().ok_or("second")?;
    // The level typed for the first is kept: arm and fire.
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|x| x.generator.firing))?;
    d.tf_frames(second.id, 240)?;
    // The transfer pane draws one measurement's group; C on each keeps both curves on it
    // whichever measurement the pane follows.
    d.key("C");
    d.send(Msg::SelectMeas(first.id));
    d.key("C");
    assert_eq!(d.st.compared_meas.len(), 2);

    let theme = Theme::dark();
    let size = ac2_scene::primitives::Viewport {
        width: 1200.0,
        height: 600.0,
    };
    let now = || ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let names = |s: &AppState| -> Vec<String> {
        ac2_ui::scenes::transfer(
            s,
            crate::common::pane(s, ac2_ui::state::PaneKind::Transfer),
            &theme,
            size,
            now(),
        )
        .legend
        .iter()
        .map(|e| e.name.clone())
        .collect()
    };
    d.until("both curves", |s| names(s).len() == 2)?;

    // Select the first in the list; A hides its curve.
    d.send(Msg::SelectMeas(first.id));
    d.key("A");
    assert_eq!(names(&d.st), std::slice::from_ref(&second.config.name));
    let row =
        d.st.meas_rows()
            .into_iter()
            .find(|r| r.id == first.id)
            .ok_or("row")?;
    assert!(row.hidden && row.text.contains("hidden"), "{}", row.text);
    assert!(
        d.st.pane_caption(crate::common::pane(&d.st, PaneKind::Transfer))
            .is_some_and(|c| c.starts_with(&format!("{} hidden", first.config.name)))
    );
    // Hidden, it keeps running and its frames keep coming.
    let seq = |s: &AppState| {
        s.data.as_ref().and_then(|x| {
            x.latest
                .get(&Topic::Data {
                    meas: first.id,
                    stream: Stream::Tf,
                })
                .map(|f| f.frame.stamp.seq)
        })
    };
    let before = seq(&d.st);
    d.until("a newer frame of the hidden measurement", |s| {
        seq(s) > before
    })?;
    assert!(
        d.st.daemon()
            .and_then(|x| x.measurements.iter().find(|m| m.id == first.id))
            .is_some_and(|m| m.running)
    );
    d.key("A");
    assert_eq!(names(&d.st).len(), 2);
    d.key("A");

    // Backspace asks first; Enter deletes it.
    d.key("Backspace");
    assert!(
        matches!(&d.st.overlay, Overlay::Delete(p) if p.target == DeleteTarget::Meas(first.id)),
        "{:?}",
        d.st.overlay
    );
    d.key("Enter");
    assert_eq!(d.st.overlay, Overlay::None);
    d.until("the first measurement gone", |s| {
        s.measurements().len() == 1 && s.meas(first.id).is_none()
    })?;
    assert!(d.st.hidden_meas.is_empty());
    assert!(d.st.meas(second.id).is_some_and(|m| m.running));
    d.until("the other curve alone", |s| {
        names(s) == [second.config.name.clone()]
    })?;
    d.stop()?;
    drop(d);
    drop(daemon);
    Ok(())
}

#[test]
fn a_remembered_level_axis_still_fits_a_started_spectrum() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    let remembered = ac2_scene::axis::Range::new(-160.0, -150.0);
    let mut prefs = crate::common::grid_ui_prefs();
    prefs.levels.spectrum_dbfs = remembered;
    let prefs = ac2_ui::prefs::UiPrefs::from_toml(&prefs.to_toml())?;
    d.st.set_prefs(prefs);
    assert_eq!(d.st.view.spectrum.level, remembered);
    measure_from_empty(&mut d)?;
    assert_eq!(d.st.view.spectrum.level, remembered);
    d.key("Ctrl+K");
    d.send(Msg::Text("new spectrum".into()));
    d.key("Enter");
    d.until(
        "the spectrum dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Spectrum),
    )?;
    d.key("Enter");
    d.until("the started spectrum fitted", |s| {
        s.view.spectrum.level != remembered
    })?;
    let fit = d.st.view.spectrum.level;
    assert!(fit.is_valid() && fit.hi > -150.0, "{fit:?}");
    // The fit is what the next start comes back to.
    assert_eq!(d.st.prefs.levels.spectrum_dbfs, fit);
    drop(d);
    drop(daemon);
    Ok(())
}

/// Types `text` into the open prompt in place of what it holds.
fn retype(d: &mut Driver, text: &str) {
    while matches!(&d.st.overlay, Overlay::Prompt(p) if !p.text.is_empty()) {
        d.send(Msg::Backspace);
    }
    d.send(Msg::Text(text.into()));
}

/// The selected stored trace exported from the palette: to a folder (under its own name),
/// then to a typed file in the folder the prompt then starts in; then A − B of an unslotted
/// selected trace and the next shown one.
#[test]
fn the_selected_trace_exports_and_subtracts_from_the_palette() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    d.key("Ctrl+1");
    d.until("slot 1", |s| s.slots()[0].is_some())?;
    let a = d.st.slots()[0].map(|t| t.id).ok_or("slot 1")?;
    let name = d.st.slots()[0].map(|t| t.edit.name.clone()).ok_or("name")?;
    let dir = tempfile::tempdir()?;
    let export = |d: &mut Driver| {
        d.key("Ctrl+K");
        d.send(Msg::Text("export the selected".into()));
        d.key("Enter");
    };

    // Nothing selected: it says so.
    d.send(Msg::Command(CommandId::SelectLive));
    export(&mut d);
    assert!(!matches!(d.st.overlay, Overlay::Prompt(_)));
    assert!(
        d.st.toasts
            .iter()
            .any(|t| t.severity != Severity::Info && t.text.contains("select"))
    );

    d.key("V");
    assert_eq!(d.st.selected_trace, Some(a));
    export(&mut d);
    let Overlay::Prompt(p) = &d.st.overlay else {
        return Err("the export prompt".into());
    };
    assert_eq!(p.kind, PromptKind::TraceExport(a));
    // It starts in the home directory.
    let home = std::env::home_dir().ok_or("home")?.display().to_string();
    assert_eq!(p.text, format!("{home}{}", std::path::MAIN_SEPARATOR));
    retype(&mut d, &dir.path().display().to_string());
    d.key("Enter");
    let file = dir.path().join(format!("{name}.csv"));
    d.until("the export written", |s| {
        s.toasts
            .iter()
            .any(|t| t.severity == Severity::Info && t.text.contains("exported to"))
    })?;
    let csv = std::fs::read_to_string(&file)?;
    assert!(csv.starts_with("# ac2 trace export"), "{csv:.80}");
    assert!(csv.contains(&format!("# name: {name}\n")), "{csv:.80}");
    assert!(csv.lines().filter(|l| !l.starts_with('#')).count() > 100);

    // The next export starts in that folder; a typed name is the file.
    export(&mut d);
    let Overlay::Prompt(p) = &d.st.overlay else {
        return Err("the export prompt".into());
    };
    let folder = format!("{}{}", dir.path().display(), std::path::MAIN_SEPARATOR);
    assert_eq!(p.text, folder);
    d.send(Msg::Text("front fill.csv".into()));
    d.key("Enter");
    let named = dir.path().join("front fill.csv");
    d.until("the second export written", |s| {
        s.toasts
            .iter()
            .any(|t| t.text.contains("front fill.csv") && t.text.contains("bytes"))
    })?;
    assert_eq!(std::fs::read_to_string(&named)?, csv);

    // A relative path is relative to the last export's folder.
    export(&mut d);
    retype(&mut d, "rel.csv");
    d.key("Enter");
    d.until("the relative export written", |s| {
        s.toasts.iter().any(|t| t.text.contains("rel.csv"))
    })?;
    assert_eq!(std::fs::read_to_string(dir.path().join("rel.csv"))?, csv);

    // A folder that is not there: the error names the path.
    export(&mut d);
    retype(&mut d, &dir.path().join("gone/x.csv").display().to_string());
    d.key("Enter");
    d.until("the write error", |s| {
        s.toasts.iter().any(|t| {
            t.severity != Severity::Info
                && t.text.contains("cannot write")
                && t.text.contains("gone")
        })
    })?;

    // A math channel of an unslotted stored trace: Shift+M starts with the selected trace
    // as A, by name.
    d.key("Ctrl+2");
    d.until("slot 2", |s| s.slots()[1].is_some())?;
    let b = d.st.slots()[1].map(|t| t.id).ok_or("slot 2")?;
    d.send(Msg::SelectTrace(b));
    d.key("Ctrl+K");
    d.send(Msg::Text("move the selected trace to slot".into()));
    d.key("Enter");
    retype(&mut d, "none");
    d.key("Enter");
    d.until("slot 2 freed", |s| {
        s.daemon()
            .is_some_and(|x| x.traces.iter().any(|t| t.id == b && t.edit.slot.is_none()))
    })?;
    let b_name =
        d.st.selected_trace_meta()
            .map(|t| t.edit.name.clone())
            .ok_or("b")?;
    d.key("Shift+M");
    let Overlay::Form(f) = &d.st.overlay else {
        return Err(format!("{:?}", d.st.overlay).into());
    };
    let name = f.text(ac2_ui::forms::FieldId::Name).to_owned();
    assert!(name.starts_with(&format!("{b_name} ÷ ")), "{name}");
    d.key("Enter");
    let operand = ac2_proto::model::Operand::Trace { trace: b };
    d.until("the math channel of the trace", |s| {
        s.measurements().iter().any(|m| {
            m.config.name == name
                && matches!(&m.config.kind, MeasKind::Math { config } if config.expr.names(operand))
        })
    })?;
    drop(d);
    drop(daemon);
    Ok(())
}

/// The streams of `meas` the app holds frames of now.
fn streams_of(s: &AppState, meas: MeasId) -> Vec<Stream> {
    s.data
        .as_ref()
        .map(|d| {
            d.latest
                .frames
                .values()
                .filter_map(|f| match f.topic {
                    Topic::Data { meas: m, stream } if m == meas => Some(stream),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// From an empty daemon: the app receives the streams its panes draw and no others. The IR
/// arrives while its pane is shown and stops when it is closed (the daemon derives it only
/// for subscribers); a maximised spectrum pane drops the transfer function; the
/// measurement's input levels, which no pane draws, never arrive.
#[test]
fn subscriptions_follow_the_panes_from_an_empty_daemon() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let m = d.st.selected_meas().cloned().ok_or("measurement")?.id;
    // The level typed there stays: arm and fire.
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|x| x.generator.firing))?;
    let has = |s: &AppState, st: Stream| streams_of(s, m).contains(&st);
    d.until("the IR, its pane shown", |s| {
        has(s, Stream::Tf) && has(s, Stream::Ir)
    })?;
    assert!(!has(&d.st, Stream::Levels));

    d.key("Alt+3");
    d.key("Q");
    assert!(d.st.layout.views.values().all(|v| !v.shows_ir()));
    d.until("no IR, its pane hidden", |s| {
        has(s, Stream::Tf) && !has(s, Stream::Ir)
    })?;
    // Still none a while later: nothing subscribes to it.
    let end = Instant::now() + Duration::from_millis(500);
    while Instant::now() < end {
        d.pump();
        assert!(!has(&d.st, Stream::Ir));
        std::thread::sleep(Duration::from_millis(20));
    }

    // The spectrum pane alone: no transfer function either.
    d.key("Alt+2");
    d.key("W");
    assert!(d.st.layout.maximized);
    d.until("nothing of the transfer measurement", |s| {
        streams_of(s, m).is_empty()
    })?;
    // Back to the panes (W cycles maximised, full screen, back), and the IR shown again:
    // both arrive again.
    d.key("W");
    d.key("W");
    assert!(!d.st.layout.maximized);
    // A new pane beside the transfer pane, stepped (G) to its IR view.
    d.key("Alt+1");
    d.key("N");
    for _ in 0..3 {
        d.key("G");
    }
    assert!(d.st.layout.focused().shows_ir());
    d.until("the TF and the IR again", |s| {
        has(s, Stream::Tf) && has(s, Stream::Ir)
    })?;
    assert!(!has(&d.st, Stream::Levels));
    d.stop()?;
    drop(d);
    drop(daemon);
    Ok(())
}

/// The spectrograph from an empty daemon: a spectrum from the palette, G shows the
/// spectrograph under it, the rig's noise fills it from the frames, the cursor reads a level
/// back, Shift+G changes the history length, a stopped measurement says so and G hides it
/// and lets the history go.
#[test]
fn spectrograph_from_an_empty_daemon() -> R {
    use ac2_scene::primitives::HeatmapAxes;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    d.key("Ctrl+K");
    d.send(Msg::Text("new spectrum".into()));
    d.key("Enter");
    d.until(
        "the spectrum dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Spectrum),
    )?;
    d.key("Enter");
    d.until("the spectrum measurement, running", |s| {
        s.measurements()
            .iter()
            .any(|m| matches!(m.config.kind, MeasKind::Spectrum { .. }) && m.running)
    })?;
    let (sp, name) =
        d.st.measurements()
            .iter()
            .find(|m| matches!(m.config.kind, MeasKind::Spectrum { .. }))
            .map(|m| (m.id, m.config.name.clone()))
            .ok_or("spectrum")?;
    d.key("Alt+2");
    assert!(
        hint_line(&d.st).iter().any(|h| h == "G/Shift+G view"),
        "{:?}",
        hint_line(&d.st)
    );
    d.key("G");
    assert_eq!(
        d.st.kind_modes(ac2_ui::state::PaneKind::Spectrum).spectrum,
        SpectrumMode::Split
    );
    // The level typed for the transfer measurement is still set: arm and fire.
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|d| d.generator.firing))?;
    let filled = |s: &AppState, n: usize| {
        s.spectrographs
            .get(&sp)
            .is_some_and(|h| h.ring().iter().flatten().count() >= n)
    };
    // A second of history (30 slots a second at 30 s).
    d.until("a second of spectrograph", |s| filled(s, 30))?;

    let theme = Theme::dark();
    let size = ac2_scene::primitives::Viewport {
        width: 1000.0,
        height: 600.0,
    };
    let now = || ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let s = ac2_ui::scenes::spectrograph(
        &d.st,
        crate::common::pane(&d.st, ac2_ui::state::PaneKind::Spectrum),
        &theme,
        size,
        now(),
    );
    assert_eq!(s.caption, format!("{name} · last 30 s · dBFS"));
    assert_eq!(s.message, None);
    let history = s
        .scene
        .layers
        .iter()
        .flat_map(|l| &l.heatmaps)
        .find(|h| h.axes == HeatmapAxes::TimeUp)
        .ok_or("the history in the scene")?;
    assert!(history.data.iter().flatten().count() >= 30);
    // Its colours span the pane's level axis, the one the level keys move.
    let r = d.st.view.spectrum.level;
    assert_eq!(history.range, [r.lo as f32, r.hi as f32]);
    // The rig's noise at 1 kHz, just now.
    d.send(Msg::SpectrographCursor {
        hz: 1000.0,
        before_s: 0.1,
    });
    let s = ac2_ui::scenes::spectrograph(
        &d.st,
        crate::common::pane(&d.st, ac2_ui::state::PaneKind::Spectrum),
        &theme,
        size,
        now(),
    );
    let text = s.cursor.ok_or("cursor")?.text;
    assert!(
        text.starts_with("1.00 kHz · 0.1 s ago · ") && text.ends_with(" dBFS"),
        "{text}"
    );
    // The cursor toggle turns it off, time and all.
    d.send(Msg::Command(CommandId::ToggleCursor));
    assert_eq!(d.st.view.spectrum.spectrograph.cursor_s, None);

    // A minute of history (Shift+B, Settings › Display), started afresh.
    d.send(Msg::Command(CommandId::SpectrographSpan));
    assert_eq!(d.st.view.spectrum.spectrograph.span_s, 60);
    assert!(!filled(&d.st, 1));
    d.until("frames in the minute", |s| filled(s, 10))?;
    let s = ac2_ui::scenes::spectrograph(
        &d.st,
        crate::common::pane(&d.st, ac2_ui::state::PaneKind::Spectrum),
        &theme,
        size,
        now(),
    );
    assert!(s.caption.contains("last 60 s"), "{}", s.caption);

    // Stopped: the picture stays and says so.
    d.stop()?;
    d.key("S");
    d.until("the spectrum stopped", |s| {
        s.measurements().iter().any(|m| m.id == sp && !m.running)
    })?;
    let s = ac2_ui::scenes::spectrograph(
        &d.st,
        crate::common::pane(&d.st, ac2_ui::state::PaneKind::Spectrum),
        &theme,
        size,
        now(),
    );
    assert!(s.caption.ends_with(" · stopped"), "{}", s.caption);
    assert!(filled(&d.st, 10));
    // G: the spectrograph alone, its history kept; W makes it the only pane, full size.
    d.key("G");
    assert_eq!(
        d.st.kind_modes(ac2_ui::state::PaneKind::Spectrum).spectrum,
        SpectrumMode::Spectrograph
    );
    assert!(filled(&d.st, 10));
    d.key("W");
    assert_eq!(
        crate::common::visible(&d.st),
        [ac2_ui::state::PaneKind::Spectrum]
    );
    let alone = ac2_ui::scenes::spectrograph(
        &d.st,
        crate::common::pane(&d.st, ac2_ui::state::PaneKind::Spectrum),
        &theme,
        size,
        now(),
    );
    assert!(alone.spectrum.is_none());
    assert!(
        alone.plot.h > s.plot.h * 1.5,
        "{:?} vs {:?}",
        alone.plot,
        s.plot
    );
    assert!(
        alone
            .caption
            .starts_with(&format!("{name} · last 60 s · dBFS · stopped · ")),
        "{}",
        alone.caption
    );
    // The level keys still move the colours.
    let before = d.st.view.spectrum.level;
    d.key("Ctrl+I");
    assert_ne!(d.st.view.spectrum.level, before);
    d.key("W");
    // G again hides it and nothing is kept.
    d.key("G");
    assert_eq!(
        d.st.kind_modes(ac2_ui::state::PaneKind::Spectrum).spectrum,
        SpectrumMode::Spectrum
    );
    assert!(d.st.spectrographs.is_empty());
    drop(d);
    drop(daemon);
    Ok(())
}

/// From an empty daemon: measure, then record from the palette while the noise plays — the
/// top bar shows REC with the audio's length and size, then what was recorded — and replay
/// the recording from the palette: the session plays the file, the bar says so, and the
/// transfer function shows the rig's path again from the recorded inputs.
#[test]
fn record_and_replay_from_the_app() -> R {
    let dir = tempfile::tempdir()?;
    let listen = ac2d::Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    };
    let audio = ac2d::backend(ac2d::BackendChoice::Fake)?;
    let mut config = ac2d::DaemonConfig::new(audio, listen, -10.0);
    config.session_dir = dir.path().join("sessions");
    config.recording_dir = Some(dir.path().join("recordings"));
    let handle = ac2d::Daemon::start(config)?;
    let ep = Endpoints {
        ctrl: handle.ctrl_endpoint().to_owned(),
        data: handle.data_endpoint().to_owned(),
    };
    let mut d = Driver::connect(ClientConfig::new(ep, NAME), "local daemon")?;
    measure_from_empty(&mut d)?;
    assert_eq!(d.st.recording_label(), None, "nothing recorded yet");
    let meas = d.st.selected_meas().map(|m| m.id).ok_or("measurement")?;

    // The level typed for the first run stands: arm and fire.
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|x| x.generator.firing))?;
    d.key("Ctrl+K");
    d.send(Msg::Text("record".into()));
    d.key("Enter");
    d.until("two seconds recorded", |s| {
        s.daemon()
            .and_then(|x| x.recording.as_ref())
            .is_some_and(|r| r.active() && r.frames >= 2 * u64::from(r.sample_rate_hz))
    })?;
    let rec = d.st.recording_label().ok_or("no indicator")?;
    assert!(rec.text.starts_with("REC 0:0"), "{}", rec.text);
    assert!(
        rec.text.contains(" kB") || rec.text.contains(" MB"),
        "{}",
        rec.text
    );
    assert_eq!(rec.tone, ac2_scene::recording::RecordingTone::Recording);
    assert!(rec.detail.contains("Room mic"), "{}", rec.detail);

    d.key("Ctrl+K");
    d.send(Msg::Text("record".into()));
    d.key("Enter");
    d.until("the recording ended", |s| {
        s.daemon()
            .and_then(|x| x.recording.as_ref())
            .is_some_and(|r| !r.active())
    })?;
    d.stop()?;
    let run =
        d.st.daemon()
            .and_then(|x| x.recording.clone())
            .ok_or("recording")?;
    let label = d.st.recording_label().ok_or("no indicator")?;
    assert!(
        label
            .text
            .starts_with(&format!("recorded {} · 0:0", run.name)),
        "{}",
        label.text
    );

    d.key("Ctrl+K");
    d.send(Msg::Text("replay".into()));
    d.key("Enter");
    d.send(Msg::Text(run.name.clone()));
    d.key("Enter");
    d.until("the replay session", |s| {
        s.open_session().is_some_and(|o| o.replay.is_some())
    })?;
    let replay = d.st.replay_label().ok_or("no replay label")?;
    assert!(
        replay.starts_with(&format!("REPLAY {} · 0:0", run.name)),
        "{replay}"
    );
    let epoch = d.st.daemon().map(|x| x.session.epoch).ok_or("state")?;
    let topic = Topic::Data {
        meas,
        stream: Stream::Tf,
    };
    d.until("the −6 dB path at 1 kHz from the recording", |s| {
        s.data.as_ref().is_some_and(|x| {
            x.latest.get(&topic).is_some_and(|f| {
                f.frame.stamp.session_epoch == epoch
                    && match &f.frame.data {
                        FrameData::Tf(tf) => tf
                            .mag
                            .get(240)
                            .is_some_and(|m| m.is_finite() && (m - (-6.02)).abs() < 0.5),
                        _ => false,
                    }
            })
        })
    })?;
    drop(d);
    handle.shutdown();
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
    for i in 1..=4 {
        d.key("Tab");
        let (id, pane) = order[i % 3];
        assert_eq!(d.st.selected, Some(id), "Tab {i}");
        assert_eq!(d.st.selected_trace, None);
        assert_eq!(d.st.layout.focus, f, "Tab {i}");
        assert_eq!(d.st.layout.focus_kind(), pane, "Tab {i}");
        assert_eq!(d.st.pane_meas(f).map(|m| m.id), Some(id), "Tab {i}");
    }
    // On the spectrum now; back round once, by the SPL meter.
    for want in [tf, spl, sp] {
        d.key("Shift+Tab");
        assert_eq!(d.st.selected, Some(want));
    }
    assert_eq!(d.st.layout.focus_kind(), Spectrum);
    drop(d);
    drop(daemon);
    Ok(())
}
