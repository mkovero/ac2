//! Reducer tests of key hints, captions, the pane layout, what the link receives and the measurement tree.

use super::*;

fn hint_texts(t: &T, pane: PaneKind) -> Option<Vec<String>> {
    t.st.key_hint_line(&t.keys, pane, crate::keys::LabelStyle::Pc)
        .map(|v| v.iter().map(crate::hints::KeyHint::text).collect())
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
    for p in [PaneKind::Spectrum, PaneKind::Ir, PaneKind::Spl] {
        assert_eq!(hint_texts(&t, p), None, "{p:?} is not focused");
    }
    t.key("Alt+2");
    assert_eq!(hint_texts(&t, PaneKind::Transfer), None);
    let sp = hint_texts(&t, PaneKind::Spectrum).expect("spectrum focused");
    assert!(sp.contains(&"P peak hold".to_owned()), "{sp:?}");
    t.key("Alt+3");
    let ir = hint_texts(&t, PaneKind::Ir).expect("IR focused");
    assert!(ir.contains(&"G linear/log/ETC".to_owned()), "{ir:?}");
    t.key("Alt+4");
    let spl = hint_texts(&t, PaneKind::Spl).expect("SPL focused");
    assert_eq!(spl[..3], ["G meter/Leq/both", "Shift+F F/S/I", "Z A/C/Z"]);
    // The sweep pane names dB / % while it shows distortion, the IR mode while it shows the IR,
    // and G its views in each.
    t.key("Alt+5");
    let d = hint_texts(&t, PaneKind::Distortion).expect("sweep pane focused");
    assert!(d.contains(&"U dB/%".to_owned()), "{d:?}");
    assert!(d.contains(&"G response/IR/room".to_owned()), "{d:?}");
    assert!(!d.contains(&"Shift+G linear/log/ETC".to_owned()), "{d:?}");
    t.key("Shift+I");
    let d = hint_texts(&t, PaneKind::Distortion).expect("sweep pane focused");
    assert!(!d.contains(&"U dB/%".to_owned()), "{d:?}");
    assert!(d.contains(&"Shift+G linear/log/ETC".to_owned()), "{d:?}");
    // Mac labels.
    let mac: Vec<String> =
        t.st.key_hint_line(&t.keys, PaneKind::Distortion, crate::keys::LabelStyle::Mac)
            .expect("line")
            .iter()
            .map(crate::hints::KeyHint::text)
            .collect();
    assert_eq!(mac.first().map(String::as_str), Some("⇧S new sweep"));

    // Off: no line anywhere, remembered, and the toast says how to bring it back.
    t.st.prefs_dirty = false;
    t.key("Shift+H");
    assert!(!t.st.prefs.key_hints);
    assert!(t.st.prefs_dirty);
    assert!(t.last_toast().contains("Shift+H"), "{}", t.last_toast());
    for p in PaneKind::ALL {
        assert_eq!(hint_texts(&t, p), None);
    }
    // The title's tooltip still lists the pane's hints.
    assert!(
        !t.st
            .pane_hints(&t.keys, PaneKind::Spl, crate::keys::LabelStyle::Pc)
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

/// Every pane's level axis is remembered in the preferences as it moves, and the next start
/// comes back to it; Ctrl+Home's defaults are remembered as such (left out of the file).
#[test]
fn level_axes_are_remembered_for_the_next_start() {
    let mut t = T::new();
    let default = crate::prefs::LevelPrefs::default();
    t.key("Alt+2");
    t.st.prefs_dirty = false;
    t.key("Ctrl+Down");
    let spectrum = t.st.view.spectrum.level;
    assert_ne!(spectrum, default.spectrum_dbfs);
    assert!(t.st.prefs_dirty);
    assert_eq!(t.st.prefs.levels.spectrum_dbfs, spectrum);
    t.key("Alt+1");
    t.key("Ctrl+I");
    let transfer = t.st.view.tf.magnitude_db;
    assert_ne!(transfer, default.transfer);
    assert_eq!(t.st.prefs.levels.transfer, transfer);
    let text = t.st.prefs.to_toml();
    assert!(text.contains("[levels]"), "{text}");

    let mut u = T::new();
    u.st.set_prefs(crate::prefs::UiPrefs::from_toml(&text).expect("parse"));
    assert_eq!(u.st.view.spectrum.level, spectrum);
    assert_eq!(u.st.view.tf.magnitude_db, transfer);
    assert_eq!(u.st.view.spectrum.level_spl, default.spectrum_spl);
    // Nothing changed by the start itself.
    u.key("Alt+1");
    assert_eq!(u.st.prefs.levels.transfer, transfer);
    u.key("Ctrl+Home");
    assert_eq!(u.st.prefs.levels.transfer, default.transfer);
    assert!(!u.st.prefs.to_toml().contains("transfer = ["));
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
    let v = t.st.pane_caption_variants(PaneKind::Transfer);
    assert_eq!(v.first(), t.st.pane_caption(PaneKind::Transfer).as_ref());
    assert!(!v.iter().any(|c| c.contains("t10")), "{v:?}");
    t.st.update(Msg::SelectTrace(TraceId(10)), &t.keys);
    let v = t.st.pane_caption_variants(PaneKind::Transfer);
    assert_eq!(
        v,
        [
            "slot 3 (t10): smoothing off · mic curve: MM1 34804 90°",
            "slot 3 (t10): smoothing off",
            "slot 3 (t10)",
        ]
    );
    assert_eq!(t.st.pane_caption(PaneKind::Transfer).as_ref(), v.first());
    // A transfer trace is not the spectrum pane's.
    assert!(
        !t.st
            .pane_caption_variants(PaneKind::Spectrum)
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

/// T steps the focused pane's plot (grid, labels, cursor) and no other pane's; the toast
/// names the pane and the step; each pane's step is kept for the next start.
#[test]
fn t_steps_only_the_focused_plot_and_is_remembered() {
    use ac2_scene::view::{PaneChrome, PlotChrome};
    let mut t = T::new();
    t.st.prefs_dirty = false;
    // The transfer pane has the keyboard at start.
    t.key("T");
    assert_eq!(
        t.st.view.chrome,
        PaneChrome {
            transfer: PlotChrome::NoGrid,
            ..PaneChrome::default()
        }
    );
    assert_eq!(t.last_toast(), "Transfer: no grid");
    t.key("Alt+3");
    t.key("T");
    t.key("T");
    assert_eq!(t.last_toast(), "Impulse response: traces only");
    let want = PaneChrome {
        transfer: PlotChrome::NoGrid,
        ir: PlotChrome::Bare,
        ..PaneChrome::default()
    };
    assert_eq!(t.st.view.chrome, want);
    assert!(t.st.prefs_dirty);
    assert_eq!(t.st.prefs.layout.chrome, want);
    // A third step wraps to the full plot.
    t.key("T");
    assert_eq!(t.st.view.chrome.ir, PlotChrome::Full);
    assert_eq!(t.last_toast(), "Impulse response: grid, labels and cursor");
    // The theme is no longer on T.
    assert_eq!(t.st.theme, T::new().st.theme);
    // The next start draws each pane as it was left.
    let mut v = T::new();
    v.st.set_prefs(t.st.prefs.clone());
    assert_eq!(v.st.view.chrome.transfer, PlotChrome::NoGrid);
    assert_eq!(v.st.view.chrome.ir, PlotChrome::Full);
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
    assert_eq!(t.st.view.ir.mode, IrMode::Log);
    t.key("Alt+4");
    t.key("N");
    assert_eq!(t.st.pane_meas(PaneKind::Spl).map(|m| m.id), Some(MeasId(5)));
    t.key("G");
    t.key("W");
    t.key("W");
    assert!(t.st.prefs_dirty);
    let l = t.st.prefs.layout.clone();
    assert_eq!(l.focus, PaneKind::Spl);
    assert!(l.maximized && l.fullscreen);
    // G from the default meter + Leq: the meter alone, remembered.
    assert_eq!(l.spl_view, SplMode::Meter);
    assert_eq!(l.ir_mode, IrMode::Log);
    assert_eq!(
        l.measurements.get(&PaneKind::Spl).map(String::as_str),
        Some("Stage SPL")
    );
    assert_eq!(
        l.measurements.get(&PaneKind::Transfer).map(String::as_str),
        Some("Main L")
    );
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
    assert_eq!(u.st.layout.focus, PaneKind::Spl);
    assert!(u.st.layout.maximized && u.st.fullscreen);
    assert_eq!(u.st.view.spl.mode, SplMode::Meter);
    assert_eq!(u.st.view.ir.mode, IrMode::Log);
    assert_eq!(u.st.stimulus.phase, StimPhase::Idle);
    // Before the daemon's state, the remembered names stay as they were.
    assert_eq!(u.st.layout_prefs(), prefs.layout);
    u.conn(connected.clone());
    u.conn(mirror(state.clone()));
    assert_eq!(u.st.pane_meas(PaneKind::Spl).map(|m| m.id), Some(MeasId(5)));
    assert!(u.st.stage_view());
    assert_eq!(u.st.layout_prefs(), prefs.layout);

    // The remembered meter is gone: the pane shows its usual choice, nothing is said.
    let mut gone = prefs.clone();
    gone.layout
        .measurements
        .insert(PaneKind::Spl, "Gone SPL".into());
    let mut v = T::disconnected();
    v.st.set_prefs(gone);
    v.conn(connected);
    v.conn(mirror(state));
    assert_eq!(v.st.pane_meas(PaneKind::Spl).map(|m| m.id), Some(MeasId(4)));
    assert!(
        !v.st.toasts.iter().any(|t| t.severity != Severity::Info),
        "{:?}",
        v.st.toasts
    );
    assert_eq!(
        v.st.prefs
            .layout
            .measurements
            .get(&PaneKind::Spl)
            .map(String::as_str),
        Some("FOH SPL")
    );
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
    // The IR pane hidden: no IR.
    t.key("Shift+I");
    assert!(!t.st.wanted_topics().contains(&topic(ir_of, Stream::Ir)));
    assert!(
        t.st.sync_link()
            .iter()
            .any(|r| matches!(r, Request::Topics(_)))
    );
    // The SPL pane alone: only the meter, and frames at its own rate.
    t.key("Alt+4");
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

fn tree_names(t: &T) -> Vec<String> {
    t.st.tree_rows().iter().map(|r| r.name.clone()).collect()
}

/// Every tree row that stands for a curve has a dot in the colour the panes draw that curve
/// in (live curves, stored traces, math results alike), a ring once it is hidden; the
/// headers have none.
#[test]
fn tree_dots_have_the_colours_of_their_curves() {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::frame::{SpecFrame, SpecMeta};
    use ac2_proto::{Frame, FrameData};
    use ac2_scene::primitives::{Color, Viewport};
    let mut s = tree_state();
    for (i, t) in s.traces.iter_mut().enumerate() {
        let v = 40 * (i as u8 + 1);
        t.edit.color = Rgb {
            r: v,
            g: 255 - v,
            b: 7,
        };
    }
    let metas = s.traces.clone();
    let mut t = T::new();
    t.conn(mirror(s));
    for m in &metas {
        let grid = GridDef::Log {
            ppo: 1,
            k_min: -5,
            k_max: 2,
        };
        t.conn(ConnEvent::Trace(
            Arc::new(TraceData {
                meta: m.clone(),
                mag_db: vec![-10.0; 8],
                phase_deg: None,
                coherence: None,
                sweep: None,
            }),
            Arc::new(grid),
        ));
    }
    // Live frames of Main L, of the math channel on it and of Sub.
    let spec_grid = GridDef::Linear {
        fs: Hz(48_000.0),
        n: 14,
    };
    let mut math = ac2_proto::samples::tf_frame();
    if let FrameData::Tf(f) = &mut math.data {
        f.meas = MeasId(6);
    }
    let spec = Frame {
        stamp: ac2_proto::samples::stamp(Some(spec_grid.clone())),
        data: FrameData::Spec(SpecFrame {
            meas: MeasId(2),
            meta: SpecMeta {
                window: Window::Hann,
                scale: LevelScale::Dbfs,
                cal: CalStatus::Uncalibrated,
                mic_curve: false,
                smoothing: None,
                math: None,
            },
            level: vec![-60.0; 8],
        }),
    };
    let mut latest = Latest::default();
    for f in [ac2_proto::samples::tf_frame(), math, spec] {
        let f = TopicFrame {
            topic: f.data.topic(),
            frame: Arc::new(f),
            received: Instant::now(),
            since_new: std::time::Duration::ZERO,
            age: Some(0.0),
            stale: false,
        };
        latest.frames.insert(f.topic.to_string().into(), f);
    }
    let mut grids = std::collections::BTreeMap::new();
    for g in [ac2_proto::samples::log_grid(), spec_grid] {
        grids.insert(g.id(), Arc::new(g));
    }
    t.conn(ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids,
        drained: Instant::now(),
    })));
    // The import is under no measurement: the transfer pane draws it when compared.
    t.st.compared_traces.insert(TraceId(5));

    let theme = ac2_scene::theme::Theme::by_name(t.st.theme);
    let size = Viewport {
        width: 1000.0,
        height: 450.0,
    };
    let now = crate::scenes::Now {
        instant: Instant::now(),
        wall: WallNs(0),
    };
    // The colour each curve is drawn in, by its name, from both panes.
    let mut drawn: Vec<(String, Color)> = crate::scenes::transfer(&t.st, &theme, size, now)
        .traces
        .iter()
        .map(|d| (d.name.clone(), d.color))
        .collect();
    drawn.extend(crate::scenes::with_spectrum(
        &t.st,
        &theme,
        now,
        |traces, _, _| {
            traces
                .iter()
                .map(|d| (d.name.clone(), d.color))
                .collect::<Vec<_>>()
        },
    ));
    let curve = |name: &str| {
        drawn
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("{name} drawn: {drawn:?}"))
            .1
    };
    let rows = t.st.tree_rows();
    let dot = |name: &str| {
        rows.iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("{name} listed"))
            .dot
    };
    for (row, name) in [
        ("Main L (live)", "Main L"),
        ("Sub (live)", "Sub"),
        ("pre ÷ post", "pre ÷ post"),
        ("pre-EQ", "pre-EQ"),
        ("post-EQ", "post-EQ"),
        ("1083 94cm", "1083 94cm"),
    ] {
        assert_eq!(dot(row), Some((curve(name), true)), "{row}");
    }
    let colours: std::collections::BTreeSet<[u8; 3]> = rows
        .iter()
        .filter_map(|r| r.dot)
        .map(|(c, _)| [c.r, c.g, c.b].map(|v| (v * 255.0).round() as u8))
        .collect();
    assert_eq!(colours.len(), 6, "each curve its own colour");
    // Each measurement its own colour family: Main L's live curve, captures and math result
    // share its hue, Sub has another, the import takes a hue no measurement holds, and the
    // daemon's per-trace colour is not what is drawn.
    let hue = |c: Color| ac2_scene::families::oklch(c).2.to_degrees();
    let gap = |a: Color, b: Color| {
        let d = (hue(a) - hue(b)).rem_euclid(360.0);
        d.min(360.0 - d)
    };
    let main = curve("Main L");
    assert_eq!(main, theme.families[0]);
    assert_eq!(curve("Sub"), theme.families[1]);
    for n in ["pre-EQ", "post-EQ", "pre ÷ post"] {
        assert!(gap(curve(n), main) < 8.0, "{n} in Main L's family");
    }
    assert_eq!(curve("1083 94cm"), theme.families[2]);
    for m in &metas {
        let c = m.edit.color;
        assert_ne!(curve(&m.edit.name), Color::from_rgba8([c.r, c.g, c.b, 255]));
    }
    for r in rows.iter().filter(|r| r.depth == 0) {
        assert_eq!(r.dot, None, "{} is a group", r.name);
    }
    // Hidden: a ring of the same colour.
    let sub = curve("Sub");
    t.st.update(Msg::ToggleMeasShown(MeasId(2)), &t.keys);
    let rows = t.st.tree_rows();
    let row = rows.iter().find(|r| r.name == "Sub (live)").expect("row");
    assert_eq!(row.dot, Some((sub, false)));
    t.st.update(Msg::ToggleMeasShown(MeasId(6)), &t.keys);
    let rows = t.st.tree_rows();
    let row = rows.iter().find(|r| r.name == "pre ÷ post").expect("row");
    assert_eq!(row.dot.map(|d| d.1), Some(false));

    // Moved under Sub, the import takes Sub's family.
    let mut moved = tree_state();
    moved.traces[2].edit.owner = TraceOwner::Meas { meas: MeasId(2) };
    let mut u = T::new();
    u.conn(mirror(moved));
    let rows = u.st.tree_rows();
    let row = rows.iter().find(|r| r.name == "1083 94cm").expect("row");
    let (c, _) = row.dot.expect("dot");
    assert!(gap(c, sub) < 8.0, "{c:?} in Sub's family");
    assert_ne!(c, sub, "a shade, not Sub's live curve");
}

#[test]
fn the_tree_lists_what_each_measurement_owns_and_folds() {
    let mut t = T::new();
    t.conn(mirror(tree_state()));
    assert_eq!(
        tree_names(&t),
        [
            "TF  Main L",
            "Main L (live)",
            "pre-EQ",
            "post-EQ",
            "pre ÷ post",
            "FFT  Sub",
            "Sub (live)",
            "Imported",
            "1083 94cm"
        ]
    );
    // V steps through the traces in the tree's order.
    t.key("V");
    assert_eq!(t.st.selected_trace, Some(TraceId(3)));
    t.key("V");
    assert_eq!(t.st.selected_trace, Some(TraceId(4)));
    // Folding Main L hides its rows; its arrow (or the palette command) unfolds it.
    t.st.update(
        Msg::ToggleGroup(TraceOwner::Meas { meas: MeasId(1) }),
        &t.keys,
    );
    assert_eq!(
        tree_names(&t),
        [
            "TF  Main L",
            "FFT  Sub",
            "Sub (live)",
            "Imported",
            "1083 94cm"
        ]
    );
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    t.st.update(Msg::Command(CommandId::ToggleGroup), &t.keys);
    assert_eq!(tree_names(&t).len(), 9);
}

/// Shift+A on a measurement hides its live curve, its traces and its math channel; again
/// shows them.
#[test]
fn shift_a_hides_a_measurement_with_everything_under_it() {
    let mut t = T::new();
    t.conn(mirror(tree_state()));
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    let r = t.key("Shift+A");
    let hidden: Vec<TraceId> = r
        .iter()
        .filter_map(|x| match x {
            Request::Call {
                cmd: Command::TraceUpdate { trace, edit },
                ..
            } if !edit.visible => Some(*trace),
            _ => None,
        })
        .collect();
    assert_eq!(hidden, [TraceId(3), TraceId(4)]);
    assert!(t.st.hidden_meas.contains("Main L"));
    assert!(t.st.hidden_meas.contains("pre ÷ post"));
    assert!(!t.st.hidden_meas.contains("Sub"));
    // The daemon hid them: Shift+A again shows the whole group.
    let mut s = tree_state();
    for tr in &mut s.traces[..2] {
        tr.edit.visible = false;
    }
    t.conn(mirror(s));
    let r = t.key("Shift+A");
    let shown = r
        .iter()
        .filter(|x| {
            matches!(x, Request::Call { cmd: Command::TraceUpdate { edit, .. }, .. } if edit.visible)
        })
        .count();
    assert_eq!(shown, 2);
    assert!(!t.st.hidden_meas.contains("Main L"));
}

/// Shift+F2 asks where the selected trace goes; Enter files it there (a `trace.update` with
/// its new owner). Where it is now cannot be picked.
#[test]
fn move_files_a_trace_under_another_measurement() {
    let mut t = T::new();
    t.conn(mirror(tree_state()));
    t.st.update(Msg::SelectTrace(TraceId(5)), &t.keys);
    t.key("Shift+F2");
    let Overlay::Choose(c) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    assert_eq!(c.title, "Move 1083 94cm to…");
    let labels: Vec<&str> = c.choices.iter().map(|x| x.label.as_str()).collect();
    assert_eq!(
        labels,
        ["TF  Main L", "FFT  Sub", "Imported (under no measurement)"]
    );
    assert!(c.choices[2].blocked.is_some(), "it is filed there");
    assert_eq!(c.index, 0);
    let r = t.key("Enter");
    match r.as_slice() {
        [
            Request::Call {
                cmd: Command::TraceUpdate { trace, edit },
                what,
            },
        ] => {
            assert_eq!(*trace, TraceId(5));
            assert_eq!(edit.owner, TraceOwner::Meas { meas: MeasId(1) });
            assert_eq!(what, "1083 94cm moved to Main L");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(t.st.overlay, Overlay::None);
    // A math channel moves with `meas.update`.
    t.st.update(Msg::SelectMeas(MeasId(6)), &t.keys);
    t.key("Shift+F2");
    t.key("Down");
    t.key("Down");
    let r = t.key("Enter");
    let moved = match r.as_slice() {
        [
            Request::Call {
                cmd: Command::MeasUpdate { meas, config },
                ..
            },
        ] => (*meas, config.kind.clone()),
        other => panic!("{other:?}"),
    };
    assert_eq!(moved.0, MeasId(6));
    assert!(matches!(moved.1, MeasKind::Math { config } if config.owner == TraceOwner::Imported));
}

/// Deleting a measurement that owns traces asks every time: keep them (the default),
/// delete them too, or cancel — keyboard only.
#[test]
fn deleting_a_measurement_with_traces_asks_keep_delete_or_cancel() {
    let mut t = T::new();
    t.conn(mirror(tree_state()));
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    assert!(t.key("Delete").is_empty());
    let Overlay::Choose(c) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    assert_eq!(c.title, "Delete measurement Main L?");
    assert_eq!(
        c.lines[0],
        "transfer function · running · it has 2 traces and 1 math channel."
    );
    assert_eq!(c.index, ac2_scene::meas_list::KEEP);
    // Esc cancels; nothing is sent.
    assert!(t.key("Esc").is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
    let deleted = |r: &[Request]| match r {
        [
            Request::Call {
                cmd: Command::MeasDelete { meas, traces },
                ..
            },
        ] => Some((*meas, *traces)),
        _ => None,
    };
    // Enter keeps them.
    t.key("Delete");
    let r = t.key("Enter");
    assert_eq!(deleted(&r), Some((MeasId(1), OwnedTraces::Keep)));
    // → then Enter deletes them too; → → then Enter cancels.
    t.key("Delete");
    t.key("Right");
    let r = t.key("Enter");
    assert_eq!(deleted(&r), Some((MeasId(1), OwnedTraces::Delete)));
    t.key("Delete");
    t.key("Right");
    t.key("Right");
    assert!(t.key("Enter").is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
}

/// Shift+M files the new math channel under the measurement selected, and offers its live
/// curve and traces first.
#[test]
fn a_new_math_channel_lives_under_the_selected_measurement() {
    let mut t = T::new();
    t.conn(mirror(tree_state()));
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    t.key("Shift+M");
    let Overlay::Form(f) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    let m = f.math.as_ref().expect("math dialog");
    assert_eq!(m.owner, TraceOwner::Meas { meas: MeasId(1) });
    let first: Vec<&str> = m
        .candidates
        .iter()
        .take(3)
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(first, ["Main L", "pre-EQ", "post-EQ"]);
    let r = t.key("Enter");
    let c = r
        .iter()
        .find_map(|x| match x {
            Request::CreateMeas { config } => Some(config.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{r:?}"));
    assert!(matches!(&c.kind, MeasKind::Math { config }
        if config.owner == TraceOwner::Meas { meas: MeasId(1) }));
}

fn tab(t: &mut T, key: &str) -> Option<u32> {
    t.key(key);
    assert_eq!(
        t.st.selected_trace, None,
        "{key} lands on the measurement, not a trace"
    );
    t.st.selected.map(|m| m.0)
}

/// Tab / Shift+Tab walk the measurements in the tree's order (math channels included, the
/// traces and the Imported header skipped), wrapping; Ctrl+Tab steps the panes.
#[test]
fn tab_steps_through_the_trees_measurements() {
    let mut t = T::new();
    t.conn(mirror(tree_state()));
    t.st.selected = None;
    // Main L, its math channel pre ÷ post, Sub; then round again.
    let fwd: Vec<_> = (0..4).map(|_| tab(&mut t, "Tab")).collect();
    assert_eq!(fwd, [Some(1), Some(6), Some(2), Some(1)]);
    let back: Vec<_> = (0..4).map(|_| tab(&mut t, "Shift+Tab")).collect();
    assert_eq!(back, [Some(2), Some(6), Some(1), Some(2)]);
    assert!(t.st.tree_reveal, "the tree scrolls to the row");

    // Nothing selected: Shift+Tab starts at the last.
    t.st.selected = None;
    assert_eq!(tab(&mut t, "Shift+Tab"), Some(2));

    // The focus goes to a pane that draws the selection, as a click on its row.
    assert_eq!(t.st.layout.focus, PaneKind::Spectrum);
    assert_eq!(tab(&mut t, "Tab"), Some(1));
    assert_eq!(t.st.layout.focus, PaneKind::Transfer);

    // The panes moved to Ctrl+Tab.
    let before = t.st.layout.focus;
    t.key("Ctrl+Tab");
    assert_ne!(t.st.layout.focus, before);
    t.key("Ctrl+Shift+Tab");
    assert_eq!(t.st.layout.focus, before);
}

/// From a selected trace, Tab goes on from the measurement that owns it; from an imported
/// trace (the tree's last group) it starts over.
#[test]
fn tab_from_a_trace_goes_on_from_its_measurement() {
    let mut t = T::new();
    t.conn(mirror(tree_state()));
    t.st.update(Msg::SelectTrace(TraceId(4)), &t.keys);
    assert_eq!(tab(&mut t, "Tab"), Some(6));
    t.st.update(Msg::SelectTrace(TraceId(3)), &t.keys);
    assert_eq!(tab(&mut t, "Shift+Tab"), Some(2));
    t.st.update(Msg::SelectTrace(TraceId(5)), &t.keys);
    assert_eq!(tab(&mut t, "Tab"), Some(1));
    t.st.update(Msg::SelectTrace(TraceId(5)), &t.keys);
    assert_eq!(tab(&mut t, "Shift+Tab"), Some(2));
}

/// A folded group's measurements are still stepped through; a math channel's folded group
/// opens to show it, a header stays folded (its row is on screen).
#[test]
fn tab_reaches_measurements_in_folded_groups() {
    let mut t = T::new();
    t.conn(mirror(tree_state()));
    let main = TraceOwner::Meas { meas: MeasId(1) };
    let sub = TraceOwner::Meas { meas: MeasId(2) };
    t.st.collapsed.insert(main);
    t.st.collapsed.insert(sub);
    t.st.selected = Some(MeasId(1));
    assert_eq!(tab(&mut t, "Tab"), Some(6));
    assert!(!t.st.collapsed.contains(&main));
    assert!(tree_names(&t).contains(&"pre ÷ post".to_owned()));
    assert_eq!(tab(&mut t, "Tab"), Some(2));
    assert!(t.st.collapsed.contains(&sub));
}

/// No measurements: a toast says so and nothing is selected.
#[test]
fn tab_without_measurements_says_so() {
    let mut t = T::new();
    let mut s = daemon_state();
    s.measurements.clear();
    t.conn(mirror(s));
    t.st.selected = None;
    t.key("Tab");
    assert_eq!(t.st.selected, None);
    assert_eq!(t.last_toast(), "no measurements to step through");
}
