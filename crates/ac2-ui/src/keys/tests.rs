use eframe::egui::{Key, Modifiers};

use super::chord::is_symbol;
use super::*;

#[test]
fn defaults_are_valid() {
    Keymap::default().validate().expect("default keymap");
}

#[test]
fn no_conflicts_in_any_scope() {
    let m = Keymap::default();
    for scope in Scope::ALL {
        let mut seen = std::collections::HashMap::new();
        for b in m
            .bindings()
            .iter()
            .filter(|b| b.scope == Scope::Global || b.scope == scope)
        {
            if let Some(prev) = seen.insert(b.chord, b.command) {
                assert_eq!(prev, b.command, "{} in {scope:?}", b.chord);
            }
        }
    }
}

#[test]
fn no_layout_unsafe_defaults() {
    for b in defaults() {
        assert!(
            !layout_unsafe(b.chord.key),
            "{:?} uses {:?}",
            b.command,
            b.chord.key
        );
        assert!(!(b.chord.shift && is_symbol(b.chord.key)));
    }
    for s in ["[", "]", "+", "-"] {
        assert!(
            defaults().iter().all(|b| b.chord.key.symbol_or_name() != s),
            "{s}"
        );
    }
}

#[test]
fn every_binding_is_in_a_scope_where_it_acts() {
    for b in defaults() {
        assert!(b.command.scopes().contains(&b.scope), "{b:?}");
    }
}

#[test]
fn every_command_is_reachable() {
    // Every command has a key or is deliberately palette-only.
    let palette_only = [
        CommandId::StimulusOutputs,
        CommandId::StimulusTakeOver,
        CommandId::Reconnect,
        CommandId::WarningToasts,
        CommandId::CloseSession,
        CommandId::Record,
        CommandId::ReplayRecording,
        CommandId::NewTransfer,
        CommandId::NewSpectrum,
        CommandId::NewRta,
        CommandId::NewSpl,
        CommandId::EditMeas,
        CommandId::PrevColour,
        CommandId::ToggleGroup,
        CommandId::AverageComplex,
        CommandId::ClearCompare,
        CommandId::AverageCoherence,
        CommandId::LegendTopLeft,
        CommandId::LegendTopRight,
        CommandId::LegendBottomLeft,
        CommandId::LegendBottomRight,
        CommandId::LegendHide,
        CommandId::ImportTrace,
        // Z's chooser offers each; the palette reaches them directly.
        CommandId::TargetFile,
        CommandId::TargetFlat,
        CommandId::TargetTilt,
        CommandId::TargetLoudness,
        CommandId::SessionNew,
        CommandId::SessionSave,
        CommandId::SessionSaveAs,
        CommandId::SessionLoad,
        CommandId::SessionExport,
        CommandId::InputSetup,
        CommandId::Calibrations,
        CommandId::InputMics,
        CommandId::MicCurve,
        CommandId::MicCurveInput,
        CommandId::CalDelete,
        CommandId::TraceMicCurve,
        CommandId::TraceSlot,
        CommandId::TraceRename,
        CommandId::TraceExport,
        CommandId::FinderAuto,
        CommandId::FinderFull,
        CommandId::FinderMid,
        CommandId::FinderSub,
        CommandId::FinderCustom,
        CommandId::FinderObservation,
        CommandId::PaneMeasurement,
        CommandId::TransferStack,
        CommandId::SplFast,
        CommandId::SplSlow,
        CommandId::SplImpulse,
        CommandId::SplA,
        CommandId::SplC,
        CommandId::SplZ,
        CommandId::SmoothOff,
        CommandId::Smooth48,
        CommandId::Smooth24,
        CommandId::Smooth12,
        CommandId::Smooth6,
        CommandId::Smooth3,
        CommandId::SplShowMeter,
        CommandId::SplShowLeq,
        CommandId::SplShowMeterLeq,
        CommandId::SplShowBands,
        // Settings › Display has the theme row; T is the plot's grid / labels / cursor.
        CommandId::CycleTheme,
        // Set once and left: Settings › Display has a row for each, so their keys went
        // to what operators press while measuring.
        CommandId::KeyHints,
        CommandId::CoherencePlacement,
        CommandId::SplLeqStyle,
        CommandId::SplLeqHistory,
        CommandId::DistortionUnit,
        CommandId::SpectrographColours,
        CommandId::SpectrographViridis,
        CommandId::SpectrographMagma,
        CommandId::SpectrographInferno,
        CommandId::SpectrographCividis,
        CommandId::SpectrographGrey,
        // Once a show: the palette's "export leq".
        CommandId::SplLogExport,
        CommandId::SplLogExportPrevious,
        // For a report, now and then: the palette's "export pane".
        CommandId::ExportPanePng,
        // The top bar's rig drop-down is a click; the operator's desktop takes the free
        // Ctrl chords and the plain keys are measuring keys.
        CommandId::Rigs,
        CommandId::ConnectRig,
    ];
    let m = Keymap::default();
    for c in CommandId::ALL {
        let bound = m.bindings().iter().any(|b| b.command == *c);
        assert!(bound || palette_only.contains(c), "{c:?} has no key");
    }
}

#[test]
fn learned_bindings_carry_over() {
    let m = Keymap::default();
    let t = Scope::Transfer;
    let c = |s: &str| Chord::parse(s).expect(s);
    for (chord, cmd) in [
        ("X", CommandId::InsertDelay),
        ("Y", CommandId::TrackDelay),
        ("U", CommandId::Invert),
        ("J", CommandId::Offset),
        ("Z", CommandId::Target),
        ("B", CommandId::CoherenceMask),
        ("M", CommandId::Average),
        ("Shift+P", CommandId::PhaseView),
        ("Ctrl+1", CommandId::Slot1),
        ("Ctrl+9", CommandId::Slot9),
        ("1", CommandId::ShowSlot1),
        ("9", CommandId::ShowSlot9),
        ("Alt+2", CommandId::FocusPane2),
        ("Alt+9", CommandId::FocusPane9),
        ("N", CommandId::SplitPane),
        ("Q", CommandId::ClosePane),
        ("G", CommandId::NextView),
        ("Shift+G", CommandId::PrevView),
        ("Space", CommandId::StimulusArm),
        ("Enter", CommandId::StimulusFire),
        ("Esc", CommandId::StimulusStop),
        ("Shift+Esc", CommandId::StopAnywhere),
        ("Up", CommandId::LevelUp),
        ("Down", CommandId::LevelDown),
        ("H", CommandId::Help),
        ("F1", CommandId::Help),
        ("Ctrl+K", CommandId::Palette),
        ("Shift+O", CommandId::OpenSession),
        ("O", CommandId::ZoomOut),
        ("K", CommandId::SmoothCoarser),
        ("Shift+K", CommandId::SmoothFiner),
        ("V", CommandId::NextTrace),
        ("Shift+V", CommandId::PrevTrace),
        ("Alt+V", CommandId::NextAnyTrace),
        ("Alt+Shift+V", CommandId::PrevAnyTrace),
        ("A", CommandId::ToggleSelected),
    ] {
        assert_eq!(m.lookup(t, c(chord)), Some(cmd), "{chord}");
    }
    // Scoped lookup: P tracks extremes in both plots — peak hold and peaks on the
    // spectrum, min / max on the transfer function; Shift+P is the phase view there.
    assert_eq!(m.lookup(Scope::Spectrum, c("P")), Some(CommandId::PeakHold));
    assert_eq!(m.lookup(Scope::Transfer, c("P")), Some(CommandId::MinMax));
    assert_eq!(
        m.lookup(Scope::Transfer, c("Shift+P")),
        Some(CommandId::PhaseView)
    );
    // H is help in every pane; Shift+I is the impulse response wherever there is one.
    for scope in Scope::ALL {
        assert_eq!(m.lookup(scope, c("H")), Some(CommandId::Help), "{scope:?}");
    }
    assert_eq!(
        m.lookup(Scope::Distortion, c("Shift+I")),
        Some(CommandId::SweepIr)
    );
    assert_eq!(m.lookup(Scope::Global, c("/")), None);
    assert_eq!(m.lookup(Scope::Spl, c("X")), None);
    // The SPL meter's weightings; Z loads a target in transfer.
    assert_eq!(
        m.lookup(Scope::Spl, c("Shift+F")),
        Some(CommandId::SplTimeWeighting)
    );
    assert_eq!(m.lookup(Scope::Spl, c("Z")), Some(CommandId::SplWeighting));
    // F is full screen in every pane.
    for scope in Scope::ALL {
        assert_eq!(
            m.lookup(scope, c("F")),
            Some(CommandId::Fullscreen),
            "{scope:?}"
        );
    }
}

/// Offsets, the level axis and Delete: modifiers on the stimulus arrows (which stay the
/// stimulus level's alone) and beside the frequency axis's I / O / Home.
#[test]
fn offset_level_and_delete_keys() {
    let m = Keymap::default();
    let c = |s: &str| Chord::parse(s).expect(s);
    for scope in Scope::ALL {
        assert_eq!(m.lookup(scope, c("Up")), Some(CommandId::LevelUp));
        assert_eq!(
            m.lookup(scope, c("Shift+Down")),
            Some(CommandId::LevelDownCoarse)
        );
        assert_eq!(m.lookup(scope, c("Alt+Up")), Some(CommandId::OffsetUp));
        assert_eq!(m.lookup(scope, c("Alt+Down")), Some(CommandId::OffsetDown));
        assert_eq!(
            m.lookup(scope, c("Alt+Shift+Up")),
            Some(CommandId::OffsetUpCoarse)
        );
        assert_eq!(
            m.lookup(scope, c("Alt+Shift+Down")),
            Some(CommandId::OffsetDownCoarse)
        );
        assert_eq!(m.lookup(scope, c("Alt+Home")), Some(CommandId::OffsetClear));
        assert_eq!(
            m.lookup(scope, c("Delete")),
            Some(CommandId::DeleteSelected)
        );
        assert_eq!(
            m.lookup(scope, c("Backspace")),
            Some(CommandId::DeleteSelected)
        );
        assert_eq!(m.lookup(scope, c("Home")), Some(CommandId::ResetView));
        for (k, cmd) in [
            ("Ctrl+Up", CommandId::LevelPanUp),
            ("Ctrl+Down", CommandId::LevelPanDown),
            ("Ctrl+I", CommandId::LevelZoomIn),
            ("Ctrl+O", CommandId::LevelZoomOut),
            ("Shift+Home", CommandId::LevelFit),
            ("Ctrl+Home", CommandId::LevelReset),
            ("I", CommandId::ZoomIn),
            ("O", CommandId::ZoomOut),
        ] {
            assert_eq!(m.lookup(scope, c(k)), Some(cmd), "{k} {scope:?}");
        }
    }
    assert_eq!(m.lookup(Scope::Spectrum, c("J")), Some(CommandId::Offset));
}

#[test]
fn symbols_ignore_shift() {
    // `/` arrives as Shift+7 → Slash with Shift on Nordic layouts.
    let shifted = Modifiers {
        shift: true,
        ..Modifiers::NONE
    };
    let ch = Chord::from_event(Key::Slash, shifted);
    // Bound in keys.toml, `/` matches however the layout produces it.
    let m = Keymap::from_toml("[global]\nhelp = [\"H\", \"/\"]").expect("valid");
    assert_eq!(m.lookup(Scope::Global, ch), Some(CommandId::Help));
    assert!(Chord::parse("Shift+/").is_err());
    // Letters keep Shift.
    assert!(Chord::from_event(Key::P, shifted).shift);
}

/// ⌘Tab is the macOS app switcher: there Control+Tab arrives as the command chord and
/// is labelled `⌃Tab`; on PC keyboards Ctrl is the command key anyway.
#[test]
fn control_tab_is_the_command_chord() {
    let mac_ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::NONE
    };
    let pc_ctrl = Modifiers {
        ctrl: true,
        command: true,
        ..Modifiers::NONE
    };
    let ctrl_tab = Chord::parse("Ctrl+Tab").expect("valid");
    assert_eq!(Chord::from_event(Key::Tab, mac_ctrl), ctrl_tab);
    assert_eq!(Chord::from_event(Key::Tab, pc_ctrl), ctrl_tab);
    // Elsewhere macOS Control is no command key.
    assert!(!Chord::from_event(Key::K, mac_ctrl).command);
    let m = Keymap::default();
    // Desktops take Ctrl+Tab (window and workspace switching): nothing is bound to it.
    assert_eq!(m.lookup(Scope::Global, ctrl_tab), None);
    let tab = Chord::key(Key::Tab);
    assert_eq!(m.lookup(Scope::Global, tab), Some(CommandId::NextInTree));
    let back = Chord::parse("Ctrl+Shift+Tab").expect("valid");
    assert_eq!(back.label_in(LabelStyle::Mac), "⌃⇧Tab");
    assert_eq!(back.label_in(LabelStyle::Pc), "Ctrl+Shift+Tab");
    assert_eq!(Chord::parse("⌃⇧Tab"), Ok(back));
}

#[test]
fn parse_and_label() {
    for (s, label) in [
        ("ctrl+k", "Ctrl+K"),
        ("Cmd+K", "Ctrl+K"),
        ("Shift+P", "Shift+P"),
        ("space", "Space"),
        ("Up", "↑"),
        ("/", "/"),
        (",", ","),
        ("F1", "F1"),
        ("Alt+Left", "Alt+←"),
        ("Home", "Home"),
        ("Ctrl+Tab", "Ctrl+Tab"),
    ] {
        let c = Chord::parse(s).expect(s);
        assert_eq!(c.label_in(LabelStyle::Pc), label, "{s}");
        assert_eq!(
            Chord::parse(&c.label_in(LabelStyle::Pc)).ok(),
            Some(c),
            "{s}"
        );
        assert_eq!(
            Chord::parse(&c.label_in(LabelStyle::Mac)).ok(),
            Some(c),
            "{s}"
        );
    }
    assert_eq!(Chord::parse("⌘⇧K"), Chord::parse("Ctrl+Shift+K"));
    assert_eq!(Chord::parse("⌥←"), Chord::parse("Alt+Left"));
    assert!(Chord::parse("Hyper+K").is_err());
    assert!(Chord::parse("Ctrl+Nope").is_err());
}

#[test]
fn toml_overrides() {
    let m = Keymap::from_toml(
        r#"
            [transfer]
            insert_delay = "Shift+Q"
            invert = ["U", "I"]

            [global]
            zoom_in = "Alt+I"
            "#,
    );
    // I is zoom_in globally by default... but zoom_in moved to Alt+I, so I is free.
    let m = m.expect("valid");
    let c = |s: &str| Chord::parse(s).expect(s);
    assert_eq!(
        m.lookup(Scope::Transfer, c("Shift+Q")),
        Some(CommandId::InsertDelay)
    );
    assert_eq!(m.lookup(Scope::Transfer, c("X")), None);
    assert_eq!(m.lookup(Scope::Transfer, c("I")), Some(CommandId::Invert));
    assert_eq!(
        m.lookup(Scope::Spectrum, c("Alt+I")),
        Some(CommandId::ZoomIn)
    );
    // Unbinding with an empty list.
    let m = Keymap::from_toml("[transfer]\ntarget = []").expect("valid");
    assert_eq!(m.lookup(Scope::Transfer, c("Z")), None);
}

#[test]
fn toml_errors() {
    let bad = [
        ("[transfer]\ninsert_delay = \"U\"", "bound to both"),
        ("[global]\nhelp = \"Space\"", "reserved"),
        ("[global]\nstimulus_stop = \"Shift+Q\"", "Esc must stay"),
        ("[global]\nstimulus_stop_anywhere = \"Shift+Q\"", "is fixed"),
        (
            "[global]\nstimulus_stop_anywhere = [\"Shift+Esc\", \"Ctrl+S\"]",
            "is fixed",
        ),
        ("[global]\nhelp = \"Shift+Esc\"", "reserved"),
        ("[spectrum]\ninsert_delay = \"Shift+Q\"", "does nothing"),
        ("[nowhere]\nhelp = \"Shift+Q\"", "unknown scope"),
        ("[global]\nfly = \"Shift+Q\"", "unknown command"),
        ("[global]\nhelp = \"Ctrl+Nope\"", "unknown key"),
        ("[global]\nhelp = 3", "keys.toml"),
        // H is help in every pane: a pane's command on H conflicts with it there.
        ("[spectrum]\npeak_hold = \"H\"", "bound to both"),
    ];
    for (src, want) in bad {
        let e = Keymap::from_toml(src).expect_err(src);
        assert!(e.contains(want), "{src}: {e}");
    }
    // A global override must not collide with any scope's binding either.
    let e = Keymap::from_toml("[global]\nzoom_in = \"X\"").expect_err("conflict");
    assert!(e.contains("transfer"), "{e}");
}

#[test]
fn load_falls_back_to_defaults() {
    let dir = std::env::temp_dir().join(format!("ac2-ui-keys-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let p = dir.join("keys.toml");
    let (m, e) = Keymap::load(Some(&p));
    assert_eq!(m, Keymap::default());
    assert!(e.is_none());
    std::fs::write(&p, "[global]\nhelp = \"Space\"").expect("write");
    let (m, e) = Keymap::load(Some(&p));
    assert_eq!(m, Keymap::default());
    assert!(e.expect("error").contains("using default keys"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn key_hints() {
    let m = Keymap::default();
    assert_eq!(
        m.key_hint(CommandId::InsertDelay, Scope::Transfer)
            .as_deref(),
        Some("X")
    );
    assert_eq!(
        m.key_hint(CommandId::InsertDelay, Scope::Spectrum)
            .as_deref(),
        Some("X (transfer)")
    );
    assert_eq!(m.key_hint(CommandId::Reconnect, Scope::Global), None);
    assert_eq!(
        m.key_hint(CommandId::Help, Scope::Ir).as_deref(),
        Some("H · F1")
    );
    assert_eq!(
        m.first_chord(CommandId::ClosePane, Scope::Ir),
        Chord::parse("Q").ok()
    );
    assert_eq!(
        m.first_chord(CommandId::MaximisePane, Scope::Spl),
        Chord::parse("W").ok()
    );
    assert_eq!(m.first_chord(CommandId::SweepIr, Scope::Spl), None);
}

/// N splits and Q closes in every pane, plain letters the same on every layout; no
/// pane takes either for its own, nor is either a stimulus key. Desktops take Ctrl+Tab,
/// Ctrl+Shift+Tab and Ctrl+D: nothing is bound to them, nor to Ctrl+N. Ctrl+Q quits.
#[test]
fn split_and_close_are_plain_letters_everywhere() {
    let m = Keymap::default();
    let c = |s: &str| Chord::parse(s).expect(s);
    for scope in Scope::ALL {
        assert_eq!(
            m.lookup(scope, c("N")),
            Some(CommandId::SplitPane),
            "{scope:?}"
        );
        assert_eq!(
            m.lookup(scope, c("Q")),
            Some(CommandId::ClosePane),
            "{scope:?}"
        );
        assert_eq!(
            m.lookup(scope, c("Ctrl+Q")),
            Some(CommandId::Quit),
            "{scope:?}"
        );
        for gone in ["Ctrl+N", "Ctrl+D", "Ctrl+Tab", "Ctrl+Shift+Tab"] {
            assert_eq!(m.lookup(scope, c(gone)), None, "{gone} in {scope:?}");
        }
    }
    assert_eq!(
        m.lookup(Scope::Transfer, c("D")),
        Some(CommandId::TypeDelay)
    );
    for chord in [c("N"), c("Q")] {
        assert!(RESERVED.iter().all(|(r, _)| *r != chord), "{chord:?}");
    }
    let bound = |cmd| m.bindings().iter().filter(|b| b.command == cmd).count();
    assert_eq!(bound(CommandId::SplitPane), 1);
    assert_eq!(bound(CommandId::ClosePane), 1);
}

/// Shift+N turns and Alt+Right / Alt+Left size the focused pane in every pane: no pane
/// binds those chords for its own, and Alt+Up / Down stay the trace offset.
#[test]
fn turn_and_resize_are_the_same_everywhere() {
    let m = Keymap::default();
    let c = |s: &str| Chord::parse(s).expect(s);
    for scope in Scope::ALL {
        for (chord, cmd) in [
            ("Shift+N", CommandId::TurnSplit),
            ("Alt+Right", CommandId::GrowPane),
            ("Alt+Left", CommandId::ShrinkPane),
            ("Alt+Up", CommandId::OffsetUp),
            ("Alt+Down", CommandId::OffsetDown),
        ] {
            assert_eq!(m.lookup(scope, c(chord)), Some(cmd), "{chord} in {scope:?}");
        }
    }
    for chord in ["Shift+N", "Alt+Right", "Alt+Left"] {
        let n = m.bindings().iter().filter(|b| b.chord == c(chord)).count();
        assert_eq!(n, 1, "{chord}");
        assert!(RESERVED.iter().all(|(r, _)| *r != c(chord)), "{chord}");
    }
}

/// G steps every plot pane's views forward and Shift+G back, the Programme pane's meters and
/// goniometer too; Shift+B is the spectrograph's history length. An Inputs row has no view.
#[test]
fn g_steps_the_views_of_every_pane() {
    let m = Keymap::default();
    let c = |s: &str| Chord::parse(s).expect(s);
    for scope in Scope::ALL {
        let (g, shift_g) = if matches!(scope, Scope::Global | Scope::Inputs) {
            (None, None)
        } else {
            (Some(CommandId::NextView), Some(CommandId::PrevView))
        };
        assert_eq!(m.lookup(scope, c("G")), g, "{scope:?}");
        assert_eq!(m.lookup(scope, c("Shift+G")), shift_g, "{scope:?}");
    }
    assert_eq!(
        m.lookup(Scope::Spectrum, c("Shift+B")),
        Some(CommandId::SpectrographSpan)
    );
    assert_eq!(m.lookup(Scope::Global, c("Shift+B")), None);
    // One binding of Shift+B anywhere: no scope's own key is shadowed.
    let n = m
        .bindings()
        .iter()
        .filter(|b| b.chord == c("Shift+B"))
        .count();
    assert_eq!(n, 1);
}

#[test]
fn pane_hints_are_bound_where_they_are_shown() {
    let m = Keymap::default();
    for scope in Scope::ALL {
        let h = hints(scope);
        if scope == Scope::Global {
            assert!(h.is_empty());
            continue;
        }
        assert!((4..=8).contains(&h.len()), "{scope:?}: {} hints", h.len());
        for x in h {
            assert!(
                x.command.scopes().contains(&scope) || x.command.scopes().contains(&Scope::Global),
                "{:?} does nothing in {scope:?}",
                x.command
            );
            assert!(
                m.first_chord(x.command, scope).is_some(),
                "{:?} has no key in {scope:?}",
                x.command
            );
            assert!(x.priority < u8::MAX, "the help hint keeps the top priority");
        }
        let mut cmds: Vec<_> = h.iter().map(|x| x.command).collect();
        cmds.sort();
        cmds.dedup();
        assert_eq!(cmds.len(), h.len(), "{scope:?} lists a command twice");
    }
}

/// The stop chord: the same in every scope, unique, layout-safe and never typed text.
#[test]
fn stop_anywhere_is_safe_on_every_layout() {
    let m = Keymap::default();
    assert_eq!(Chord::parse("Shift+Esc"), Ok(STOP_ANYWHERE));
    assert!(!layout_unsafe(STOP_ANYWHERE.key) && !is_symbol(STOP_ANYWHERE.key));
    const { assert!(!(STOP_ANYWHERE.command || STOP_ANYWHERE.alt)) };
    for scope in Scope::ALL {
        assert_eq!(
            m.lookup(scope, STOP_ANYWHERE),
            Some(CommandId::StopAnywhere),
            "{scope:?}"
        );
        let holders: Vec<_> = m
            .bindings()
            .iter()
            .filter(|b| (b.scope == Scope::Global || b.scope == scope) && b.chord == STOP_ANYWHERE)
            .collect();
        assert_eq!(holders.len(), 1, "{scope:?}");
    }
    // Nordic layouts send Esc and Shift as such: the chord of the event is the stop.
    let shifted = Modifiers {
        shift: true,
        ..Modifiers::NONE
    };
    assert_eq!(Chord::from_event(Key::Escape, shifted), STOP_ANYWHERE);
    assert_eq!(STOP_ANYWHERE.label_in(LabelStyle::Pc), "Shift+Esc");
    // Keeping it and adding nothing is fine.
    Keymap::from_toml("[global]\nstimulus_stop_anywhere = \"Shift+Esc\"").expect("valid");
}

/// Message text names a command by its chord in the label style, else by its palette
/// entry, the palette named by its own key while it has one.
#[test]
fn message_text_names_keys_in_the_label_style() {
    use LabelStyle::{Mac, Pc};
    let m = Keymap::default();
    assert_eq!(m.reach_in(CommandId::SweepNew, Pc), "Shift+S");
    assert_eq!(m.reach_in(CommandId::SweepNew, Mac), "⇧S");
    assert_eq!(m.reach_in(CommandId::NewSpl, Mac), "New SPL meter… in ⌘K");
    assert_eq!(
        m.label_in(CommandId::SweepIr, Scope::Distortion, Mac)
            .as_deref(),
        Some("⇧I")
    );
    assert_eq!(m.label_in(CommandId::SweepIr, Scope::Transfer, Pc), None);
    let bare = Keymap::from_toml("[global]\npalette = []\nsweep_new = []\n").expect("valid");
    assert_eq!(
        bare.reach_in(CommandId::SweepNew, Mac),
        "New sweep… in the command palette"
    );
}

/// Palette lines are labels, not a manual: short, no asides, every title its own, and one
/// shape per genre so a family reads (and searches) alike.
#[test]
fn palette_titles_are_terse_and_uniform() {
    use CommandId as C;
    let mut seen = std::collections::HashSet::new();
    for &c in CommandId::ALL {
        let t = c.title();
        assert!(t.chars().count() <= 28, "too long: {t}");
        assert!(!t.contains(['(', ')', ':', ';']), "aside in: {t}");
        assert!(
            !t.contains(" / ") && !t.contains('→'),
            "manual text in: {t}"
        );
        assert!(!t.ends_with("...") && t.matches('…').count() <= 1, "{t}");
        assert!(t.chars().next().is_some_and(char::is_uppercase), "{t}");
        assert!(seen.insert(t), "two commands read {t}");
    }
    for c in [
        C::NewMeasurement,
        C::NewTransfer,
        C::NewSpectrum,
        C::NewRta,
        C::NewSpl,
        C::NewMath,
        C::SweepNew,
    ] {
        let t = c.title();
        assert!(t.starts_with("New ") && t.ends_with('…'), "{t}");
    }
    for (c, t) in [
        (C::SessionLoad, "Open session…"),
        (C::SessionSave, "Save session"),
        (C::SessionSaveAs, "Save session as…"),
        (C::SessionExport, "Export session…"),
        (C::SplitPane, "Split pane"),
        (C::ClosePane, "Close pane"),
        (C::MaximisePane, "Maximise pane"),
        (C::PeakHold, "Peaks"),
        (C::CoherenceMask, "Coherence mask"),
        (C::PhaseView, "Phase view"),
    ] {
        assert_eq!(c.title(), t);
    }
}
