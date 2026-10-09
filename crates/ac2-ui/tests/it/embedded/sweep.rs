//! Sweep measurements from the app: runs, room parameters, choosing sweeps, the measurement tree.

use super::harness::{
    Driver, NAME, R, arm_new_sweep, measure_from_empty, sweep_count, sweep_from_the_dialog,
};
use ac2_proto::model::MeasKind;
use ac2_proto::topic::Topic;
use ac2_scene::theme::Theme;
use ac2_ui::embedded::{EmbeddedBackend, Setup, start_embedded_with};
use ac2_ui::forms::FormKind;
use ac2_ui::keys::CommandId;
use ac2_ui::state::{AppState, Msg, Overlay, StimPhase};
use std::time::{Duration, Instant};

/// From an empty daemon to a stored sweep using only the app, on the simulated rig whose
/// "speaker" distorts (H2 −40 dB, H3 −50 dB at −20 dBFS): the session from the dialog,
/// Shift+S, a typed level, Enter arms, Enter plays, the result opens the distortion pane
/// with the rig's harmonics, and the stimulus is off again: nothing re-arms after a sweep.
#[test]
fn empty_embedded_daemon_sweeps_from_the_app() -> R {
    use ac2_scene::distortion::{Reading, reading_at};
    use ac2_ui::forms::FieldId;
    use ac2_ui::state::PaneKind;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;

    let sweeps_before = sweep_count(&d.st);
    d.key("Shift+S");
    d.send(Msg::Text("S".into()));
    d.until(
        "the sweep dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Sweep),
    )?;
    if let Overlay::Form(f) = &mut d.st.overlay {
        assert_eq!(f.channel(FieldId::Reference), Some(0));
        assert_eq!(f.channel(FieldId::Measurement), Some(1));
        // A short sweep over the band the rig's harmonics stay below Nyquist in.
        f.set_text(FieldId::Level, "-20");
        f.set_text(FieldId::From, "100 Hz");
        f.set_text(FieldId::To, "5 kHz");
        f.focus = f
            .fields
            .iter()
            .position(|x| x.id == FieldId::Duration)
            .ok_or("duration")?;
        f.cycle(-1);
        assert_eq!(f.fields[f.focus].display(), "1 s (quick look)");
    }
    d.key("Enter");
    arm_new_sweep(&mut d, sweeps_before)?;
    d.until("armed with the sweep", |s| {
        s.stimulus.phase == StimPhase::Armed
            && s.daemon().is_some_and(|x| {
                x.generator.armed
                    && x.generator
                        .settings
                        .as_ref()
                        .is_some_and(|g| matches!(g.signal, ac2_proto::model::Signal::Ess { .. }))
            })
    })?;
    d.key("Enter");
    d.until("the sweep stored and shown", |s| {
        s.sweep.run.is_none()
            && s.layout.focus_kind() == PaneKind::Distortion
            && s.shown_sweep().is_some()
    })?;
    let (data, grid) = d.st.shown_sweep().ok_or("no sweep")?;
    let freqs = ac2_scene::grid::column_frequencies(grid);
    let s = data.sweep.as_ref().ok_or("no sweep data")?;
    let m = s.info.floor_margin.0;
    for (order, want) in [(2u8, -40.0), (3, -50.0)] {
        let h = s
            .harmonics
            .iter()
            .find(|h| h.order == order)
            .ok_or("order")?;
        match reading_at(&h.curve, &freqs, 1000.0, m) {
            Reading::Level(v) => assert!((v - want).abs() < 1.0, "H{order} at 1 kHz: {v}"),
            other => return Err(format!("H{order} at 1 kHz: {other:?}").into()),
        }
    }
    // The cursor at 1 kHz (a click in the pane) reads every order there, in dB and in %;
    // the IR view (G) keeps its own time cursor.
    d.send(Msg::CursorAt(Some(1000.0)));
    let rows = |d: &Driver| -> R<Vec<(String, String)>> {
        let now = ac2_ui::scenes::Now {
            instant: Instant::now(),
            wall: ac2_proto::units::WallNs(0),
        };
        let size = ac2_scene::primitives::Viewport {
            width: 1100.0,
            height: 600.0,
        };
        match ac2_ui::scenes::sweep(
            &d.st,
            crate::common::pane(&d.st, ac2_ui::state::PaneKind::Distortion),
            &Theme::dark(),
            size,
            now,
        ) {
            ac2_ui::scenes::SweepPane::Distortion(s) => {
                Ok(s.cursor.ok_or("no distortion cursor")?.rows)
            }
            _ => Err("not the distortion view".into()),
        }
    };
    let value = |rows: &[(String, String)], n: &str| {
        rows.iter()
            .find(|r| r.0 == n)
            .map(|r| r.1.clone())
            .unwrap_or_default()
    };
    let db = rows(&d)?;
    let h2: f64 = value(&db, "H2")
        .trim_end_matches(" dB")
        .replace('\u{2212}', "-")
        .parse()?;
    assert!((h2 + 40.0).abs() < 1.0, "{db:?}");
    assert!(value(&db, "THD").ends_with(" dB"), "{db:?}");
    d.key("U");
    let pc = rows(&d)?;
    let h2: f64 = value(&pc, "H2").trim_end_matches(" %").parse()?;
    assert!((h2 - 1.0).abs() < 0.15, "{pc:?}");
    d.key("U");
    d.key("G");
    assert_eq!(
        d.st.ir_target(),
        Some(ac2_scene::view::IrPane::Sweep),
        "the sweep's IR view"
    );
    d.send(Msg::Command(CommandId::ToggleCursor));
    assert!(d.st.view.distortion.ir.cursor_ms.is_some());
    assert_eq!(
        d.st.view.cursor_hz,
        Some(1000.0),
        "the frequency cursor stays"
    );
    d.key("G");
    d.key("G");
    d.until("the stimulus off and the lease given back", |s| {
        s.stimulus.phase == StimPhase::Idle
            && s.daemon().is_some_and(|x| {
                !x.generator.armed && !x.generator.firing && x.generator.owner.is_none()
            })
    })?;
    assert!(!d.st.stimulus_live(), "STIM OFF");
    assert!(d.st.sweep.plan.is_none());
    drop(d);
    drop(daemon);
    Ok(())
}

/// Room parameters from the app: from an empty daemon on the simulated rig, the session
/// dialog takes the rig's hall mic (in 3) as a second mic, a sweep plays to it with 2 s of
/// silence after it, and the sweep's impulse response (Shift+I) carries the octave table
/// with the hall's reverberation time (0.8 s), every string from `ac2_scene::room`.
#[test]
fn slow_room_parameters_of_a_sweep_from_the_app() -> R {
    use ac2_ui::forms::FieldId;
    use ac2_ui::session_dialog::Row;
    use ac2_ui::state::PaneKind;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    d.synced()?;
    d.key("Shift+O");
    d.send(Msg::Text("O".into()));
    d.until("the meters of the device", |s| {
        s.overlay
            .settings()
            .is_some_and(|x| x.session.device_info().is_some())
            && s.input_meters().len() == 4
    })?;
    d.send(Msg::Session(ac2_ui::state::SessionMsg::Focus(Row::Input(
        2,
    ))));
    d.key("M");
    d.key("Enter");
    d.until("the session", |s| s.open_session().is_some())?;
    let open = d.st.open_session().cloned().ok_or("session")?;
    assert_eq!(open.config.input_channels, vec![0, 1, 2]);
    d.until("the measurement offer", |s| {
        matches!(s.overlay, Overlay::Offer(_))
    })?;
    d.key("Escape");

    let sweeps_before = sweep_count(&d.st);
    d.key("Shift+S");
    d.send(Msg::Text("S".into()));
    d.until(
        "the sweep dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Sweep),
    )?;
    if let Overlay::Form(f) = &mut d.st.overlay {
        assert!(f.set_channel(FieldId::Measurement, 2), "the hall mic");
        f.set_text(FieldId::Level, "-20");
        f.set_text(FieldId::From, "100 Hz");
        f.set_text(FieldId::To, "10 kHz");
        let at = |f: &ac2_ui::forms::Form, id| f.fields.iter().position(|x| x.id == id);
        f.focus = at(f, FieldId::Duration).ok_or("duration")?;
        f.cycle(-1);
        f.focus = at(f, FieldId::Tail).ok_or("tail")?;
        assert_eq!(f.fields[f.focus].display(), "1 s (small rooms)");
        f.cycle(1);
        assert_eq!(f.fields[f.focus].display(), "2 s");
    }
    d.key("Enter");
    arm_new_sweep(&mut d, sweeps_before)?;
    d.until("armed with the sweep", |s| {
        s.stimulus.phase == StimPhase::Armed && s.daemon().is_some_and(|x| x.generator.armed)
    })?;
    d.key("Enter");
    d.until("the sweep stored and shown", |s| {
        s.sweep.run.is_none()
            && s.layout.focus_kind() == PaneKind::Distortion
            && s.shown_sweep().is_some()
    })?;
    d.key("Shift+I");
    assert_eq!(
        d.st.kind_modes(ac2_ui::state::PaneKind::Distortion).sweep,
        ac2_scene::view::SweepMode::Ir
    );
    let theme = Theme::dark();
    let size = ac2_scene::primitives::Viewport {
        width: 1100.0,
        height: 600.0,
    };
    let now = ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let ac2_ui::scenes::SweepPane::Ir(ir) = ac2_ui::scenes::sweep(
        &d.st,
        crate::common::pane(&d.st, ac2_ui::state::PaneKind::Distortion),
        &theme,
        size,
        now,
    ) else {
        return Err("the sweep pane does not show the impulse response".into());
    };
    let t = ir.room.as_ref().ok_or("no room table")?;
    assert!(
        t.caption
            .starts_with("Room (ISO 3382-1) · octave bands · decay to "),
        "{}",
        t.caption
    );
    assert_eq!(t.bands, ["250", "500", "1k", "2k", "4k", "All"]);
    let row = |p| t.rows.iter().find(|r| r.param == p).ok_or("row");
    for p in [ac2_scene::room::Param::T20, ac2_scene::room::Param::T30] {
        for c in &row(p)?.cells {
            let v: f64 = c.text.trim_end_matches('*').parse()?;
            assert!((v / 0.8 - 1.0).abs() < 0.1, "{} {}: {v}", p.name(), c.text);
        }
    }
    // The table is drawn: its strings are in the scene.
    let labels: Vec<&str> = ir
        .scene
        .layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|x| x.text.as_str()))
        .collect();
    assert!(labels.contains(&"T30 (s)") && labels.contains(&t.caption.as_str()));

    // G: the room parameters alone, the whole pane, every band at a larger size.
    d.key("G");
    assert_eq!(
        d.st.kind_modes(ac2_ui::state::PaneKind::Distortion).sweep,
        ac2_scene::view::SweepMode::Room
    );
    let ac2_ui::scenes::SweepPane::Room(room) = ac2_ui::scenes::sweep(
        &d.st,
        crate::common::pane(&d.st, ac2_ui::state::PaneKind::Distortion),
        &theme,
        size,
        now,
    ) else {
        return Err("the sweep pane does not show the room parameters".into());
    };
    let rt = room.table.as_ref().ok_or("no room table")?;
    assert_eq!(rt.bands, t.bands);
    assert_eq!(rt.rows, t.rows);
    assert!(room.font_size > theme.font_size, "{}", room.font_size);
    let labels: Vec<&str> = room
        .scene
        .layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|x| x.text.as_str()))
        .collect();
    assert!(labels.contains(&"T30 (s)") && labels.contains(&"250"));
    assert!(!labels.iter().any(|x| x.contains("hidden")), "{labels:?}");
    drop(d);
    drop(daemon);
    Ok(())
}

/// From an empty daemon, using only the app: the session's inputs metered by name and role
/// in the sidebar, then a set of two sweeps followed on the progress strip, sweep 1 of 2
/// then 2 of 2, stopped from it: the output stops, the generator is disarmed and the run
/// is discarded.
#[test]
fn input_meters_and_a_stopped_sweep_set_from_the_app() -> R {
    use ac2_proto::model::{SweepFailure, SweepStatus};
    use ac2_scene::meter::{InputUse, MeterState};
    use ac2_ui::forms::FieldId;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;

    // Every captured input, named with its role, metering the rig's signal.
    d.until("the input meters with names", |s| {
        let rows = s.session_inputs();
        rows.len() == 2
            && s.devices.is_some()
            && rows.iter().all(|r| r.reading.state != MeterState::NoData)
    })?;
    let rows = d.st.session_inputs();
    assert!(
        rows[0].label.contains("reference") && rows[0].label.contains("(in 1)"),
        "{}",
        rows[0].label
    );
    assert!(
        rows[1].label.starts_with("Room mic · mic"),
        "{}",
        rows[1].label
    );
    // The selected transfer measurement's inputs are the marked ones.
    assert_eq!(rows[0].used, Some(InputUse::Reference));
    assert_eq!(rows[1].used, Some(InputUse::Measurement));
    assert_eq!(d.st.operation(), None);

    let sweeps_before = sweep_count(&d.st);
    d.key("Shift+S");
    d.send(Msg::Text("S".into()));
    d.until(
        "the sweep dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Sweep),
    )?;
    if let Overlay::Form(f) = &mut d.st.overlay {
        f.set_text(FieldId::Level, "-20");
        f.set_text(FieldId::From, "100 Hz");
        f.set_text(FieldId::To, "5 kHz");
        for (id, step, want) in [
            (FieldId::Duration, -1, "1 s (quick look)"),
            (FieldId::Repeats, 1, "2"),
        ] {
            f.focus = f.fields.iter().position(|x| x.id == id).ok_or("field")?;
            f.cycle(step);
            assert_eq!(f.fields[f.focus].display(), want);
        }
    }
    d.key("Enter");
    arm_new_sweep(&mut d, sweeps_before)?;
    d.until("armed with the sweep", |s| {
        s.stimulus.phase == StimPhase::Armed && s.daemon().is_some_and(|x| x.generator.armed)
    })?;
    d.key("Enter");
    let step = |s: &AppState| s.operation().map(|p| p.step);
    d.until("sweep 1 of 2", |s| {
        step(s).as_deref() == Some("sweep 1 of 2")
    })?;
    let p = d.st.operation().ok_or("progress")?;
    assert_eq!(p.title, "sweep \"Run 1\"");
    assert!(
        p.remaining
            .is_some_and(|r| r.ends_with("left") || r == "finishing")
    );
    // The meters keep running during the sweep.
    let seq = |s: &AppState| {
        s.data
            .as_ref()
            .and_then(|x| x.latest.get(&Topic::SessionLevels))
            .map_or(0, |f| f.frame.stamp.seq)
    };
    let before = seq(&d.st);
    d.until("meter frames during the sweep", |s| seq(s) > before)?;
    d.until("sweep 2 of 2", |s| {
        step(s).as_deref() == Some("sweep 2 of 2")
    })?;

    // The strip's Stop button (the same command as Esc).
    d.send(Msg::Command(CommandId::StimulusStop));
    // The app ends its sweep mode once it has seen the stop, which can be a step after the
    // daemon's state says so: wait for both.
    d.until("stopped and disarmed, the run discarded", |s| {
        s.sweep.plan.is_none()
            && s.operation().is_none()
            && s.daemon().is_some_and(|x| {
                !x.generator.firing
                    && !x.generator.armed
                    && x.sweep.as_ref().is_some_and(|r| {
                        matches!(
                            r.status,
                            SweepStatus::Failed {
                                reason: SweepFailure::Stopped,
                                ..
                            }
                        )
                    })
            })
    })?;
    assert_eq!(d.st.operation(), None);
    assert!(d.st.sweep.plan.is_none());
    drop(d);
    drop(daemon);
    Ok(())
}

/// The selected sweep measurement run again (Space, Enter): its second run, selected.
fn run_again(d: &mut Driver) -> R<ac2_proto::units::TraceId> {
    let before = d.st.sweep_traces().len();
    d.key("Space");
    d.until("armed with the sweep", |s| {
        s.stimulus.phase == StimPhase::Armed
    })?;
    d.key("Enter");
    d.until("the run stored, the stimulus off", |s| {
        s.sweep.run.is_none()
            && s.sweep_traces().len() == before + 1
            && s.stimulus.phase == StimPhase::Idle
            && s.daemon().is_some_and(|x| x.generator.owner.is_none())
    })?;
    d.st.selected_trace
        .ok_or_else(|| "the new run is not selected".into())
}

/// From an empty daemon, two sweeps, then the transfer pane chooses between them: V selects
/// each in turn and the sweep pane follows; A hides one, V skips it; N on the sweep pane
/// selects for the transfer pane too.
#[test]
fn slow_two_sweeps_chosen_between_in_the_transfer_pane() -> R {
    use ac2_ui::state::PaneKind;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let first = sweep_from_the_dialog(&mut d)?;
    let second = run_again(&mut d)?;
    assert_ne!(first, second);
    let shown = |s: &AppState| s.shown_sweep().map(|(t, _)| t.meta.id);
    let name = |s: &AppState, id| {
        s.trace_list()
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.edit.name.clone())
            .unwrap_or_default()
    };
    assert_eq!(shown(&d.st), Some(second));
    let (n1, n2) = (name(&d.st, first), name(&d.st, second));
    assert_ne!(n1, n2);

    // Both in the list, by name, as sweeps; the newest selected.
    let rows = d.st.trace_rows();
    let listed: Vec<(&str, &str, bool, bool)> = rows
        .iter()
        .map(|r| (r.name.as_str(), r.kind, r.shown, r.selected))
        .collect();
    assert_eq!(
        listed,
        [
            (n1.as_str(), "sweep run", true, false),
            (n2.as_str(), "sweep run", true, true)
        ]
    );

    // Live again, the transfer pane focused: V selects the first sweep, then the second;
    // the sweep pane shows whichever is selected.
    d.key("Escape");
    d.key("Alt+1");
    assert_eq!(d.st.layout.focus_kind(), PaneKind::Transfer);
    assert_eq!(d.st.selected_trace, None);
    d.key("V");
    assert_eq!(d.st.selected_trace, Some(first));
    assert_eq!(shown(&d.st), Some(first));
    assert_eq!(
        d.st.pane_caption(crate::common::pane(&d.st, PaneKind::Transfer)),
        Some(format!("{n1}: smoothing off"))
    );
    let theme = Theme::dark();
    let size = ac2_scene::primitives::Viewport {
        width: 1200.0,
        height: 500.0,
    };
    let now = ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let caption = |s: &AppState| match ac2_ui::scenes::sweep(
        s,
        crate::common::pane(s, ac2_ui::state::PaneKind::Distortion),
        &theme,
        size,
        now,
    ) {
        ac2_ui::scenes::SweepPane::Distortion(x) => x.caption.clone(),
        ac2_ui::scenes::SweepPane::Ir(_) | ac2_ui::scenes::SweepPane::Room(_) => String::new(),
    };
    assert!(caption(&d.st).starts_with(&n1), "{}", caption(&d.st));
    d.key("V");
    assert_eq!(d.st.selected_trace, Some(second));
    assert_eq!(shown(&d.st), Some(second));
    assert!(caption(&d.st).starts_with(&n2), "{}", caption(&d.st));

    // A hides the selected sweep: the transfer pane stops drawing it, V skips it.
    d.key("A");
    d.until("the second sweep hidden", |s| {
        s.trace_rows()
            .iter()
            .any(|r| r.id == second && !r.shown && r.details[0].contains("hidden"))
            && s.traces
                .get(&second)
                .is_some_and(|(t, _)| !t.meta.edit.visible)
    })?;
    let legend: Vec<String> = ac2_ui::scenes::transfer(
        &d.st,
        crate::common::pane(&d.st, ac2_ui::state::PaneKind::Transfer),
        &theme,
        size,
        now,
    )
    .legend
    .iter()
    .map(|e| e.name.clone())
    .collect();
    assert!(legend.contains(&n1), "{legend:?}");
    assert!(!legend.contains(&n2), "{legend:?}");
    d.key("V");
    assert_eq!(d.st.selected_trace, None, "past the last shown: live");
    d.key("V");
    assert_eq!(d.st.selected_trace, Some(first));
    d.key("V");
    assert_eq!(d.st.selected_trace, None);
    // Alt+V reaches it hidden; the sweep pane still shows what is selected.
    d.key("Alt+Shift+V");
    assert_eq!(d.st.selected_trace, Some(second));
    assert_eq!(shown(&d.st), Some(second));

    // N on the sweep pane steps the sweeps and selects them for the transfer pane.
    d.key("Alt+5");
    assert_eq!(d.st.layout.focus_kind(), PaneKind::Distortion);
    d.key("N");
    assert_eq!(d.st.selected_trace, Some(first));
    assert_eq!(shown(&d.st), Some(first));
    d.key("Alt+1");
    assert_eq!(
        d.st.selected_trace,
        Some(first),
        "kept on the way to the transfer pane"
    );

    // Shown again from the list's eye.
    d.send(Msg::ToggleShown(second));
    d.until("the second sweep shown again", |s| {
        s.trace_rows().iter().all(|r| r.shown)
    })?;
    drop(d);
    drop(daemon);
    Ok(())
}

/// The measurement tree from an empty daemon, keyboard only: a transfer measurement, two
/// captures filed under it, a math channel made on it; a sweep measurement that plays
/// nothing until Space and Enter, two runs under it; the transfer measurement deleted with
/// its traces kept, which then list under Imported.
#[test]
fn slow_the_measurement_tree_from_an_empty_daemon() -> R {
    use ac2_proto::model::{MathOp, Operand, TraceOwner};
    use ac2_scene::meas_list::TreeKey;
    use ac2_ui::forms::FieldId;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let tf = d.st.selected_meas().map(|m| m.id).ok_or("measurement")?;
    let under_tf = TraceOwner::Meas { meas: tf };

    // Two captures, filed under the measurement they came from.
    d.key("Ctrl+1");
    d.until("slot 1", |s| s.slots()[0].is_some())?;
    d.key("Ctrl+2");
    d.until("slot 2", |s| s.slots()[1].is_some())?;
    let (a, b) = (
        d.st.slots()[0].map(|t| t.id).ok_or("slot 1")?,
        d.st.slots()[1].map(|t| t.id).ok_or("slot 2")?,
    );
    for id in [a, b] {
        assert_eq!(d.st.trace_meta(id)?.edit.owner, under_tf);
    }

    // A math channel made with the measurement selected lives under it.
    d.send(Msg::SelectMeas(tf));
    d.key("Shift+M");
    d.send(Msg::Text("M".into()));
    d.until(
        "the math dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.math.is_some()),
    )?;
    if let Overlay::Form(f) = &mut d.st.overlay {
        assert!(f.pick_operand(FieldId::OperandA, Operand::Trace { trace: a }));
        assert!(f.pick_operand(FieldId::OperandB, Operand::Trace { trace: b }));
    }
    d.key("Enter");
    let math = |s: &AppState| {
        s.measurements()
            .into_iter()
            .find(
                |m| matches!(&m.config.kind, MeasKind::Math { config } if config.owner == under_tf),
            )
            .map(|m| m.id)
    };
    d.until("the math channel under the measurement", |s| {
        math(s).is_some()
    })?;
    let math_id = math(&d.st).ok_or("math")?;
    if let Some(MeasKind::Math { config }) = d.st.meas(math_id).map(|m| &m.config.kind) {
        assert!(matches!(
            config.expr,
            ac2_proto::model::MathExpr::Binary {
                op: MathOp::Divide,
                ..
            }
        ));
    }
    let keys: Vec<TreeKey> = d.st.tree_rows().iter().map(|r| r.key).collect();
    assert_eq!(
        &keys[..5],
        &[
            TreeKey::Meas(tf),
            TreeKey::Live(tf),
            TreeKey::Trace(a),
            TreeKey::Trace(b),
            TreeKey::Math(math_id)
        ]
    );

    // A sweep measurement: made by the dialog, it waits; nothing plays.
    let sweeps_before = sweep_count(&d.st);
    d.key("Shift+S");
    d.send(Msg::Text("S".into()));
    d.until(
        "the sweep dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Sweep),
    )?;
    if let Overlay::Form(f) = &mut d.st.overlay {
        f.set_text(FieldId::Level, "-20");
        f.set_text(FieldId::From, "100 Hz");
        f.set_text(FieldId::To, "5 kHz");
        f.focus = f
            .fields
            .iter()
            .position(|x| x.id == FieldId::Duration)
            .ok_or("duration")?;
        f.cycle(-1);
    }
    d.key("Enter");
    d.until("the sweep measurement listed", |s| {
        sweep_count(s) > sweeps_before && s.sweep_meas().is_some_and(|m| Some(m.id) == s.selected)
    })?;
    let sweep = d.st.sweep_meas().map(|m| m.id).ok_or("sweep measurement")?;
    std::thread::sleep(Duration::from_millis(300));
    d.pump();
    let st = d.st.daemon().ok_or("state")?;
    assert!(!st.generator.armed && !st.generator.firing, "nothing armed");
    assert!(st.sweep.is_none(), "nothing played");
    // Space arms its run, Enter plays it; twice.
    for n in 1..=2 {
        d.key("Space");
        d.until("armed with the sweep", |s| {
            s.stimulus.phase == StimPhase::Armed
        })?;
        d.key("Enter");
        d.until("the run stored, the stimulus off", |s| {
            runs_of(s, sweep).len() == n
                && s.sweep.run.is_none()
                && s.stimulus.phase == StimPhase::Idle
                && s.daemon().is_some_and(|x| x.generator.owner.is_none())
        })?;
    }
    assert_eq!(runs_of(&d.st, sweep), ["Run 1", "Run 2"]);

    // Delete the transfer measurement, keeping what it owns: under Imported now.
    d.send(Msg::SelectMeas(tf));
    d.key("Delete");
    let Overlay::Choose(c) = &d.st.overlay else {
        return Err(format!("{:?}", d.st.overlay).into());
    };
    assert_eq!(
        c.lines[0],
        "transfer function · running · it has 2 traces and 1 math channel."
    );
    assert_eq!(c.index, ac2_scene::meas_list::KEEP);
    d.key("Enter");
    d.until("the measurement gone, its traces under Imported", |s| {
        s.meas(tf).is_none()
            && [a, b].iter().all(|t| {
                s.trace_meta(*t)
                    .is_ok_and(|m| m.edit.owner == TraceOwner::Imported)
            })
            && matches!(s.meas(math_id).map(|m| &m.config.kind),
                Some(MeasKind::Math { config }) if config.owner == TraceOwner::Imported)
    })?;
    let names: Vec<String> = d.st.tree_rows().iter().map(|r| r.name.clone()).collect();
    let at = names
        .iter()
        .position(|n| n == "Imported")
        .ok_or("Imported")?;
    assert_eq!(names.len(), at + 4, "{names:?}");
    drop(d);
    drop(daemon);
    Ok(())
}

/// The names of sweep measurement `meas`'s runs, oldest first.
fn runs_of(s: &AppState, meas: ac2_proto::units::MeasId) -> Vec<String> {
    let owner = ac2_proto::model::TraceOwner::Meas { meas };
    s.daemon().map_or(Vec::new(), |x| {
        x.traces
            .iter()
            .filter(|t| t.edit.owner == owner && t.kind == ac2_proto::model::TraceKind::Sweep)
            .map(|t| t.edit.name.clone())
            .collect()
    })
}
