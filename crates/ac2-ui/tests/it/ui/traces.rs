//! Stored traces, the measurement and trace lists.

use super::*;

/// Two captures in slots 1 and 2 and an imported target curve, moved under Main L, drawn
/// with the live traces (the other group and the target compared):
/// captures share the epoch's time base (Δt to the reference), the target is independent.
#[test]
fn stored_traces_and_target() {
    if !have_gpu("stored_traces_and_target") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    // Ctrl+1 captures Main L (selected) into slot 1; Tab, Ctrl+2 Delay tower into slot 2.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num1);
    step_until(&mut h, "slot 1", |a| a.state.slots()[0].is_some());
    tab_to(&mut h, MeasId(2));
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num2);
    step_until(&mut h, "slot 2", |a| a.state.slots()[1].is_some());
    // Z: a target curve from a file, the chooser's first row.
    h.key_press(Key::Z);
    step_until(&mut h, "target chooser", |a| {
        matches!(a.state.overlay, Overlay::Choose(_))
    });
    h.key_press(Key::Enter);
    step_until(&mut h, "target prompt", |a| {
        matches!(a.state.overlay, Overlay::Prompt(_))
    });
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ac2-traces/tests/fixtures/house_curve.txt");
    h.event(Event::Text(path.to_string_lossy().into_owned()));
    h.key_press(Key::Enter);
    step_until(&mut h, "three stored traces with data", |a| {
        a.state.traces.len() == 3
    });
    // 2 hides slot 2, 2 again shows it.
    h.key_press(Key::Num2);
    step_until(&mut h, "slot 2 hidden", |a| {
        a.state.slots()[1].is_some_and(|t| !t.edit.visible)
    });
    h.key_press(Key::Num2);
    step_until(&mut h, "slot 2 shown", |a| {
        a.state.slots()[1].is_some_and(|t| t.edit.visible)
            && a.state.traces.values().all(|(d, _)| d.meta.edit.visible)
    });
    // An imported trace is drawn on no pane: the target moved under Main L (Shift+F2).
    let target = h
        .state()
        .state
        .traces
        .values()
        .find(|(d, _)| d.meta.edit.name == "house_curve")
        .map(|(d, _)| d.meta.id)
        .expect("the target");
    h.state_mut()
        .dispatch(ac2_ui::state::Msg::SelectTrace(target));
    h.state_mut().dispatch(ac2_ui::state::Msg::Command(
        ac2_ui::keys::CommandId::MoveTrace,
    ));
    {
        let Overlay::Choose(c) = &mut h.state_mut().state.overlay else {
            panic!("no move dialog");
        };
        c.index = c
            .owners
            .iter()
            .position(|o| *o == ac2_proto::model::TraceOwner::Meas { meas: MeasId(1) })
            .expect("Main L among the owners");
    }
    h.key_press(Key::Enter);
    step_until(&mut h, "the target under Main L", |a| {
        a.state.traces.get(&target).is_some_and(|(d, _)| {
            d.meta.edit.owner == ac2_proto::model::TraceOwner::Meas { meas: MeasId(1) }
        })
    });
    // A click on its row again: nothing selected, as before the move.
    h.state_mut()
        .dispatch(ac2_ui::state::Msg::SelectTrace(target));
    // The pane shows Delay tower's group (N chose it): Main L, its capture and the target
    // join it compared (C).
    {
        use ac2_proto::model::Compared;
        let st = &h.state().state;
        let mut compared = vec![Compared::Meas { meas: MeasId(1) }];
        compared.extend(
            st.traces
                .values()
                .map(|(d, _)| d.meta.clone())
                .filter(|t| !t.edit.name.starts_with("Delay tower"))
                .map(|t| Compared::Trace { trace: t.id }),
        );
        rig.compare_into(2, compared);
    }
    step_until(&mut h, "compared into Delay tower", |a| {
        a.state
            .meas(MeasId(2))
            .is_some_and(|m| !m.compared.is_empty())
    });
    {
        let st = &h.state().state;
        let s = ac2_ui::scenes::transfer(
            st,
            crate::common::pane(st, ac2_ui::state::PaneKind::Transfer),
            &ac2_scene::theme::Theme::dark(),
            ac2_scene::primitives::Viewport {
                width: 1000.0,
                height: 450.0,
            },
            ac2_ui::scenes::Now {
                instant: Instant::now(),
                wall: ac2_proto::units::WallNs(0),
            },
        );
        let legend: Vec<&str> = s.legend.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(legend.len(), 5, "{legend:?}");
        assert!(legend.contains(&"house_curve · cmp · indep."), "{legend:?}");
        assert!(
            legend.iter().any(|l| l.starts_with("Main L S1 · cmp · Δt")),
            "{legend:?}"
        );
    }
    // Toasts expire on the wall clock; the snapshot shows the plots only.
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "transfer_stored_traces");
}

/// Two captures, the second spread +3 dB with Alt+Shift+↑: its curve sits 3 dB above the
/// other's and its legend row says `+3.0 dB`, so the spread is never read as a level
/// difference.
#[test]
fn trace_offset_spread() {
    if !have_gpu("trace_offset_spread") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num1);
    step_until(&mut h, "slot 1", |a| a.state.slots()[0].is_some());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num2);
    step_until(&mut h, "both slots with data", |a| {
        a.state.slots()[1].is_some() && a.state.traces.len() == 2
    });
    let id = h.state().state.slots()[1].map(|t| t.id).expect("slot 2");
    h.key_press(Key::V);
    h.key_press(Key::V);
    step_until(&mut h, "slot 2 selected", |a| {
        a.state.selected_trace == Some(id)
    });
    h.key_press_modifiers(Modifiers::ALT | Modifiers::SHIFT, Key::ArrowUp);
    step_until(&mut h, "slot 2 spread", |a| {
        a.state
            .traces
            .get(&id)
            .is_some_and(|(d, _)| d.meta.edit.offset.0 == 3.0)
    });
    {
        let st = &h.state().state;
        let s = ac2_ui::scenes::transfer(
            st,
            crate::common::pane(st, ac2_ui::state::PaneKind::Transfer),
            &ac2_scene::theme::Theme::dark(),
            ac2_scene::primitives::Viewport {
                width: 1000.0,
                height: 450.0,
            },
            ac2_ui::scenes::Now {
                instant: Instant::now(),
                wall: ac2_proto::units::WallNs(0),
            },
        );
        let legend: Vec<&str> = s.legend.iter().map(|e| e.text.as_str()).collect();
        assert!(
            legend
                .iter()
                .any(|l| l.starts_with("Main L S2") && l.contains("· +3.0 dB")),
            "{legend:?}"
        );
        assert!(
            legend
                .iter()
                .any(|l| l.starts_with("Main L S1") && !l.contains("dB")),
            "{legend:?}"
        );
    }
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "trace_offset_spread");
}

/// The transfer pane's title chip names the measurement it shows; a click opens the list of
/// transfer measurements and a pick switches the pane (and the selection) to it.
#[test]
fn pane_measurement_chip_and_list() {
    if !have_gpu("pane_measurement_chip_and_list") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    // The transfer pane's chip (the IR pane's names the same measurement, drawn after it).
    h.get_all_by_label("Main L")
        .next()
        .expect("transfer pane chip")
        .click();
    step_until(
        &mut h,
        "list",
        |a| matches!(a.state.overlay, Overlay::PaneMenu(m) if m.pane == crate::common::pane(&a.state, PaneKind::Transfer)),
    );
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "pane_measurement_list");
    h.get_by_label("TF  Delay tower").click();
    step_until(&mut h, "delay tower shown", |a| {
        a.state.overlay == Overlay::None
            && a.state.kind_meas(PaneKind::Transfer).map(|m| m.id) == Some(MeasId(2))
    });
    assert_eq!(
        h.state().state.selected,
        Some(MeasId(1)),
        "a pick selects nothing"
    );
    // The pane now leads with it: first legend row, IR of it.
    let st = &h.state().state;
    let s = ac2_ui::scenes::transfer(
        st,
        crate::common::pane(st, ac2_ui::state::PaneKind::Transfer),
        &ac2_scene::theme::Theme::dark(),
        ac2_scene::primitives::Viewport {
            width: 1000.0,
            height: 450.0,
        },
        ac2_ui::scenes::Now {
            instant: Instant::now(),
            wall: ac2_proto::units::WallNs(0),
        },
    );
    assert_eq!(s.legend[0].name, "Delay tower");
    assert_eq!(ac2_ui::scenes::focus_tf(st).map(|m| m.id), Some(MeasId(2)));
}

/// A slot selected in the list takes the smoothing keys: K re-smooths the stored capture
/// (the daemon serves it at the new setting) while the live curves keep theirs.
#[test]
fn slot_resmoothed() {
    if !have_gpu("slot_resmoothed") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num1);
    step_until(&mut h, "slot 1 with data", |a| {
        a.state.slots()[0].is_some() && a.state.traces.len() == 1
    });
    let id = h.state().state.slots()[0].map(|t| t.id).expect("slot 1");
    h.get_by_label("Main L S1\ncapture · slot 1 · 1/6 oct")
        .click();
    step_until(&mut h, "slot selected", |a| {
        a.state.selected_trace == Some(id)
    });
    let before = h.state().state.traces[&id].0.mag_db.clone();
    h.key_press(Key::K);
    step_until(&mut h, "re-smoothed data", |a| {
        a.state.traces.get(&id).is_some_and(|(d, _)| {
            d.meta
                .edit
                .smoothing
                .is_some_and(|s| s.fraction == ac2_proto::model::SmoothingFraction::Third)
        })
    });
    let st = &h.state().state;
    assert_ne!(st.traces[&id].0.mag_db, before, "served at the new setting");
    assert_eq!(
        st.smoothing_caption(crate::common::pane(st, PaneKind::Transfer))
            .as_deref(),
        Some("slot 1 (Main L S1): smoothing 1/3 oct")
    );
    // The live measurement was not touched.
    assert!(st.meas(MeasId(1)).is_some_and(|m| matches!(
        &m.config.kind,
        ac2_proto::model::MeasKind::Transfer { config }
            if config.smoothing.is_some_and(|s| s.fraction == ac2_proto::model::SmoothingFraction::Sixth)
    )));
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "slot_resmoothed");
}

/// Smoothing on phase and spectrum: the rig publishes scattered curves smoothed as the
/// daemon does. The transfer scene draws the smoothed phase (wrapped, unwrapped and group
/// delay all follow from it), and the spectrum axis says the level is smoothed.
#[test]
fn smoothed_phase_and_spectrum() {
    if !have_gpu("smoothed_phase_and_spectrum") {
        return;
    }
    let rig = common::Rig::start_smoothed();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    {
        let st = &h.state().state;
        let now = ac2_ui::scenes::Now {
            instant: Instant::now(),
            wall: ac2_proto::units::WallNs(0),
        };
        let size = ac2_scene::primitives::Viewport {
            width: 1000.0,
            height: 450.0,
        };
        let theme = ac2_scene::theme::Theme::dark();
        let s = ac2_ui::scenes::transfer(
            st,
            crate::common::pane(st, ac2_ui::state::PaneKind::Transfer),
            &theme,
            size,
            now,
        );
        let raw = common::raw_tf_frame(1, 0.0, 0.0, 2000.0);
        let want = common::smoothed_tf_frame(&raw);
        let t = &s.traces[0];
        let diff = |a: f64, b: f64| {
            let d = (a - b).rem_euclid(360.0);
            d.min(360.0 - d)
        };
        let mut moved: f64 = 0.0;
        for (i, p) in t.phase_wrapped_deg.iter().enumerate() {
            // The frame reached the app in hundredths of a degree.
            assert!(
                diff(*p, f64::from(want.phase[i])) < 0.006,
                "column {i}: drawn {p} vs smoothed {}",
                want.phase[i]
            );
            moved = moved.max(diff(*p, f64::from(raw.phase[i])));
        }
        assert!(moved > 5.0, "the drawn phase is the raw one");
        // Unwrapped phase steps (and so group delay) follow the smoothed phase.
        for i in 1..want.phase.len() {
            let step = t.phase_unwrapped_deg[i] - t.phase_unwrapped_deg[i - 1];
            let want_step = f64::from(want.phase[i] - want.phase[i - 1]);
            assert!(diff(step, want_step) < 0.011, "column {i}");
        }
        assert_eq!(s.legend[0].text, "Main L · ref · 1/6 oct");

        let sp = ac2_ui::scenes::spectrum(
            st,
            crate::common::pane(st, ac2_ui::state::PaneKind::Spectrum),
            &theme,
            size,
            now,
        );
        assert_eq!(sp.unit, "dBFS per 11.7 Hz bin (tone, 1/6 oct smoothed)");
        assert_eq!(sp.caption, "Hann window");
    }
    // The phase drawn in its pane of the transfer stack, under the magnitude.
    let tf = crate::common::pane(&h.state().state, ac2_ui::state::PaneKind::Transfer);
    h.state_mut().state.layout.set_focus(tf);
    // Main L's row chosen: the app's own first pick takes no panes, the operator's does.
    h.state_mut()
        .dispatch(ac2_ui::state::Msg::SelectMeas(MeasId(1)));
    h.state_mut().dispatch(ac2_ui::state::Msg::Command(
        ac2_ui::keys::CommandId::TransferStack,
    ));
    let views: Vec<_> = {
        let l = &h.state().state.layout;
        l.panes()
            .into_iter()
            .filter_map(|id| l.view(id))
            .filter(|v| v.kind == ac2_ui::state::PaneKind::Transfer && !v.shows_ir())
            .map(|v| v.modes.transfer)
            .collect()
    };
    use ac2_ui::state::TransferView as V;
    assert_eq!(views, [V::Magnitude, V::Phase, V::Coherence]);
    // The stack becomes Main L's own panes on the daemon (`meas.layout`); its reply changes
    // the tree row and the session label, so the snapshot waits for it. Its toast expires
    // on the wall clock.
    step_until(&mut h, "own panes on the daemon", |a| {
        a.state.working().dirty
            && a.state
                .measurements()
                .iter()
                .any(|m| m.id == MeasId(1) && m.layout.is_some())
    });
    h.state_mut().state.toasts.list.clear();
    snapshot(&mut h, "smoothed_phase_and_spectrum");
}

/// A daemon with no audio I/O (a fresh local daemon, an embedded one on real audio):
/// the transfer pane says how to open one, also once a file is imported.
#[test]
fn empty_session_hint() {
    if !have_gpu("empty_session_hint") {
        return;
    }
    let fake = ac2_client::fake::FakeDaemon::start(common::fake_options()).expect("fake daemon");
    let mut h = harness(options_at(Some(fake.endpoints())));
    step_until(&mut h, "synced, no session", |a| {
        a.state.mirror.as_ref().is_some_and(|m| m.synced())
            && a.state
                .empty_hint(a.state.layout.focus, &a.keymap)
                .is_some()
    });
    assert_eq!(
        h.state()
            .state
            .empty_hint(h.state().state.layout.focus, &h.state().keymap)
            .map(|h| h.text)
            .as_deref(),
        Some("No session open — Alt+S, then New session")
    );
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "empty_session_hint");

    // An imported target, without a session to file it under, is under Imported: no pane
    // draws it, and the pane's hint still says how to open a session.
    // Z, then the chooser's first row: a target curve from a file.
    h.key_press(Key::Z);
    step_until(&mut h, "target chooser", |a| {
        matches!(a.state.overlay, Overlay::Choose(_))
    });
    h.key_press(Key::Enter);
    step_until(&mut h, "target prompt", |a| {
        matches!(a.state.overlay, Overlay::Prompt(_))
    });
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ac2-traces/tests/fixtures/house_curve.txt");
    h.event(Event::Text(path.to_string_lossy().into_owned()));
    h.key_press(Key::Enter);
    step_until(&mut h, "the target stored", |a| {
        a.state.traces.len() == 1 && a.state.overlay == Overlay::None
    });
    let st = &h.state().state;
    assert!(!st.pane_shows_stored(st.layout.focus));
    let hint = st.empty_hint(st.layout.focus, &h.state().keymap);
    assert_eq!(
        hint.as_ref().map(|h| h.place),
        Some(ac2_ui::state::HintPlace::Centre)
    );
    assert!(
        hint.is_some_and(|h| h.text.starts_with("No session open")),
        "the session hint stays"
    );
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "empty_session_hint_imported");
}

/// The Traces list: every stored trace by name with what it is, its slot, shown or hidden
/// (the dot, a click on it toggles) and the selected one highlighted (a click on the row).
#[test]
fn traces_list() {
    if !have_gpu("traces_list") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num1);
    step_until(&mut h, "slot 1", |a| a.state.slots()[0].is_some());
    tab_to(&mut h, MeasId(2));
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num2);
    step_until(&mut h, "slot 2", |a| a.state.slots()[1].is_some());
    // Z, then the chooser's first row: a target curve from a file.
    h.key_press(Key::Z);
    step_until(&mut h, "target chooser", |a| {
        matches!(a.state.overlay, Overlay::Choose(_))
    });
    h.key_press(Key::Enter);
    step_until(&mut h, "target prompt", |a| {
        matches!(a.state.overlay, Overlay::Prompt(_))
    });
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ac2-traces/tests/fixtures/house_curve.txt");
    h.event(Event::Text(path.to_string_lossy().into_owned()));
    h.key_press(Key::Enter);
    step_until(&mut h, "three stored traces with data", |a| {
        a.state.traces.len() == 3
    });
    // The dot hides slot 2; a click on the target's row selects it.
    h.get_by_label("Hide Delay tower S2").click();
    step_until(&mut h, "slot 2 hidden", |a| {
        a.state.slots()[1].is_some_and(|t| !t.edit.visible)
    });
    h.get_by_label("house_curve\ntarget").click();
    step_until(&mut h, "target selected", |a| {
        a.state
            .selected_trace_meta()
            .is_some_and(|t| t.edit.name == "house_curve")
    });
    assert_eq!(
        h.state()
            .state
            .pane_caption(crate::common::pane(&h.state().state, PaneKind::Transfer))
            .as_deref(),
        Some("house_curve")
    );
    // The hidden trace's dot now offers to show it.
    h.get_by_label("Show Delay tower S2");
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "traces_list");
}

/// A on the selected measurement hides its curve (its list row says `hidden`, dimmed; the
/// transfer pane's title says so and its legend loses it), and Backspace asks before it is
/// deleted, naming it and what goes.
#[test]
fn measurement_hidden_and_delete_confirm() {
    if !have_gpu("measurement_hidden_and_delete_confirm") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    // Past the first measurement's live curve row to the next measurement.
    h.key_press(Key::Tab);
    h.key_press(Key::Tab);
    step_until(&mut h, "delay tower", |a| {
        a.state.selected == Some(MeasId(2))
    });
    h.key_press(Key::A);
    step_until(&mut h, "delay tower hidden", |a| {
        a.state.hidden_meas.contains(&MeasId(2))
    });
    let caption = h
        .state()
        .state
        .pane_caption(crate::common::pane(&h.state().state, PaneKind::Transfer));
    assert!(
        caption
            .as_deref()
            .is_some_and(|c| c.starts_with("Delay tower hidden · ")),
        "{caption:?}"
    );
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "measurement_hidden");
    h.key_press(Key::Backspace);
    step_until(
        &mut h,
        "the confirmation",
        |a| matches!(&a.state.overlay, Overlay::Delete(p) if p.target == ac2_ui::state::DeleteTarget::Meas(MeasId(2))),
    );
    h.step();
    snapshot(&mut h, "measurement_delete_confirm");
    h.key_press(Key::Escape);
    step_until(&mut h, "kept", |a| {
        a.state.overlay == Overlay::None && a.state.measurements().len() == 4
    });
}

/// The measurement tree: captures filed under the measurement they came from, the imported
/// target under Imported, a folded group; then Delete on a measurement that owns traces asks
/// Keep / Delete / Cancel.
#[test]
fn measurement_tree_and_delete_choices() {
    if !have_gpu("measurement_tree_and_delete_choices") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num1);
    step_until(&mut h, "slot 1", |a| a.state.slots()[0].is_some());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num2);
    step_until(&mut h, "slot 2", |a| a.state.slots()[1].is_some());
    tab_to(&mut h, MeasId(2));
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num3);
    step_until(&mut h, "slot 3", |a| a.state.slots()[2].is_some());
    // Z, then the chooser's first row: a target curve from a file.
    h.key_press(Key::Z);
    step_until(&mut h, "target chooser", |a| {
        matches!(a.state.overlay, Overlay::Choose(_))
    });
    h.key_press(Key::Enter);
    step_until(&mut h, "target prompt", |a| {
        matches!(a.state.overlay, Overlay::Prompt(_))
    });
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ac2-traces/tests/fixtures/house_curve.txt");
    h.event(Event::Text(path.to_string_lossy().into_owned()));
    h.key_press(Key::Enter);
    step_until(&mut h, "four stored traces with data", |a| {
        a.state.traces.len() == 4
    });
    let names: Vec<String> = h
        .state()
        .state
        .tree_rows()
        .iter()
        .map(|r| r.name.clone())
        .collect();
    // The target was loaded with Delay tower on screen: it is listed under it, and nothing
    // is left under Imported.
    let at = names.iter().position(|n| n == "house_curve");
    assert_eq!(
        at.map(|i| names[i - 1].as_str()),
        Some("Delay tower S3"),
        "{names:?}"
    );
    assert!(!names.iter().any(|n| n == "Imported"), "{names:?}");
    // Delay tower's group folded with its arrow.
    h.state_mut().dispatch(ac2_ui::state::Msg::ToggleGroup(
        ac2_proto::model::TraceOwner::Meas { meas: MeasId(2) },
    ));
    h.state_mut()
        .dispatch(ac2_ui::state::Msg::SelectMeas(MeasId(1)));
    // The IR pane follows the selection, and only the followed measurement's IR is
    // received: Main L's was dropped while Delay tower was selected and comes back with
    // the next frame after the resubscription, so the picture waits for it.
    step_until(&mut h, "Main L's IR again", |a| {
        a.state.data.as_ref().is_some_and(|d| {
            d.latest
                .get(&Topic::Data {
                    meas: MeasId(1),
                    stream: Stream::Ir,
                })
                .is_some()
        })
    });
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "measurement_tree");
    h.key_press(Key::Delete);
    step_until(&mut h, "the three answers", |a| {
        matches!(a.state.overlay, Overlay::Choose(_))
    });
    h.step();
    snapshot(&mut h, "measurement_delete_choices");
    h.key_press(Key::Escape);
    step_until(&mut h, "cancelled", |a| a.state.overlay == Overlay::None);
}

/// Z's built-in curves against the fake daemon: the chooser, the loudness dialog, and the
/// generated traces it stores under their parameters' names.
#[test]
fn built_in_target_curves() {
    if !have_gpu("built_in_target_curves") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press(Key::Z);
    step_until(&mut h, "target chooser", |a| {
        matches!(a.state.overlay, Overlay::Choose(_))
    });
    snapshot(&mut h, "target_chooser");
    for _ in 0..3 {
        h.key_press(Key::ArrowDown);
    }
    h.key_press(Key::Enter);
    step_until(&mut h, "loudness dialog", |a| {
        matches!(a.state.overlay, Overlay::Form(_))
    });
    snapshot(&mut h, "target_loudness_dialog");
    h.key_press(Key::Enter);
    step_until(&mut h, "the loudness shift stored", |a| {
        a.state.traces.values().any(|(d, _)| {
            d.meta.edit.name == "loudness 85 → 100 phon"
                && matches!(
                    d.meta.source,
                    ac2_proto::model::TraceSource::Generated { .. }
                )
                && d.meta.kind == ac2_proto::model::TraceKind::Target
        })
    });
    // Flat needs no dialog.
    h.key_press(Key::Z);
    step_until(&mut h, "target chooser", |a| {
        matches!(a.state.overlay, Overlay::Choose(_))
    });
    h.key_press(Key::ArrowDown);
    h.key_press(Key::Enter);
    step_until(&mut h, "flat stored", |a| {
        a.state
            .traces
            .values()
            .any(|(d, _)| d.meta.edit.name == "flat")
    });
}
