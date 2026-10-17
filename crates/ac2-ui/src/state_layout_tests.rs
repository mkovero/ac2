//! Reducer tests of key hints, captions, the pane layout, what the link receives and the measurement tree.

use super::*;

fn hint_texts(t: &T, pane: PaneKind) -> Option<Vec<String>> {
    hints_of(t, t.st.layout.lead(pane)?)
}

fn hints_of(t: &T, id: PaneId) -> Option<Vec<String>> {
    t.st.key_hint_line(&t.keys, id, crate::keys::LabelStyle::Pc)
        .map(|v| v.iter().map(crate::hints::KeyHint::text).collect())
}

/// `key` pressed: the row it selected, `m<id>` a measurement, `l<id>` its live curve,
/// `t<id>` a stored trace.
fn tab(t: &mut T, key: &str) -> String {
    t.key(key);
    match (t.st.selected_trace, t.st.selected) {
        (Some(tr), _) => format!("t{}", tr.0),
        (None, Some(m)) if t.st.live_row_selected() => format!("l{}", m.0),
        (None, Some(m)) => format!("m{}", m.0),
        (None, None) => "none".into(),
    }
}

/// The focused pane, and only it, has a hint line; its hints are the pane's own and follow
/// what the pane shows now; Shift+H turns the lines off and on and the choice is kept.
#[test]
fn key_hints_follow_the_focused_pane() {
    let mut t = T::new();
    assert!(t.st.prefs.key_hints);
    let tf = hint_texts(&t, PaneKind::Transfer).expect("transfer focused");
    assert_eq!(tf.first().map(String::as_str), Some("V select trace"));
    assert_eq!(tf.last().map(String::as_str), Some("H all keys"));
    for p in [PaneKind::Spectrum, PaneKind::Spl] {
        assert_eq!(hint_texts(&t, p), None, "{p:?} is not focused");
    }
    assert_eq!(
        hints_of(&t, t.ir_pane()),
        None,
        "the IR view is not focused"
    );
    t.key("Alt+2");
    assert_eq!(hint_texts(&t, PaneKind::Transfer), None);
    let sp = hint_texts(&t, PaneKind::Spectrum).expect("spectrum focused");
    assert!(sp.contains(&"P peak hold".to_owned()), "{sp:?}");
    t.key("Alt+3");
    let ir = hints_of(&t, t.ir_pane()).expect("IR view focused");
    assert!(ir.contains(&"G/Shift+G view".to_owned()), "{ir:?}");
    t.key("Alt+4");
    let spl = hint_texts(&t, PaneKind::Spl).expect("SPL focused");
    assert_eq!(spl[..3], ["G/Shift+G view", "Shift+F F/S/I", "Z A/C/Z"]);
    // The sweep pane's view keys are the same pair in each of its views.
    t.go(PaneKind::Distortion);
    let d = hint_texts(&t, PaneKind::Distortion).expect("sweep pane focused");
    assert!(d.contains(&"G/Shift+G view".to_owned()), "{d:?}");
    t.key("Shift+I");
    let d = hint_texts(&t, PaneKind::Distortion).expect("sweep pane focused");
    assert!(d.contains(&"G/Shift+G view".to_owned()), "{d:?}");
    // Mac labels.
    let mac: Vec<String> =
        t.st.key_hint_line(
            &t.keys,
            t.pane(PaneKind::Distortion),
            crate::keys::LabelStyle::Mac,
        )
        .expect("line")
        .iter()
        .map(crate::hints::KeyHint::text)
        .collect();
    assert_eq!(mac.first().map(String::as_str), Some("⇧S new sweep"));

    // Off: no line anywhere, remembered, and the toast says how to bring it back.
    t.st.prefs_dirty = false;
    t.st.update(Msg::Command(CommandId::KeyHints), &t.keys);
    assert!(!t.st.prefs.key_hints);
    assert!(t.st.prefs_dirty);
    assert!(t.last_toast().contains("the palette"), "{}", t.last_toast());
    for p in PaneKind::ALL {
        assert_eq!(hint_texts(&t, p), None);
    }
    // The title's tooltip still lists the pane's hints.
    assert!(
        !t.st
            .pane_hints(&t.keys, t.pane(PaneKind::Spl), crate::keys::LabelStyle::Pc)
            .is_empty()
    );
    // The palette entry turns them back on.
    t.st.update(Msg::Command(CommandId::KeyHints), &t.keys);
    assert!(t.st.prefs.key_hints);
    assert!(hint_texts(&t, PaneKind::Distortion).is_some());
    // The next start takes the remembered choice.
    let mut u = T::new();
    u.st.set_prefs(crate::prefs::UiPrefs {
        key_hints: false,
        ..Default::default()
    });
    assert_eq!(hint_texts(&u, PaneKind::Transfer), None);
}

/// Every pane's level axis the operator moves is remembered in the preferences with its
/// pane, and the next start comes back to it; Ctrl+Home's defaults are remembered as set by
/// hand too (a spectrum pane keeps them rather than framing its first curve).
#[test]
fn level_axes_are_remembered_for_the_next_start() {
    let mut t = T::new();
    let default = PaneLevels::default();
    let remembered = |t: &T, kind: PaneKind| {
        let pane = t.st.layout.lead(kind).expect("pane");
        let p = t.st.prefs.layout.panes.as_ref().expect("panes");
        let l = p.views.iter().find(|v| v.id == pane).expect("view").levels;
        let r = |r: Option<ac2_proto::model::layout::LevelRange>| r.map(|r| Range::new(r.lo, r.hi));
        (r(l.transfer), r(l.spectrum_dbfs))
    };
    t.key("Alt+2");
    t.st.prefs_dirty = false;
    t.key("Ctrl+Down");
    let spectrum = t.level(PaneKind::Spectrum);
    assert_ne!(spectrum, default.spectrum_dbfs);
    assert!(t.st.prefs_dirty);
    assert_eq!(remembered(&t, PaneKind::Spectrum).1, Some(spectrum));
    t.key("Alt+1");
    t.key("Ctrl+I");
    let transfer = t.level(PaneKind::Transfer);
    assert_ne!(transfer, default.transfer);
    assert_eq!(remembered(&t, PaneKind::Transfer).0, Some(transfer));
    let text = t.st.prefs.to_toml();
    assert!(text.contains("[layout.panes.levels]"), "{text}");
    assert!(!text.contains("[levels]"), "{text}");

    let mut u = T::new();
    u.st.set_prefs(crate::prefs::UiPrefs::from_toml(&text).expect("parse"));
    assert_eq!(u.level(PaneKind::Spectrum), spectrum);
    assert_eq!(u.level(PaneKind::Transfer), transfer);
    let sp = u.st.layout.lead(PaneKind::Spectrum).expect("pane");
    assert_eq!(u.st.pane_levels(sp).spectrum_spl, default.spectrum_spl);
    // Nothing changed by the start itself.
    u.key("Alt+1");
    assert_eq!(remembered(&u, PaneKind::Transfer).0, Some(transfer));
    u.key("Ctrl+Home");
    assert_eq!(remembered(&u, PaneKind::Transfer).0, Some(default.transfer));
}

/// A remapped key shows its new chord on the line; the stage view has no line.
#[test]
fn key_hints_use_the_live_keymap_and_never_show_on_stage() {
    let mut t = T::new();
    t.keys = Keymap::from_toml("[global]\nnext_trace = \"Alt+T\"\nhelp = \"F1\"\n").expect("valid");
    let tf = hint_texts(&t, PaneKind::Transfer).expect("line");
    assert_eq!(tf.first().map(String::as_str), Some("Alt+T select trace"));
    assert_eq!(tf.last().map(String::as_str), Some("F1 all keys"));
    // The stage view: full screen, the SPL pane maximised on its Leq windows.
    t.key("Alt+4");
    t.key("G");
    t.key("W");
    t.key("F11");
    assert!(t.st.stage_view());
    assert!(t.st.prefs.key_hints);
    assert!(!t.st.key_hints_shown());
    assert_eq!(hint_texts(&t, PaneKind::Spl), None);
    // Out of it (W: the split layout), the line is back.
    t.key("W");
    assert!(!t.st.stage_view());
    assert!(hint_texts(&t, PaneKind::Spl).is_some());
}

/// A narrow title keeps the selected stored trace's name: the caption's variants shorten
/// from everything to the name alone.
#[test]
fn pane_caption_shortens_to_the_selected_trace() {
    let mut t = T::new();
    let mut a = stored(10, Some(3), 2);
    a.mic = Some(MicState {
        name: "MM1 34804".into(),
        curve: Some(curve_ref("90°")),
    });
    t.conn(with_traces(vec![a]));
    // Nothing selected: the measurement's smoothing.
    let v = t.st.pane_caption_variants(t.pane(PaneKind::Transfer));
    assert_eq!(
        v.first(),
        t.st.pane_caption(t.pane(PaneKind::Transfer)).as_ref()
    );
    assert!(!v.iter().any(|c| c.contains("t10")), "{v:?}");
    t.st.update(Msg::SelectTrace(TraceId(10)), &t.keys);
    let v = t.st.pane_caption_variants(t.pane(PaneKind::Transfer));
    assert_eq!(
        v,
        [
            "slot 3 (t10): smoothing off · mic curve: MM1 34804 90°",
            "slot 3 (t10): smoothing off",
            "slot 3 (t10)",
        ]
    );
    assert_eq!(
        t.st.pane_caption(t.pane(PaneKind::Transfer)).as_ref(),
        v.first()
    );
    // A transfer trace is not the spectrum pane's.
    assert!(
        !t.st
            .pane_caption_variants(t.pane(PaneKind::Spectrum))
            .iter()
            .any(|c| c.contains("t10"))
    );
}

/// W: split → the focused pane alone → full screen (the stage view, on any pane) → split.
/// F (or F11) alone is the window full screen in whatever layout; with one pane, the stage
/// view.
/// Full screen stays the pane alone with a stimulus armed.
#[test]
fn w_cycles_split_maximised_full_screen() {
    let mut t = T::new();
    assert!(!t.st.layout.maximized && !t.st.fullscreen);
    t.key("W");
    assert!(t.st.layout.maximized && !t.st.fullscreen);
    assert!(!t.st.stage_view());
    t.key("W");
    assert!(t.st.layout.maximized && t.st.fullscreen);
    assert!(t.st.stage_view(), "the transfer pane full screen");
    assert!(!t.st.key_hints_shown());
    t.st.stimulus.phase = StimPhase::Armed;
    assert!(t.st.stage_view());
    t.st.stimulus.phase = StimPhase::Idle;
    t.key("W");
    assert!(!t.st.layout.maximized && !t.st.fullscreen);
    // F11 in the split layout: the window full screen, the layout as it was.
    t.key("F11");
    assert!(t.st.fullscreen && !t.st.layout.maximized && !t.st.stage_view());
    t.key("W");
    assert!(t.st.stage_view());
    t.key("F11");
    assert!(t.st.layout.maximized && !t.st.fullscreen && !t.st.stage_view());
    t.key("F");
    assert!(t.st.stage_view());
    t.key("W");
    assert!(!t.st.layout.maximized && !t.st.fullscreen);
}

/// F: windowed → the window full screen (top bar and lists as before) → the panes alone,
/// every pane of the layout (the stage view: no top bar, lists, pane titles or hints) →
/// windowed; F11 the same. In the panes alone, W steps the focused pane alone and back,
/// staying full screen; F leaves. The step is remembered, and the hidden list still
/// answers Tab.
#[test]
fn f_cycles_window_full_screen_then_panes_only() {
    let mut t = T::new();
    t.conn(mirror(tree_state()));
    let panes = t.st.layout.panes().len();
    assert!(panes > 1);
    t.key("F");
    assert!(t.st.fullscreen && !t.st.panes_only && !t.st.stage_view());
    t.key("F");
    assert!(t.st.fullscreen && t.st.panes_only && !t.st.layout.maximized);
    assert!(t.st.stage_view() && !t.st.key_hints_shown());
    assert_eq!(t.st.visible_tree(), t.st.layout.root, "every pane");
    // The lists are hidden, not gone: Tab still steps the selection.
    t.st.selected = None;
    assert_eq!(tab(&mut t, "Tab"), "m1");
    assert_eq!(tab(&mut t, "Tab"), "l1");
    // W inside: one pane, then all again, full screen throughout.
    t.key("W");
    assert!(t.st.layout.maximized && t.st.stage_view());
    t.key("W");
    assert!(!t.st.layout.maximized && t.st.stage_view());
    // Remembered, and restored on the next start.
    let l = t.st.prefs.layout.clone();
    assert!(l.fullscreen && l.panes_only);
    let mut u = T::disconnected();
    u.st.set_prefs(t.st.prefs.clone());
    assert!(u.st.fullscreen && u.st.panes_only && u.st.stage_view());
    t.key("F11");
    assert!(!t.st.fullscreen && !t.st.panes_only && !t.st.stage_view());
    assert!(!t.st.prefs.layout.panes_only);
    // From W's full-screen pane F leaves full screen, the pane still alone; W's own cycle
    // is as before.
    t.key("W");
    t.key("W");
    assert!(t.st.stage_view() && !t.st.panes_only);
    t.key("F");
    assert!(t.st.layout.maximized && !t.st.fullscreen);
    t.key("W");
    t.key("W");
    assert!(!t.st.layout.maximized && !t.st.fullscreen);
    // Panes-only, then W to one pane, then F: windowed with the pane alone.
    t.key("F");
    t.key("F");
    t.key("W");
    t.key("F");
    assert!(t.st.layout.maximized && !t.st.fullscreen && !t.st.panes_only);
}

/// T steps the focused pane's plot (grid, labels, cursor) and no other pane's, two panes of
/// one kind included; the toast names the pane and the step; a split copies the step, a
/// change of kind keeps it, and each pane's step is kept for the next start.
#[test]
fn t_steps_only_the_focused_plot_and_is_remembered() {
    use ac2_scene::view::PlotChrome;
    let chrome = |t: &T, id: PaneId| t.st.layout.view(id).expect("a pane").modes.chrome;
    let mut t = T::new();
    t.st.prefs_dirty = false;
    let (tf, ir) = (t.pane(PaneKind::Transfer), t.ir_pane());
    // The transfer pane has the keyboard at start.
    t.key("T");
    assert_eq!(chrome(&t, tf), PlotChrome::NoGrid);
    assert_eq!(t.st.view_for(tf).chrome, PlotChrome::NoGrid);
    assert_eq!(t.last_toast(), "Transfer: no grid or marks");
    t.key("Alt+3");
    t.key("T");
    t.key("T");
    assert_eq!(t.last_toast(), "Impulse response: traces only");
    assert_eq!(
        (chrome(&t, tf), chrome(&t, ir)),
        (PlotChrome::NoGrid, PlotChrome::Bare)
    );
    for p in [PaneKind::Spectrum, PaneKind::Spl] {
        assert_eq!(chrome(&t, t.pane(p)), PlotChrome::Full, "{p:?}");
    }
    assert!(t.st.prefs_dirty);
    // A split copies the step; the new IR view then steps alone.
    t.key("N");
    let ir2 = t.st.layout.focus;
    assert_eq!(chrome(&t, ir2), PlotChrome::Bare);
    t.key("T");
    assert_eq!(t.last_toast(), "Impulse response: grid, labels and cursor");
    assert_eq!(
        (chrome(&t, ir), chrome(&t, ir2)),
        (PlotChrome::Bare, PlotChrome::Full)
    );
    // A change of kind keeps the pane's step.
    t.st.layout.set_focus(ir);
    t.show(PaneKind::Spectrum);
    assert_eq!(chrome(&t, ir), PlotChrome::Bare);
    assert_eq!(t.st.view_for(ir).chrome, PlotChrome::Bare);
    // The theme is no longer on T.
    assert_eq!(t.st.theme, T::new().st.theme);
    // The next start draws each pane as it was left.
    let mut v = T::fresh();
    v.st.set_prefs(t.st.prefs.clone());
    assert_eq!(chrome(&v, tf), PlotChrome::NoGrid);
    assert_eq!(chrome(&v, ir), PlotChrome::Bare);
    assert_eq!(chrome(&v, ir2), PlotChrome::Full);
}

/// The layout goes into the preferences whenever it changes, measurements by name; the next
/// start (preferences set before the link) comes back to it, the pane's measurement once
/// the daemon's state is known. One that is gone falls back quietly.
#[test]
fn layout_is_remembered_and_restored() {
    use ac2_scene::view::IrMode;
    let mut state = with_spl();
    state.measurements.push(meas(5, "Stage SPL", spl_meter()));
    let mut t = T::new();
    t.conn(mirror(state.clone()));
    t.st.prefs_dirty = false;
    t.key("Alt+3");
    t.key("G");
    let ir_mode = |t: &T| t.st.layout.view(t.ir_pane()).expect("view").modes.transfer;
    assert_eq!(ir_mode(&t), TransferView::Ir(IrMode::Log));
    t.key("Alt+4");
    t.st.update(Msg::SelectMeas(MeasId(5)), &t.keys);
    assert_eq!(t.st.kind_meas(PaneKind::Spl).map(|m| m.id), Some(MeasId(5)));
    t.key("G");
    t.key("W");
    t.key("W");
    assert!(t.st.prefs_dirty);
    let l = t.st.prefs.layout.clone();
    assert!(l.maximized && l.fullscreen);
    let panes = l.panes.clone().expect("a layout");
    assert_eq!(panes.root, t.st.layout.root);
    assert_eq!(panes.focus, t.pane(PaneKind::Spl));
    let view = |k: PaneKind| panes.views.iter().find(|v| v.kind == k).expect("a pane");
    let ir_view = panes
        .views
        .iter()
        .find(|v| v.modes.transfer.ir().is_some())
        .expect("the IR view");
    // G from the default meter + Leq: the meter alone, remembered.
    assert_eq!(view(PaneKind::Spl).modes.spl, SplMode::Meter);
    assert_eq!(ir_view.modes.transfer, TransferView::Ir(IrMode::Log));
    assert_eq!(
        view(PaneKind::Spl).measurement.as_deref(),
        Some("Stage SPL")
    );
    // Both transfer panes follow the selection: no measurement of their own.
    assert_eq!(view(PaneKind::Transfer).measurement, None);
    assert_eq!(ir_view.measurement, None);
    // Ticks change nothing and write nothing.
    t.st.prefs_dirty = false;
    t.st.update(
        Msg::Tick {
            now_s: 5.0,
            dt_s: 0.1,
        },
        &t.keys,
    );
    assert!(!t.st.prefs_dirty);

    // The next start.
    let prefs = t.st.prefs.clone();
    let connected = ConnEvent::Connected {
        target: "local daemon".into(),
        server: "ac2d test".into(),
        client_id: ClientId("c1".into()),
    };
    let mut u = T::disconnected();
    u.st.set_prefs(prefs.clone());
    assert_eq!(u.focus_kind(), PaneKind::Spl);
    assert!(u.st.layout.maximized && u.st.fullscreen);
    assert_eq!(u.st.kind_modes(PaneKind::Spl).spl, SplMode::Meter);
    assert_eq!(ir_mode(&u), TransferView::Ir(IrMode::Log));
    assert_eq!(u.st.stimulus.phase, StimPhase::Idle);
    // Before the daemon's state, the remembered names stay as they were.
    assert_eq!(u.st.layout_prefs(), prefs.layout);
    u.conn(connected.clone());
    u.conn(mirror(state.clone()));
    assert_eq!(u.st.kind_meas(PaneKind::Spl).map(|m| m.id), Some(MeasId(5)));
    assert!(u.st.stage_view());
    assert_eq!(u.st.layout_prefs(), prefs.layout);

    // The remembered meter is gone: the pane shows its usual choice, nothing is said.
    let spl_entry = |p: &mut crate::prefs::UiPrefs| {
        let panes = p.layout.panes.as_mut().expect("a layout");
        panes
            .views
            .iter_mut()
            .find(|v| v.kind == PaneKind::Spl)
            .map(|v| v.measurement.clone())
    };
    let mut gone = prefs.clone();
    if let Some(v) = gone
        .layout
        .panes
        .as_mut()
        .and_then(|p| p.views.iter_mut().find(|v| v.kind == PaneKind::Spl))
    {
        v.measurement = Some("Gone SPL".into());
    }
    let mut v = T::disconnected();
    v.st.set_prefs(gone);
    v.conn(connected);
    v.conn(mirror(state));
    assert_eq!(v.st.kind_meas(PaneKind::Spl).map(|m| m.id), Some(MeasId(4)));
    assert!(
        !v.st.toasts.iter().any(|t| t.severity != Severity::Info),
        "{:?}",
        v.st.toasts
    );
    // The usual choice is not the operator's: nothing is saved for the pane.
    assert_eq!(spl_entry(&mut v.st.prefs), Some(None));
}

/// The link receives the streams the visible panes draw: the TF of every transfer
/// measurement with the transfer pane shown, the IR of the one the IR pane follows, spectra
/// with the spectrum pane shown or peak hold on, SPL meters always; and frames reach the UI
/// less often when only the SPL pane is in view.
#[test]
fn the_link_receives_what_the_panes_draw() {
    use ac2_proto::topic::{Stream, Topic};
    let mut t = T::new();
    let mut s = four();
    s.measurements.push(meas(5, "SPL", spl_meter()));
    t.conn(mirror(s));
    let topic = |m: u32, stream| Topic::Data {
        meas: MeasId(m),
        stream,
    };
    let want = |v: &[(u32, Stream)]| -> std::collections::HashSet<Topic> {
        v.iter().map(|&(m, s)| topic(m, s)).collect()
    };
    let r = t.st.sync_link();
    assert!(
        r.iter()
            .any(|r| matches!(r, Request::Topics(x) if *x == t.st.wanted_topics())),
        "{r:?}"
    );
    assert!(t.st.sync_link().is_empty(), "sent once");
    let ir_of = crate::scenes::focus_tf(&t.st).map_or(0, |m| m.id.0);
    assert_eq!(
        t.st.wanted_topics(),
        want(&[
            (1, Stream::Tf),
            (3, Stream::Tf),
            (ir_of, Stream::Ir),
            (2, Stream::Spec),
            (4, Stream::Rta),
            (5, Stream::Spl),
            (5, Stream::Leq),
        ])
    );
    assert_eq!(t.st.display_period(), crate::conn::DISPLAY_PERIOD);
    // The IR view closed: no IR.
    t.key("Alt+3");
    t.key("Q");
    assert!(!t.st.wanted_topics().contains(&topic(ir_of, Stream::Ir)));
    assert!(
        t.st.sync_link()
            .iter()
            .any(|r| matches!(r, Request::Topics(_)))
    );
    // The SPL pane alone: only the meter, and frames at its own rate.
    t.go(PaneKind::Spl);
    t.key("W");
    assert!(t.st.layout.maximized);
    assert_eq!(
        t.st.wanted_topics(),
        want(&[(5, Stream::Spl), (5, Stream::Leq)])
    );
    assert_eq!(t.st.display_period(), crate::link_wants::SPL_ONLY_PERIOD);
    // Peak hold folds every spectrum frame, shown or not.
    t.st.view.spectrum.peak_hold = true;
    assert!(t.st.wanted_topics().contains(&topic(2, Stream::Spec)));
    assert!(t.st.wanted_topics().contains(&topic(4, Stream::Rta)));
}

/// The session dialog says when input and output may be on different clocks: an open
/// session playing on another device, or a device whose directions are separate endpoints.
#[test]
fn session_dialog_notes_separate_clocks() {
    let mut t = T::new();
    let mut s = daemon_state();
    if let Some(o) = &mut s.session.open {
        o.backend = BackendKind::Fake;
    }
    t.conn(mirror(s.clone()));
    t.st.update(Msg::Command(CommandId::OpenSession), &t.keys);
    t.conn(ConnEvent::Devices(Ok(backends(true))));
    assert!(dialog(&t).is_open_device());
    assert_eq!(dialog(&t).clock_note(), None);
    t.key("Escape");

    if let Some(o) = &mut s.session.open {
        o.output_device = DeviceId("speakers".into());
        o.clock = ClockRelation::Unknown;
    }
    t.conn(mirror(s));
    t.st.update(Msg::Command(CommandId::OpenSession), &t.keys);
    let mut b = backends(true);
    b[0].devices[0].duplex_clock = ClockRelation::Unknown;
    t.conn(ConnEvent::Devices(Ok(b)));
    assert_eq!(
        dialog(&t).clock_note().as_deref(),
        Some(
            "Output plays on speakers, another device than the input: the two may run on \
             different clocks; the loopback monitor measures their drift while a stimulus plays."
        )
    );
}

/// With nothing selected, a key that needs a measurement names keys the keymap binds:
/// Tab walks the list to one, the palette (Ctrl+K) makes one; N only splits a pane.
#[test]
fn need_meas_names_the_keys_that_reach_a_measurement() {
    let mut t = T::new();
    t.st.selected = None;
    t.st.selected_trace = None;
    assert!(
        t.st.need_meas(&[MeasKind::publishes_tf], "transfer")
            .is_none()
    );
    let w = t.last_toast().to_owned();
    for (c, chord) in [
        (CommandId::NextInTree, "Tab"),
        (CommandId::NewMeasurement, "Ctrl+M"),
    ] {
        let want = Chord::parse(chord).expect(chord);
        assert_eq!(t.keys.first_chord(c, Scope::Global), Some(want), "{c:?}");
        assert!(w.contains(chord), "{w}");
    }
    assert!(!w.contains("(N)"), "{w}");
}

#[path = "state_layout_tree_tests.rs"]
mod tree;
