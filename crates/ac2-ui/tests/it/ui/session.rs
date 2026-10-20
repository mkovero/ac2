//! The session dialogs.

use super::*;

/// Shift+O: the session dialog lists the backends and the rig's channels by name with their
/// live meters and roles; the mic is named inline, the loopback detected after a typed
/// level, Enter opens and the New measurement chooser asks what to measure.
#[test]
fn session_dialog() {
    if !have_gpu("settings_audio_mic_named") {
        return;
    }
    let fake =
        Arc::new(ac2_client::fake::FakeDaemon::start(common::fake_options()).expect("fake daemon"));
    let _meters = Meters::start(Arc::clone(&fake));
    let mut h = harness(options_at(Some(fake.endpoints())));
    step_until(&mut h, "synced", |a| {
        a.state.mirror.as_ref().is_some_and(|m| m.synced())
    });
    h.key_press_modifiers(Modifiers::SHIFT, Key::O);
    // This fake publishes preview frames unasked, and with a session open the meters are
    // subscribed before the dialog opens: the preview request is waited for on its own.
    step_until(
        &mut h,
        "dialog with devices, meters and the device previewed",
        |a| {
            session_dialog_of(a).is_some_and(|d| d.device_info().is_some())
                && a.state.input_meters().len() == 4
                && fake.lock().preview.is_some()
        },
    );
    // Audio I/O on the input device; ↓ past output device and input 1 to input 2, N names
    // its mic.
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "settings_audio");
    for _ in 0..3 {
        h.key_press(Key::ArrowDown);
    }
    h.key_press(Key::N);
    h.event(Event::Text("n".into()));
    h.event(Event::Text("M30 FOH".into()));
    h.key_press(Key::Enter);
    step_until(&mut h, "mic named", |a| {
        session_dialog_of(a).is_some_and(|d| d.inputs[1].mic == "M30 FOH" && d.edit.is_none())
    });
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "settings_audio_mic_named");

    // D, a typed level, Enter: the fake answers in 1, which becomes the Reference.
    h.key_press(Key::D);
    h.event(Event::Text("d".into()));
    h.event(Event::Text("-30".into()));
    step_until(&mut h, "level typed", |a| {
        session_dialog_of(a)
            .and_then(|d| d.detect.as_ref())
            .is_some_and(|p| p.level == "-30")
    });
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "settings_detect_confirm");
    h.key_press(Key::Enter);
    step_until(&mut h, "detected", |a| {
        session_dialog_of(a).is_some_and(|d| {
            matches!(
                d.detect.as_ref().map(|p| &p.phase),
                Some(ac2_ui::session_dialog::DetectPhase::Done(_))
            )
        })
    });
    assert_eq!(fake.executions("session.detect_loopback"), 1);
    step_until(&mut h, "lease released", |a| {
        a.state
            .daemon()
            .is_some_and(|s| s.generator.owner.is_none())
    });
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "settings_detected");

    h.key_press(Key::Enter);
    // The chooser opens on the session's answer, before its first level frame: the
    // snapshot waits for the session's two meters too.
    step_until(
        &mut h,
        "session open, the New measurement chooser, its meters",
        |a| {
            a.state.open_session().is_some()
                && new_meas_chooser(a)
                && a.state.input_meters().len() == 2
        },
    );
    assert_eq!(fake.executions("session.open"), 1);
    assert_eq!(fake.executions("session.inputs"), 1);
    assert!(
        fake.lock().preview.is_none(),
        "the session replaced the preview"
    );
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "session_new_measurement");
    // Enter on Transfer (highlighted on the transfer pane), Enter on its form.
    h.key_press(Key::Enter);
    step_until(
        &mut h,
        "the transfer form",
        |a| matches!(&a.state.overlay, Overlay::Form(f) if f.kind == ac2_ui::forms::FormKind::Transfer),
    );
    h.key_press(Key::Enter);
    // Created, then started by a second command: the picture shows it running.
    step_until(&mut h, "measurement created and running", |a| {
        a.state
            .measurements()
            .iter()
            .any(|m| m.config.name == "Reference → M30 FOH" && m.running)
    });

    // The new-measurement dialog picks inputs by name, with the session's meters.
    h.state_mut().dispatch(ac2_ui::state::Msg::Command(
        ac2_ui::keys::CommandId::NewTransfer,
    ));
    step_until(&mut h, "transfer dialog with meters", |a| {
        matches!(a.state.overlay, Overlay::Form(_)) && a.state.input_meters().len() == 2
    });
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "transfer_dialog");
    h.key_press(Key::Escape);
    step_until(&mut h, "dialog closed", |a| {
        a.state.overlay == Overlay::None
    });

    // Shift+S: the sweep dialog, inputs and outputs by name, a typed level; Enter makes the
    // sweep measurement (nothing armed) and focuses the sweep pane, Space there arms its
    // run, Enter plays it and the result shows on the pane.
    h.key_press_modifiers(Modifiers::SHIFT, Key::S);
    h.event(Event::Text("S".into()));
    step_until(
        &mut h,
        "sweep dialog",
        |a| matches!(&a.state.overlay, Overlay::Form(f) if f.kind == ac2_ui::forms::FormKind::Sweep),
    );
    for _ in 0..3 {
        h.key_press(Key::ArrowDown);
    }
    h.event(Event::Text("-30".into()));
    step_until(&mut h, "level typed, inputs metered", |a| {
        matches!(&a.state.overlay, Overlay::Form(f)
            if f.text(ac2_ui::forms::FieldId::Level) == "-30")
            && a.state.input_meters().len() == 2
    });
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "sweep_dialog");
    // A sweep pane beside the others (as N and the pane menu make one): the sweep goes there.
    h.state_mut().state.layout = common::layout_of(&common::grid_with_sweep_prefs());
    h.key_press(Key::Enter);
    step_until(&mut h, "the sweep measurement, nothing armed", |a| {
        a.state.sweep_meas().is_some()
            && a.state.layout.focus_kind() == PaneKind::Distortion
            && a.state.daemon().is_some_and(|s| !s.generator.armed)
    });
    h.key_press(Key::Space);
    step_until(&mut h, "armed with the sweep", |a| {
        a.state.daemon().is_some_and(|s| {
            s.generator.armed
                && s.generator
                    .settings
                    .as_ref()
                    .is_some_and(|g| matches!(g.signal, ac2_proto::model::Signal::Ess { .. }))
        })
    });
    h.key_press(Key::Enter);
    step_until(&mut h, "sweep stored and shown", |a| {
        a.state.layout.focus_kind() == PaneKind::Distortion && a.state.kind_sweep().is_some()
    });
    assert_eq!(fake.executions("sweep.run"), 1);
    // The sweep left nothing armed: the stimulus is off (STIM OFF) and the lease was given
    // back.
    step_until(&mut h, "stimulus off and released", |a| {
        !a.state.stimulus_live()
            && a.state
                .daemon()
                .is_some_and(|s| s.generator.owner.is_none() && !s.generator.armed)
    });
    // In the grid, beside the other panes: caption and legend fit a small pane.
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "sweep_distortion_pane");
    // The focused pane alone, for the picture.
    h.key_press(Key::W);
    step_until(&mut h, "maximized", |a| a.state.layout.maximized);
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "sweep_distortion");
    // The title's dB | % toggle (U does the same): percent on a log axis.
    h.get_by_label("%").click();
    step_until(&mut h, "percent", |a| {
        a.state.view.distortion.unit == ac2_scene::view::DistortionUnit::Percent
    });
    // The pointer away again: no cursor or tooltip in the pictures.
    h.event(Event::PointerGone);
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "sweep_distortion_percent");
    // The cursor (palette) reads the fundamental, every order and THD, here in percent.
    palette(&mut h, "comparison cursor");
    step_until(&mut h, "the cursor", |a| a.state.view.cursor_hz.is_some());
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "sweep_distortion_cursor");
    palette(&mut h, "comparison cursor");
    h.key_press_modifiers(Modifiers::SHIFT, Key::I);
    step_until(&mut h, "sweep IR", |a| {
        a.state
            .kind_modes(ac2_ui::state::PaneKind::Distortion)
            .sweep
            == ac2_scene::view::SweepMode::Ir(ac2_scene::view::IrMode::Linear)
    });
    // G: the log view, where the harmonics' impulses read at their level.
    h.key_press(Key::G);
    step_until(&mut h, "log IR", |a| {
        a.state
            .kind_modes(ac2_ui::state::PaneKind::Distortion)
            .sweep
            == ac2_scene::view::SweepMode::Ir(ac2_scene::view::IrMode::Log)
    });
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "sweep_ir");
    // G past the ETC: the room parameters alone, the whole (maximised) pane.
    h.key_press(Key::G);
    h.step();
    h.key_press(Key::G);
    step_until(&mut h, "room parameters", |a| {
        a.state
            .kind_modes(ac2_ui::state::PaneKind::Distortion)
            .sweep
            == ac2_scene::view::SweepMode::Room
    });
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "sweep_room");
}

/// Alt+S: the Sessions dialog's actions in one left-aligned column; Save as takes a name
/// typed through the app's real input path (key press, then its text, as egui delivers it),
/// letters that are shortcuts elsewhere included; Esc backs out.
#[test]
fn sessions_dialog_save_as() {
    use ac2_ui::sessions::Page;
    if !have_gpu("sessions_dialog_save_as") {
        return;
    }
    let fake = ac2_client::fake::FakeDaemon::start(common::fake_options()).expect("fake daemon");
    let mut h = harness(options_at(Some(fake.endpoints())));
    step_until(&mut h, "synced", |a| {
        a.state.mirror.as_ref().is_some_and(|m| m.synced())
    });
    h.key_press_modifiers(Modifiers::ALT, Key::S);
    step_until(&mut h, "the dialog, listed", |a| {
        a.state
            .sessions_dialog()
            .is_some_and(|d| d.daemon.is_some())
    });
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "sessions_dialog");
    let save_as = h
        .state()
        .state
        .sessions_rows(ac2_proto::units::WallNs(0))
        .iter()
        .position(|r| r.title == "Save as…")
        .expect("a Save as row");
    for _ in 0..save_as {
        h.key_press(Key::ArrowDown);
    }
    h.key_press(Key::Enter);
    step_until(&mut h, "the name page", |a| {
        a.state
            .sessions_dialog()
            .is_some_and(|d| matches!(d.page, Page::Name(_)))
    });
    // The suggested name goes; Backspace held repeats on the name.
    for _ in 0..40 {
        h.key_press(Key::Backspace);
    }
    let keys = [
        (Key::F, "F"),
        (Key::A, "a"),
        (Key::W, "w"),
        (Key::Q, "q"),
        (Key::G, "g"),
        (Key::N, "n"),
        (Key::Space, " "),
        (Key::Num2, "2"),
    ];
    for (k, t) in keys {
        h.key_press(k);
        h.event(Event::Text(t.into()));
    }
    h.key_press(Key::Tab);
    h.step();
    {
        let Some(Page::Name(n)) = h.state().state.sessions_dialog().map(|d| &d.page) else {
            panic!("{:?}", h.state().state.overlay);
        };
        assert_eq!(n.text, "Fawqgn 2");
    }
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "sessions_dialog_save_as");
    // The fake keeps no session directory, so the save itself is the reducer tests'
    // (`state_sessions_tests`); Esc backs out to the actions, a second Esc closes.
    h.key_press(Key::Escape);
    step_until(&mut h, "back on the actions", |a| {
        a.state
            .sessions_dialog()
            .is_some_and(|d| d.page == Page::Main)
    });
    h.key_press(Key::Escape);
    step_until(&mut h, "closed", |a| a.state.overlay == Overlay::None);
}
