use super::*;

#[test]
fn round_trip_is_one_based_in_the_file() {
    let mut p = UiPrefs::default();
    p.outputs.insert("hw:UMC1820".into(), vec![0, 1]);
    p.outputs.insert("fake:loop".into(), vec![3]);
    p.sessions.insert(
        "coreaudio/hw:UMC1820".into(),
        DeviceRoles {
            inputs: vec![0, 1, 2],
            outputs: 2,
            reference: Some(0),
            mics: vec![1, 2],
            stimulus: vec![0],
            mic_names: [(1, "M30 FOH".to_owned()), (2, "ECM8000".to_owned())].into(),
            output_device: Some("hw:Out".into()),
        },
    );
    let text = p.to_toml();
    assert!(text.contains("\"hw:UMC1820\" = [1, 2]"), "{text}");
    assert!(text.contains("reference = 1"), "{text}");
    assert!(text.contains("mics = [2, 3]"), "{text}");
    assert!(text.contains("2 = \"M30 FOH\""), "{text}");
    assert_eq!(UiPrefs::from_toml(&text), Ok(p));
    assert_eq!(UiPrefs::from_toml(""), Ok(UiPrefs::default()));
}

#[test]
fn leq_layout_round_trips() {
    let mut p = UiPrefs::default();
    assert_eq!(
        p.leq,
        LeqLayout {
            style: LeqStyle::Columns,
            history: false
        }
    );
    // The default is not written.
    assert!(!p.to_toml().contains("[leq]"));
    p.leq = LeqLayout {
        style: LeqStyle::Tiles,
        history: true,
    };
    let text = p.to_toml();
    assert!(text.contains("[leq]"), "{text}");
    assert!(text.contains("style = \"tiles\""), "{text}");
    assert!(text.contains("history = true"), "{text}");
    assert_eq!(UiPrefs::from_toml(&text), Ok(p));
    let q = UiPrefs::from_toml("[leq]\nstyle = \"columns\"\nhistory = true\n").expect("parse");
    assert_eq!(q.leq.style, LeqStyle::Columns);
    assert!(q.leq.history);
    assert!(UiPrefs::from_toml("[leq]\nstyle = \"bars\"\n").is_err());
}

#[test]
fn an_unknown_setting_is_dropped_and_the_rest_kept() {
    let p = UiPrefs::from_toml("key_hints = false\nno_such_setting = true\n").expect("loads");
    assert!(!p.key_hints);
    assert!(!p.to_toml().contains("no_such_setting"));
}

#[test]
fn warning_toasts_round_trip() {
    let mut p = UiPrefs::default();
    assert!(p.warning_toasts);
    // On is the default and is not written.
    assert!(!p.to_toml().contains("warning_toasts"));
    p.warning_toasts = false;
    let text = p.to_toml();
    assert!(text.contains("warning_toasts = false"), "{text}");
    assert_eq!(UiPrefs::from_toml(&text), Ok(p));
    assert!(UiPrefs::from_toml("warning_toasts = \"off\"\n").is_err());
}

#[test]
fn resolution_marker_round_trip() {
    let mut p = UiPrefs::default();
    assert!(p.resolution_marker);
    assert!(!p.to_toml().contains("resolution_marker"));
    p.resolution_marker = false;
    let text = p.to_toml();
    assert!(text.contains("resolution_marker = false"), "{text}");
    assert_eq!(UiPrefs::from_toml(&text), Ok(p));
}

#[test]
fn spec_ease_round_trip() {
    let mut p = UiPrefs::default();
    assert!(p.spec_ease);
    assert!(!p.to_toml().contains("spec_ease"));
    p.spec_ease = false;
    let text = p.to_toml();
    assert!(text.contains("spec_ease = false"), "{text}");
    assert_eq!(UiPrefs::from_toml(&text), Ok(p));
}

#[test]
fn bar_ease_round_trip() {
    let mut p = UiPrefs::default();
    assert!(p.bar_ease);
    assert!(!p.to_toml().contains("bar_ease"));
    p.bar_ease = false;
    let text = p.to_toml();
    assert!(text.contains("bar_ease = false"), "{text}");
    assert_eq!(UiPrefs::from_toml(&text), Ok(p));
}

#[test]
fn key_hints_round_trip() {
    let mut p = UiPrefs::default();
    assert!(p.key_hints);
    // On is the default and is not written.
    assert!(!p.to_toml().contains("key_hints"));
    p.key_hints = false;
    p.outputs.insert("fake:loop".into(), vec![0]);
    let text = p.to_toml();
    assert!(text.contains("key_hints = false"), "{text}");
    assert_eq!(UiPrefs::from_toml(&text), Ok(p));
    assert!(
        UiPrefs::from_toml("key_hints = true\n")
            .expect("parse")
            .key_hints
    );
    assert!(UiPrefs::from_toml("key_hints = \"no\"\n").is_err());
}

/// Two panes side by side: a transfer pane on `Main L` and an SPL pane on `FOH SPL`
/// showing the Leq windows, the SPL pane focused.
fn two_panes() -> PanesPrefs {
    PanesPrefs {
        root: PaneNode::Split {
            axis: Axis::Row,
            ratio: 0.5,
            a: Box::new(PaneNode::Leaf(PaneId(1))),
            b: Box::new(PaneNode::Leaf(PaneId(2))),
        },
        focus: PaneId(2),
        views: vec![
            PanePrefs {
                id: PaneId(1),
                kind: PaneKind::Transfer,
                measurement: Some("Main L".to_owned()),
                modes: PaneModes {
                    transfer: TransferView::Ir(IrMode::Log),
                    ..PaneModes::default()
                },
                levels: PaneLevels::default(),
            },
            PanePrefs {
                id: PaneId(2),
                kind: PaneKind::Spl,
                measurement: Some("FOH SPL".to_owned()),
                modes: PaneModes {
                    transfer: TransferView::Coherence,
                    spl: SplMode::Leq,
                    spectrum: SpectrumMode::Spectrograph,
                    sweep: SweepMode::Ir(IrMode::Etc),
                    sweep_reference: false,
                    chrome: PlotChrome::Bare,
                },
                levels: PaneLevels::default(),
            },
        ],
    }
}

#[test]
fn layout_window_and_hold_round_trip() {
    let mut p = UiPrefs::default();
    let text = p.to_toml();
    assert!(
        !text.contains("[layout]") && !text.contains("[window]"),
        "{text}"
    );
    p.spl_hold_ms = Some(250);
    p.layout = LayoutPrefs {
        panes: Some(two_panes()),
        maximized: true,
        fullscreen: true,
        panes_only: true,
        distortion_unit: DistortionUnit::Percent,
        hidden: ["TF 2".to_owned()].into(),
        compared: ["Main R".to_owned()].into(),
    };
    p.window = Some(WindowPrefs {
        width: 1600,
        height: 900,
        pos: Some((80, -20)),
    });
    let text = p.to_toml();
    for want in [
        "spl_hold_ms = 250",
        "[layout]",
        "focus = 2",
        "fullscreen = true",
        "panes_only = true",
        "distortion_unit = \"percent\"",
        "hidden = [\"TF 2\"]",
        "compared = [\"Main R\"]",
        "[layout.tree]",
        "split = \"row\"",
        "[[layout.panes]]",
        "kind = \"spl\"",
        "measurement = \"FOH SPL\"",
        "spl_view = \"leq\"",
        "spectrum_view = \"spectrograph\"",
        "transfer_view = \"ir_log\"",
        "sweep_view = \"etc\"",
        "sweep_reference = false",
        "chrome = \"traces_only\"",
        "[window]",
        "y = -20",
    ] {
        assert!(text.contains(want), "{want} in {text}");
    }
    assert_eq!(UiPrefs::from_toml(&text), Ok(p.clone()));
    // No position (Wayland): size only.
    p.window = Some(WindowPrefs {
        width: 1000,
        height: 700,
        pos: None,
    });
    assert_eq!(UiPrefs::from_toml(&p.to_toml()), Ok(p));
    for bad in [
        "spl_hold_ms = 5\n",
        "spl_hold_ms = 20000\n",
        "[window]\nwidth = 100\nheight = 100\n",
    ] {
        assert!(UiPrefs::from_toml(bad).is_err(), "{bad}");
    }
}

/// A layout that is not a pane tree — tables of panes by kind, a pane twice in the tree,
/// an unknown view, a ratio not a number, two entries of one pane — is dropped: the rest
/// of the file stays and the app starts with one pane.
#[test]
fn an_unreadable_layout_starts_with_one_pane() {
    for old in [
        "[layout]\nfocus = \"spl\"\nspl_view = \"leq\"\n\n[layout.measurements]\nspl = \"FOH\"\n",
        "[layout]\nfocus = 1\n\n[layout.tree]\nsplit = \"row\"\nratio = 0.5\na = { pane = 1 }\nb = { pane = 1 }\n",
        "[layout]\n\n[layout.tree]\npane = 1\n\n[[layout.panes]]\nid = 1\nkind = \"spl\"\nspl_view = \"bars\"\n",
        "[layout]\n\n[layout.tree]\npane = 1\n\n[[layout.panes]]\nid = 1\nkind = \"spl\"\nchrome = \"bare\"\n",
        // The IR's display is part of the view's name: an `ir` view without one, or
        // an `ir_mode` beside the view, names no view.
        "[layout]\n\n[layout.tree]\npane = 1\n\n[[layout.panes]]\nid = 1\nkind = \"transfer\"\ntransfer_view = \"ir\"\n",
        "[layout]\n\n[layout.tree]\npane = 1\n\n[[layout.panes]]\nid = 1\nkind = \"distortion\"\nsweep_view = \"ir\"\n",
        "[layout]\n\n[layout.tree]\npane = 1\n\n[[layout.panes]]\nid = 1\nkind = \"transfer\"\nir_mode = \"log\"\n",
        // `response` names no view: magnitude, phase and coherence together are the transfer
        // stack, three panes.
        "[layout]\n\n[layout.tree]\npane = 1\n\n[[layout.panes]]\nid = 1\nkind = \"transfer\"\ntransfer_view = \"response\"\n",
        "[layout]\nfocus = 1\n\n[layout.tree]\nsplit = \"row\"\nratio = nan\na = { pane = 1 }\nb = { pane = 2 }\n",
        "[layout]\n\n[layout.tree]\npane = 1\n\n[[layout.panes]]\nid = 1\nkind = \"spl\"\n\n[[layout.panes]]\nid = 1\nkind = \"transfer\"\n",
    ] {
        let text = format!("spl_hold_ms = 250\n{old}");
        let q = UiPrefs::from_toml(&text).expect(&text);
        assert_eq!(q.layout.panes, None, "{old}");
        assert_eq!(q.spl_hold_ms, Some(250));
    }
    // A pane of the tree without its entry shows a transfer pane: the tree stays.
    let q = UiPrefs::from_toml("[layout]\nfocus = 1\n\n[layout.tree]\npane = 1\n").expect("parse");
    let panes = q.layout.panes.expect("tree");
    assert_eq!(panes.root, PaneNode::Leaf(PaneId(1)));
    assert!(panes.views.is_empty());
}

/// Each pane keeps its own views, written by name; a pane written without them takes
/// the defaults (the meter with the Leq windows, the spectrum).
#[test]
fn pane_views_default_and_keep_a_choice() {
    assert_eq!(PaneModes::default().spl, SplMode::MeterLeq);
    assert_eq!(PaneModes::default().spectrum, SpectrumMode::Spectrum);
    let q = UiPrefs::from_toml(
        "[layout]\n\n[layout.tree]\npane = 1\n\n[[layout.panes]]\nid = 1\nkind = \"spl\"\n",
    )
    .expect("parse");
    let v = &q.layout.panes.expect("tree").views[0];
    assert_eq!((v.kind, v.modes), (PaneKind::Spl, PaneModes::default()));
    for (spl, name) in [
        (SplMode::Meter, "meter"),
        (SplMode::Leq, "leq"),
        (SplMode::MeterLeq, "meter_leq"),
        (SplMode::Bands, "bands"),
    ] {
        let mut p = UiPrefs::default();
        let mut panes = two_panes();
        panes.views[1].modes.spl = spl;
        p.layout.panes = Some(panes);
        let text = p.to_toml();
        assert!(text.contains(&format!("spl_view = \"{name}\"")), "{text}");
        assert_eq!(UiPrefs::from_toml(&text).expect("parse"), p);
    }
    for (mode, name) in [
        (SpectrumMode::Spectrum, "spectrum"),
        (SpectrumMode::Split, "spectrum_spectrograph"),
        (SpectrumMode::Spectrograph, "spectrograph"),
    ] {
        let mut p = UiPrefs::default();
        let mut panes = two_panes();
        panes.views[0].kind = PaneKind::Spectrum;
        panes.views[0].modes.spectrum = mode;
        p.layout.panes = Some(panes);
        let text = p.to_toml();
        assert!(
            text.contains(&format!("spectrum_view = \"{name}\"")),
            "{text}"
        );
        assert_eq!(UiPrefs::from_toml(&text).expect("parse"), p);
    }
    // Every view of the transfer and sweep chains, the IR's displays among them.
    let names = [
        "magnitude",
        "phase",
        "coherence",
        "ir_linear",
        "ir_log",
        "etc",
    ];
    for (view, name) in TransferView::ALL.into_iter().zip(names) {
        let mut p = UiPrefs::default();
        let mut panes = two_panes();
        panes.views[0].modes.transfer = view;
        p.layout.panes = Some(panes);
        let text = p.to_toml();
        assert!(
            text.contains(&format!("transfer_view = \"{name}\"")),
            "{text}"
        );
        assert_eq!(UiPrefs::from_toml(&text).expect("parse"), p);
    }
    let names = ["response", "ir_linear", "ir_log", "etc", "room"];
    for (mode, name) in SweepMode::ALL.into_iter().zip(names) {
        let mut p = UiPrefs::default();
        let mut panes = two_panes();
        panes.views[0].kind = PaneKind::Distortion;
        panes.views[0].modes.sweep = mode;
        p.layout.panes = Some(panes);
        let text = p.to_toml();
        assert!(text.contains(&format!("sweep_view = \"{name}\"")), "{text}");
        assert_eq!(UiPrefs::from_toml(&text).expect("parse"), p);
    }
}

#[test]
fn bad_files_are_reported() {
    assert!(UiPrefs::from_toml("[stimulus_outputs]\nx = [0]\n").is_err());
    assert!(UiPrefs::from_toml("[stimulus_outputs]\nx = []\n").is_err());
    assert!(UiPrefs::from_toml("[sessions.\"fake/x\"]\nmics = [0]\n").is_err());
    assert!(UiPrefs::from_toml("[sessions.\"fake/x\"]\nmic_names = { a = \"M\" }\n").is_err());
    assert!(UiPrefs::from_toml("[sessions.\"fake/x\"]\ncolour = 1\n").is_err());
    assert!(UiPrefs::from_toml("[ir_levels]\ntransfer = [10.0, -10.0]\n").is_err());
    assert!(UiPrefs::from_toml("[ir_levels]\nsweep = [-10.0]\n").is_err());
    assert!(UiPrefs::from_toml("[ir_levels]\nphase = [-10.0, 10.0]\n").is_err());
}

/// The impulse-response dB axes and each pane's own level axes moved off their defaults
/// are written, the rest left out; an out-of-limits range is brought within the axis
/// limits on reading.
#[test]
fn level_ranges_round_trip() {
    let d = UiPrefs::default();
    assert!(!d.to_toml().contains("levels"));
    let mut p = UiPrefs::default();
    p.ir_levels.sweep = Range::new(-90.0, 0.0);
    let mut panes = two_panes();
    panes.views[0].levels.transfer = Range::new(-24.5, 12.0);
    panes.views[1].levels.spectrum_dbfs = Range::new(-140.0, -40.0);
    p.layout.panes = Some(panes);
    let text = p.to_toml();
    assert!(text.contains("[ir_levels]"), "{text}");
    assert!(text.contains("sweep = [-90.0, 0.0]"), "{text}");
    assert!(!text.contains("\ntransfer = [-80"), "{text}");
    assert!(
        text.contains("[layout.panes.levels]\ntransfer = [-24.5, 12.0]"),
        "{text}"
    );
    assert!(
        text.contains("[layout.panes.levels]\nspectrum_dbfs = [-140.0, -40.0]"),
        "{text}"
    );
    assert!(!text.contains("spectrum_spl"), "{text}");
    assert!(!text.contains("distortion ="), "{text}");
    assert_eq!(UiPrefs::from_toml(&text), Ok(p));
    let far = UiPrefs::from_toml(
        "[layout.tree]\npane = 1\n\n[[layout.panes]]\nid = 1\nkind = \"distortion\"\n\
         levels = { distortion = [-900.0, -800.0], transfer = [3.0, -3.0] }\n",
    )
    .expect("parse");
    let v = far.layout.panes.expect("panes").views[0].levels;
    assert_eq!(v.distortion, Range::new(-300.0, -200.0));
    // Low above high is no range: that one is the default, the layout stays.
    assert_eq!(v.transfer, PaneLevels::default().transfer);
}

/// A kind-wide `[levels]` table is no setting: logged and left out (the panes start on
/// their defaults), and the rest of the file reads.
#[test]
fn a_kind_wide_levels_table_is_ignored() {
    let p = UiPrefs::from_toml("theme = \"light\"\n\n[levels]\ntransfer = [-24.0, 12.0]\n")
        .expect("parse");
    assert_eq!(p.theme, Some(ac2_scene::theme::ThemeName::Light));
    assert_eq!(p.ir_levels, IrLevelPrefs::default());
    assert!(!p.to_toml().contains("levels"));
}

/// A legend moved or resized is written, its defaults left out; out of bounds is an
/// error.
#[test]
fn legend_round_trip() {
    assert!(!UiPrefs::default().to_toml().contains("legend"));
    let mut p = UiPrefs::default();
    p.legend.x = 1.0;
    p.legend.max_height = 0.3;
    let text = p.to_toml();
    assert!(
        text.contains("[legend.transfer]\nx = 1.0\nmax_height = 0.3\n"),
        "{text}"
    );
    let back = UiPrefs::from_toml(&text).expect("parse");
    assert_eq!(back.legend.x, 1.0);
    assert_eq!(back.legend.max_height, 0.3);
    assert_eq!(back.legend.y, 0.0);
    assert!(UiPrefs::from_toml("[legend.transfer]\nx = 2.0\n").is_err());
    assert!(UiPrefs::from_toml("[legend.transfer]\nmax_width = 0.01\n").is_err());
    assert!(UiPrefs::from_toml("[legend.spectrum]\nx = 0.5\n").is_err());
    let hidden = UiPrefs::from_toml("[legend.transfer]\nhidden = true\n").expect("parse");
    assert!(hidden.legend.hidden);
}

#[test]
fn load_and_atomic_save() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ac2").join("ui.toml");
    assert_eq!(UiPrefs::load(Some(&path)), (UiPrefs::default(), None));
    let mut p = UiPrefs::default();
    p.outputs.insert("fake:loop".into(), vec![1]);
    p.save(&path).expect("save");
    assert_eq!(UiPrefs::load(Some(&path)), (p.clone(), None));
    // Only the file itself is left in the directory.
    let names: Vec<_> = std::fs::read_dir(path.parent().expect("dir"))
        .expect("list")
        .map(|e| e.expect("entry").file_name())
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");
    std::fs::write(&path, "garbage =").expect("write");
    let (q, err) = UiPrefs::load(Some(&path));
    assert_eq!(q, UiPrefs::default());
    assert!(err.is_some_and(|e| e.contains("ui.toml")));
}

/// A file from before the panes-only step: `fullscreen = true` alone is the window full
/// screen, top bar and lists shown.
#[test]
fn fullscreen_alone_is_the_window_full_screen() {
    let p = UiPrefs::from_toml("[layout]\nfullscreen = true\n").expect("parses");
    assert!(p.layout.fullscreen && !p.layout.panes_only);
}
