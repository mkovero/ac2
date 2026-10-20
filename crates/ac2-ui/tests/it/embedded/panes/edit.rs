//! Zoom, hide and delete the selected measurement, a remembered level axis, and export and
//! subtract of the selected stored trace.

use crate::embedded::harness::{Driver, NAME, R, measure_from_empty};
use ac2_proto::model::MeasKind;
use ac2_proto::topic::{Stream, Topic};
use ac2_scene::theme::Theme;
use ac2_ui::embedded::{EmbeddedBackend, Setup, start_embedded_with};
use ac2_ui::forms::FormKind;
use ac2_ui::keys::CommandId;
use ac2_ui::state::{AppState, Msg, Overlay, PaneKind, PromptKind, Severity, StimPhase};
use std::time::Instant;

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
    let level = |s: &AppState| crate::common::spectrum_level(s);
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
            .list
            .iter()
            .any(|t| t.text.starts_with("Spectrum / RTA: level"))
    );
    d.key("Ctrl+Home");
    assert_eq!(level(&d.st), start);

    // Delete slot 1: asked first, then gone; the selection moves to slot 2. (Slot 2 is
    // still selected, the focus keys leaving it: V steps round to slot 1.)
    d.key("Alt+1");
    for _ in 0..3 {
        if d.st.selected_trace == Some(a) {
            break;
        }
        d.key("V");
    }
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
/// running and measuring, A shows it again; Backspace asks before it goes and Backspace
/// again deletes it (Enter does not), the other measuring on.
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
    // The transfer pane draws one measurement's group; each compared into the other (C,
    // Enter on the one target) keeps both curves on it whichever measurement it follows.
    d.key("C");
    d.key("Enter");
    d.key("Escape");
    d.send(Msg::SelectMeas(first.id));
    d.key("C");
    d.key("Enter");
    d.key("Escape");
    d.until("each compared into the other", |s| {
        [first.id, second.id]
            .iter()
            .all(|id| s.meas(*id).is_some_and(|m| m.compared.len() == 1))
    })?;

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

    // Backspace asks first; Enter, the stimulus's key, deletes nothing; Backspace again does.
    d.key("Backspace");
    assert!(
        matches!(&d.st.overlay, Overlay::Delete(p) if p.target == DeleteTarget::Meas(first.id)),
        "{:?}",
        d.st.overlay
    );
    d.key("Enter");
    assert!(matches!(&d.st.overlay, Overlay::Delete(_)));
    d.key("Backspace");
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
    let spectrum_pane = |p: &ac2_ui::prefs::UiPrefs| {
        let panes = p.layout.panes.as_ref().expect("panes");
        let v = panes.views.iter().find(|v| v.kind == PaneKind::Spectrum);
        v.expect("a spectrum pane").levels
    };
    let as_level =
        |r: ac2_scene::axis::Range| ac2_proto::model::layout::LevelRange { lo: r.lo, hi: r.hi };
    if let Some(v) = prefs
        .layout
        .panes
        .as_mut()
        .and_then(|p| p.views.iter_mut().find(|v| v.kind == PaneKind::Spectrum))
    {
        v.levels.spectrum_dbfs = Some(as_level(remembered));
    }
    let prefs = ac2_ui::prefs::UiPrefs::from_toml(&prefs.to_toml())?;
    d.st.set_prefs(prefs);
    let level = |s: &AppState| crate::common::spectrum_level(s);
    assert_eq!(level(&d.st), remembered);
    measure_from_empty(&mut d)?;
    assert_eq!(level(&d.st), remembered);
    d.key("Ctrl+K");
    d.send(Msg::Text("new spectrum".into()));
    d.key("Enter");
    d.until(
        "the spectrum dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Spectrum),
    )?;
    d.key("Enter");
    d.until("the started spectrum fitted", |s| level(s) != remembered)?;
    let fit = level(&d.st);
    assert!(fit.is_valid() && fit.hi > -150.0, "{fit:?}");
    // The fit is the app's, not the operator's: not remembered, the next start frames the
    // pane's first curve again.
    assert_eq!(spectrum_pane(&d.st.prefs).spectrum_dbfs, None);
    drop(d);
    drop(daemon);
    Ok(())
}

/// From an empty daemon: a new spectrum's pane shows its first frames on a fitted level
/// axis (its labels are the fit's, not the default's), and frames after that leave it.
#[test]
fn a_new_spectrum_pane_shows_a_fitted_level_axis() -> R {
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
    let default = ac2_scene::view::ViewState::default().spectrum.level;
    let level = |s: &AppState| crate::common::spectrum_level(s);
    d.until("the spectrum's level axis fitted", |s| {
        s.measurements()
            .iter()
            .any(|m| matches!(m.config.kind, MeasKind::Spectrum { .. }))
            && level(s) != default
    })?;
    let fit = level(&d.st);
    // The labels the pane draws span the fitted range.
    let labels = |s: &AppState| -> Vec<f64> {
        let pane = ac2_ui::scenes::spectrum(
            s,
            crate::common::pane(s, PaneKind::Spectrum),
            &Theme::dark(),
            ac2_scene::primitives::Viewport {
                width: 1200.0,
                height: 600.0,
            },
            ac2_ui::scenes::Now {
                instant: Instant::now(),
                wall: ac2_proto::units::WallNs(0),
            },
        );
        pane.y_axis
            .labels()
            .into_iter()
            .filter_map(|l| l.replace('\u{2212}', "-").parse().ok())
            .collect()
    };
    let shown = labels(&d.st);
    let (lo, hi) = shown
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), x| {
            (a.min(*x), b.max(*x))
        });
    assert!(lo >= fit.lo && hi <= fit.hi, "{shown:?} on {fit:?}");
    assert_ne!((lo, hi), (default.lo, default.hi), "{shown:?}");
    // Later frames leave the range alone.
    let seq = |s: &AppState| {
        let sp = s
            .measurements()
            .iter()
            .find(|m| matches!(m.config.kind, MeasKind::Spectrum { .. }))
            .map(|m| m.id)?;
        ac2_ui::scenes::frame(s, sp, Stream::Spec).map(|f| f.frame.stamp.seq)
    };
    let first = seq(&d.st);
    d.until("more spectrum frames", |s| seq(s) > first)?;
    assert_eq!(level(&d.st), fit);
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
        d.send(Msg::Text("export trace".into()));
        d.key("Enter");
    };

    // Nothing selected: it says so.
    d.send(Msg::Command(CommandId::SelectLive));
    export(&mut d);
    assert!(!matches!(d.st.overlay, Overlay::Prompt(_)));
    assert!(
        d.st.toasts
            .list
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
            .list
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
            .list
            .iter()
            .any(|t| t.text.contains("front fill.csv") && t.text.contains("bytes"))
    })?;
    assert_eq!(std::fs::read_to_string(&named)?, csv);

    // A relative path is relative to the last export's folder.
    export(&mut d);
    retype(&mut d, "rel.csv");
    d.key("Enter");
    d.until("the relative export written", |s| {
        s.toasts.list.iter().any(|t| t.text.contains("rel.csv"))
    })?;
    assert_eq!(std::fs::read_to_string(dir.path().join("rel.csv"))?, csv);

    // A folder that is not there: the error names the path.
    export(&mut d);
    retype(&mut d, &dir.path().join("gone/x.csv").display().to_string());
    d.key("Enter");
    d.until("the write error", |s| {
        s.toasts.list.iter().any(|t| {
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
    d.send(Msg::Text("move trace to slot".into()));
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
