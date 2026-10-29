use super::*;

/// The rig the tests remember devices for.
const RIG: RigId = RigId(0xa);

/// A device is remembered per rig: two rigs' `jack:system` keep their own outputs and
/// session choices, and a key that names no rig is dropped from the file.
#[test]
fn devices_are_remembered_per_rig() {
    let other = RigId(0xb);
    let mut p = UiPrefs::default();
    p.outputs.insert((RIG, "jack:system".into()), vec![0]);
    p.outputs.insert((other, "jack:system".into()), vec![3]);
    let text = p.to_toml();
    assert!(
        text.contains("\"000000000000000a/jack:system\" = [1]"),
        "{text}"
    );
    let back = UiPrefs::from_toml(&text).expect("parses");
    assert_eq!(back.outputs_for(RIG, "jack:system"), Some(&[0u16][..]));
    assert_eq!(back.outputs_for(other, "jack:system"), Some(&[3u16][..]));
    let old = UiPrefs::from_toml(
        "[stimulus_outputs]\n\"jack:system\" = [1]\n[sessions.\"jack/jack:system\"]\ninputs = [1]\n",
    )
    .expect("parses");
    assert!(old.outputs.is_empty() && old.sessions.is_empty(), "{old:?}");
    assert!(!old.to_toml().contains("jack:system"));
}

#[test]
fn round_trip_is_one_based_in_the_file() {
    let mut p = UiPrefs::default();
    p.outputs.insert((RIG, "hw:UMC1820".into()), vec![0, 1]);
    p.outputs.insert((RIG, "fake:loop".into()), vec![3]);
    p.sessions.insert(
        (RIG, "coreaudio/hw:UMC1820".into()),
        DeviceRoles {
            inputs: vec![0, 1, 2],
            outputs: 2,
            spare_inputs: 2,
            reference: Some(0),
            mics: vec![1, 2],
            stimulus: vec![0],
            mic_names: [(1, "M30 FOH".to_owned()), (2, "ECM8000".to_owned())].into(),
            kinds: [
                (0, KindPref::Line),
                (1, KindPref::Mic),
                (2, KindPref::Mic),
                (3, KindPref::Digital),
                (4, KindPref::Digital),
            ]
            .into(),
            pairs: vec![ac2_proto::model::Pair {
                left: 3,
                right: 4,
                name: Some("AES".into()),
            }],
            output_device: Some("hw:Out".into()),
        },
    );
    let text = p.to_toml();
    assert!(
        text.contains("\"000000000000000a/hw:UMC1820\" = [1, 2]"),
        "{text}"
    );
    assert!(text.contains("reference = 1"), "{text}");
    assert!(text.contains("spare_inputs = 2"), "{text}");
    assert!(text.contains("mics = [2, 3]"), "{text}");
    assert!(text.contains("2 = \"M30 FOH\""), "{text}");
    assert!(text.contains("4 = \"digital\""), "{text}");
    assert!(text.contains("left = 4"), "{text}");
    assert!(text.contains("right = 5"), "{text}");
    assert!(text.contains("name = \"AES\""), "{text}");
    assert_eq!(UiPrefs::from_toml(&text), Ok(p));
    assert_eq!(UiPrefs::from_toml(""), Ok(UiPrefs::default()));
}

/// A file from before kinds names its mics in `mics` alone: they stay mics (an unnamed mic
/// keeps its SPL meters), every other input is a line input.
#[test]
fn roles_without_kinds_keep_their_mics() {
    let p = UiPrefs::from_toml(
        "[sessions.\"000000000000000a/fake/x\"]\ninputs = [1, 2, 3]\nreference = 1\nmics = [2]\n",
    )
    .expect("parses");
    let r = &p.sessions[&(RIG, "fake/x".into())];
    assert_eq!(r.kind_of(0), KindPref::Line);
    assert_eq!(r.kind_of(1), KindPref::Mic);
    assert_eq!(r.kind_of(2), KindPref::Line);
    assert!(r.pairs.is_empty());
    assert!(
        UiPrefs::from_toml("[sessions.\"000000000000000a/fake/x\"]\nkinds = { \"1\" = \"aes\" }\n")
            .is_err()
    );
    assert!(
        UiPrefs::from_toml(
            "[sessions.\"000000000000000a/fake/x\"]\npairs = [{ left = 0, right = 1 }]\n"
        )
        .is_err()
    );
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
fn range_band_round_trip() {
    let mut p = UiPrefs::default();
    assert!(p.range_band);
    assert!(!p.to_toml().contains("range_band"));
    p.range_band = false;
    let text = p.to_toml();
    assert!(text.contains("range_band = false"), "{text}");
    assert_eq!(UiPrefs::from_toml(&text), Ok(p));
}

#[test]
fn plate_background_round_trip() {
    use ac2_scene::plate::PlateBackground;
    let mut p = UiPrefs::default();
    assert_eq!(p.plate_background, PlateBackground::SeeThrough);
    assert!(!p.to_toml().contains("plate_background"));
    p.plate_background = PlateBackground::Solid;
    let text = p.to_toml();
    assert!(text.contains("plate_background = \"solid\""), "{text}");
    assert_eq!(UiPrefs::from_toml(&text), Ok(p));
    let see = UiPrefs::from_toml("plate_background = \"see_through\"\n").expect("parse");
    assert_eq!(see.plate_background, PlateBackground::SeeThrough);
    assert!(UiPrefs::from_toml("plate_background = \"glass\"\n").is_err());
}

/// A file from before the spectrograph settings and compare moved to the measurements
/// still loads: those keys are dropped with a warning, the rest (the pane tree too) kept.
#[test]
fn settings_now_the_measurements_are_dropped_and_the_rest_kept() {
    let text = "key_hints = false\nspectrograph_span_s = 60\nspectrograph_colours = \"magma\"\n\n\
                [layout]\nfocus = 2\nhidden = [\"Sub\"]\ncompared = [\"Main\"]\n";
    let p = UiPrefs::from_toml(text).expect("loads");
    assert!(!p.key_hints);
    assert_eq!(p.layout.hidden, ["Sub".to_owned()].into());
    let out = p.to_toml();
    for gone in ["spectrograph_span_s", "spectrograph_colours", "compared"] {
        assert!(!out.contains(gone), "{out}");
    }
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
fn jump_prominence_round_trip() {
    let mut p = UiPrefs::default();
    assert_eq!(p.jump_prominence_db, 3.0);
    assert!(!p.to_toml().contains("jump_prominence_db"));
    p.jump_prominence_db = 6.0;
    let text = p.to_toml();
    assert!(text.contains("jump_prominence_db = 6.0"), "{text}");
    assert_eq!(UiPrefs::from_toml(&text), Ok(p));
    // Nothing that cannot stand out reads back: the default instead.
    let p = UiPrefs::from_toml("jump_prominence_db = -1.0\n").expect("prefs");
    assert_eq!(p.jump_prominence_db, 3.0);
}

#[test]
fn shared_scale_round_trip() {
    let mut p = UiPrefs::default();
    assert!(p.shared_scale);
    assert!(!p.to_toml().contains("shared_scale"));
    p.shared_scale = false;
    let text = p.to_toml();
    assert!(text.contains("shared_scale = false"), "{text}");
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
    p.outputs.insert((RIG, "fake:loop".into()), vec![0]);
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
                meters: None,
                rig: None,
                modes: PaneModes {
                    transfer: TransferView::Ir(IrMode::Log),
                    ..PaneModes::default()
                },
                levels: LayoutLevels::default(),
                plates: ac2_scene::plate::Plates {
                    readout: ac2_scene::plate::PlatePlace {
                        at: Some([0.0, 1.0]),
                        scale: 2.0,
                    },
                    ..ac2_scene::plate::Plates::default()
                },
            },
            PanePrefs {
                id: PaneId(2),
                kind: PaneKind::Spl,
                measurement: Some("FOH SPL".to_owned()),
                meters: None,
                // Pinned to another rig: kept by its id, the name for when it is away.
                rig: Some((ac2_proto::units::RigId(0x7075_7075), "pupu".to_owned())),
                modes: PaneModes {
                    transfer: TransferView::Coherence,
                    spl: SplMode::Leq,
                    spectrum: SpectrumMode::Spectrograph,
                    sweep: SweepMode::Ir(IrMode::Etc),
                    programme: ac2_scene::view::ProgrammeMode::Goniometer,
                    sweep_reference: false,
                    chrome: PlotChrome::Bare,
                },
                levels: LayoutLevels::default(),
                plates: ac2_scene::plate::Plates::default(),
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
        "[layout.tree]",
        "split = \"row\"",
        "[[layout.panes]]",
        "kind = \"spl\"",
        "measurement = \"FOH SPL\"",
        "[layout.panes.rig]",
        "id = \"0000000070757075\"",
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
        "alignment",
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
    assert!(UiPrefs::from_toml("[stimulus_outputs]\n\"000000000000000a/x\" = [0]\n").is_err());
    assert!(UiPrefs::from_toml("[stimulus_outputs]\n\"000000000000000a/x\" = []\n").is_err());
    assert!(UiPrefs::from_toml("[sessions.\"000000000000000a/fake/x\"]\nmics = [0]\n").is_err());
    assert!(
        UiPrefs::from_toml("[sessions.\"000000000000000a/fake/x\"]\nmic_names = { a = \"M\" }\n")
            .is_err()
    );
    assert!(UiPrefs::from_toml("[sessions.\"000000000000000a/fake/x\"]\ncolour = 1\n").is_err());
    assert!(UiPrefs::from_toml("[ir_levels]\ntransfer = [10.0, -10.0]\n").is_err());
    assert!(UiPrefs::from_toml("[ir_levels]\nsweep = [-10.0]\n").is_err());
    assert!(UiPrefs::from_toml("[ir_levels]\nphase = [-10.0, 10.0]\n").is_err());
}

/// The impulse-response dB axes moved off their defaults and each pane's level axes the
/// operator set are written, the rest left out; an out-of-limits range is brought within the
/// axis limits on reading.
#[test]
fn level_ranges_round_trip() {
    let d = UiPrefs::default();
    assert!(!d.to_toml().contains("levels"));
    let mut p = UiPrefs::default();
    p.ir_levels.sweep = Range::new(-90.0, 0.0);
    let mut panes = two_panes();
    panes.views[0].levels.transfer = Some(LevelRange {
        lo: -24.5,
        hi: 12.0,
    });
    panes.views[1].levels.spectrum_dbfs = Some(LevelRange {
        lo: -140.0,
        hi: -40.0,
    });
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
    assert_eq!(
        v.distortion,
        Some(LevelRange {
            lo: -300.0,
            hi: -200.0
        })
    );
    // Low above high is no range: that one is left out (framed on first show), the layout
    // stays.
    assert_eq!(v.transfer, None);
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

/// A hidden legend is written, the default left out; a `[legend]` this version cannot
/// read (one from before the plates were each pane's) is dropped, the rest of the file kept.
#[test]
fn legend_round_trip() {
    assert!(!UiPrefs::default().to_toml().contains("legend"));
    let mut p = UiPrefs::default();
    p.legend.hidden = true;
    let text = p.to_toml();
    assert!(
        text.contains("[legend.transfer]\nhidden = true\n"),
        "{text}"
    );
    assert!(UiPrefs::from_toml(&text).expect("parse").legend.hidden);
    let old =
        UiPrefs::from_toml("theme = \"light\"\n\n[legend.transfer]\nhidden = true\nx = 1.0\n")
            .expect("parse");
    assert!(!old.legend.hidden);
    assert_eq!(old.theme, Some(ac2_scene::theme::ThemeName::Light));
}

/// A pane's plates moved or scaled are written under the pane, the ones left alone not;
/// a place or scale out of bounds is left out.
#[test]
fn plates_round_trip() {
    let mut p = UiPrefs::default();
    p.layout.panes = Some(two_panes());
    let text = p.to_toml();
    assert!(
        text.contains("[layout.panes.plates.readout]\nat = [0.0, 1.0]\nscale = 2.0\n"),
        "{text}"
    );
    assert_eq!(text.matches("plates").count(), 1, "{text}");
    let back = UiPrefs::from_toml(&text).expect("parse");
    assert_eq!(back.layout.panes, p.layout.panes);
    let bad = text.replace("at = [0.0, 1.0]", "at = [0.0, 1.5]");
    let back = UiPrefs::from_toml(&bad).expect("parse");
    let readout = back.layout.panes.expect("panes").views[0].plates.readout;
    assert_eq!(readout.at, None);
    assert_eq!(readout.scale, 2.0);
}

/// The peak list's plate is written under the pane as the others are.
#[test]
fn the_peak_plate_round_trips() {
    let mut p = UiPrefs::default();
    let mut panes = two_panes();
    panes.views[0].plates.peaks.move_to(1.0, 0.25);
    panes.views[0].plates.peaks.set_scale(1.5);
    p.layout.panes = Some(panes);
    let text = p.to_toml();
    assert!(
        text.contains("[layout.panes.plates.peaks]\nat = [1.0, 0.25]\nscale = 1.5\n"),
        "{text}"
    );
    let back = UiPrefs::from_toml(&text).expect("parse");
    assert_eq!(back.layout.panes, p.layout.panes);
}

/// A Programme pane's loudness plate is written under the pane as the others are, and
/// left out where the pane docks it.
#[test]
fn the_loudness_plate_round_trips() {
    let mut p = UiPrefs::default();
    let mut panes = two_panes();
    panes.views[1].plates.loudness.move_to(0.0, 0.5);
    panes.views[1].plates.loudness.set_scale(2.5);
    p.layout.panes = Some(panes);
    let text = p.to_toml();
    assert!(
        text.contains("[layout.panes.plates.loudness]\nat = [0.0, 0.5]\nscale = 2.5\n"),
        "{text}"
    );
    let back = UiPrefs::from_toml(&text).expect("parse");
    assert_eq!(back.layout.panes, p.layout.panes);
    let bad = text.replace("scale = 2.5", "scale = 4.0");
    let back = UiPrefs::from_toml(&bad).expect("parse");
    let l = back.layout.panes.expect("panes").views[1].plates.loudness;
    assert_eq!((l.at, l.scale()), (Some([0.0, 0.5]), 1.0));
}

#[test]
fn load_and_atomic_save() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ac2").join("ui.toml");
    assert_eq!(UiPrefs::load(Some(&path)), (UiPrefs::default(), None));
    let mut p = UiPrefs::default();
    p.outputs.insert((RIG, "fake:loop".into()), vec![1]);
    p.save(&path).expect("save");
    assert_eq!(UiPrefs::load(Some(&path)), (p.clone(), None));
    // Only the file itself is left in the directory.
    let names: Vec<_> = std::fs::read_dir(path.parent().expect("dir"))
        .expect("list")
        .map(|e| e.expect("entry").file_name())
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");
    // Garbage, a zero-filled tail after a power cut, bytes that are not text: defaults,
    // the reason said once, the file kept aside for inspection.
    for bad in [&b"garbage ="[..], &[0u8; 64][..], &[0xff, 0xfe, b'a'][..]] {
        std::fs::write(&path, bad).expect("write");
        let (q, err) = UiPrefs::load(Some(&path));
        assert_eq!(q, UiPrefs::default());
        assert!(err.is_some_and(|e| e.contains("ui.toml") && e.contains("set aside")));
        assert!(!path.exists());
    }
    let aside = std::fs::read_dir(path.parent().expect("dir"))
        .expect("list")
        .filter(|e| {
            e.as_ref()
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with("ui.toml.damaged-")
        })
        .count();
    assert_eq!(aside, 3);
    // Zero length is an empty file: the defaults, nothing to say.
    std::fs::write(&path, b"").expect("write");
    assert_eq!(UiPrefs::load(Some(&path)), (UiPrefs::default(), None));
}

/// A file from before the panes-only step: `fullscreen = true` alone is the window full
/// screen, top bar and lists shown.
#[test]
fn fullscreen_alone_is_the_window_full_screen() {
    let p = UiPrefs::from_toml("[layout]\nfullscreen = true\n").expect("parses");
    assert!(p.layout.fullscreen && !p.layout.panes_only);
}

#[test]
fn advice_round_trip() {
    let mut p = UiPrefs::default();
    assert_eq!(p.advice, ac2_scene::advice::AdviceSettings::default());
    // The defaults are not written.
    assert!(!p.to_toml().contains("advice"));
    p.advice.low_level_dbfs = -60.0;
    p.advice.on = false;
    let text = p.to_toml();
    assert!(text.contains("[advice]"), "{text}");
    assert!(text.contains("low_level_dbfs = -60.0"), "{text}");
    assert!(!text.contains("low_coherence"), "{text}");
    assert_eq!(UiPrefs::from_toml(&text), Ok(p));
    assert!(UiPrefs::from_toml("[advice]\nlow_coherence = 1.5\n").is_err());
    assert!(UiPrefs::from_toml("[advice]\nref_headroom_db = 0.0\n").is_err());
}

/// The ember decay is written only when changed, read back, and a hand-edited value outside
/// its range is clamped into it.
#[test]
fn ember_decay_round_trips_and_clamps() {
    use ac2_scene::gonio::{EMBER_DECAY_DEFAULT_S, EMBER_DECAY_RANGE_S};
    assert!(!UiPrefs::default().to_toml().contains("ember_decay_s"));
    let p = UiPrefs {
        ember_decay_s: 1.2,
        ..UiPrefs::default()
    };
    let text = p.to_toml();
    assert!(text.contains("ember_decay_s = 1.2"), "{text}");
    assert_eq!(
        UiPrefs::from_toml(&text).expect("parses").ember_decay_s,
        1.2
    );
    let read = |t: &str| UiPrefs::from_toml(t).expect("parses").ember_decay_s;
    assert_eq!(read("ember_decay_s = 30.0\n"), EMBER_DECAY_RANGE_S.1);
    assert_eq!(read("ember_decay_s = 0.0\n"), EMBER_DECAY_RANGE_S.0);
    assert_eq!(read(""), EMBER_DECAY_DEFAULT_S);
}

/// The ember glow is written only when changed, read back, and a hand-edited value outside
/// its range is clamped into it.
#[test]
fn ember_glow_round_trips_and_clamps() {
    use ac2_scene::gonio::{EMBER_GLOW_DEFAULT, EMBER_GLOW_RANGE};
    assert!(!UiPrefs::default().to_toml().contains("ember_glow"));
    let p = UiPrefs {
        ember_glow: 3.0,
        ..UiPrefs::default()
    };
    let text = p.to_toml();
    assert!(text.contains("ember_glow = 3.0"), "{text}");
    assert_eq!(UiPrefs::from_toml(&text).expect("parses").ember_glow, 3.0);
    let read = |t: &str| UiPrefs::from_toml(t).expect("parses").ember_glow;
    assert_eq!(read("ember_glow = 100.0\n"), EMBER_GLOW_RANGE.1);
    assert_eq!(read("ember_glow = 0.0\n"), EMBER_GLOW_RANGE.0);
    assert_eq!(read("ember_glow = -2.0\n"), EMBER_GLOW_RANGE.0);
    assert_eq!(read(""), EMBER_GLOW_DEFAULT);
}

/// A Programme pane keeps what it meters, one-based in the file; inputs no pane can meter
/// (input 0) bring the pane back empty rather than refusing the file.
#[test]
fn programme_pane_meters_round_trip() {
    use crate::state::Metered;
    let mut panes = two_panes();
    panes.views[1].kind = PaneKind::Programme;
    panes.views[1].meters = Some(Metered::Pair { left: 2, right: 3 });
    let p = UiPrefs {
        layout: LayoutPrefs {
            panes: Some(panes.clone()),
            ..LayoutPrefs::default()
        },
        ..UiPrefs::default()
    };
    let text = p.to_toml();
    assert!(text.contains("pair = [3, 4]"), "{text}");
    let back = UiPrefs::from_toml(&text).expect("parses");
    assert_eq!(back.layout.panes, Some(panes));
    let bad = text.replace("pair = [3, 4]", "pair = [0, 4]");
    let back = UiPrefs::from_toml(&bad).expect("parses");
    assert_eq!(back.layout.panes.expect("panes").views[1].meters, None);
}

/// A Programme pane's view round-trips; a stage-4 pane, saved before the views, has no key
/// and reads back as one Meters pane (prefs carry no version to rewrite it by).
#[test]
fn the_programme_view_round_trips_and_defaults_to_meters() {
    use ac2_scene::view::ProgrammeMode;
    let mut p = UiPrefs::default();
    let mut panes = two_panes();
    panes.views[1].kind = PaneKind::Programme;
    panes.views[1].modes.programme = ProgrammeMode::Goniometer;
    p.layout.panes = Some(panes);
    let text = p.to_toml();
    assert!(text.contains("programme_view = \"goniometer\""), "{text}");
    assert_eq!(UiPrefs::from_toml(&text).expect("parse"), p);
    let old = text.replace("programme_view = \"goniometer\"\n", "");
    assert!(!old.contains("programme_view = \"goniometer\""));
    let back = UiPrefs::from_toml(&old).expect("parse");
    let v = &back.layout.panes.expect("panes").views[1];
    assert_eq!(
        (v.kind, v.modes.programme),
        (PaneKind::Programme, ProgrammeMode::Meters)
    );
}

/// The rigs held are remembered with their ids, names and where each was reached; a rig
/// whose id is not one a daemon has is dropped.
#[test]
fn rigs_round_trip() {
    let p = UiPrefs {
        rigs: vec![
            RememberedRig {
                id: RigId(0x9e9e),
                name: "pupu".into(),
                addr: RigAddr::Network {
                    host: "pupu.local".into(),
                    port: 47820,
                    fingerprint: Some("1a2b-3c4d".into()),
                },
            },
            RememberedRig {
                id: RigId(0x7e57),
                name: "laptop".into(),
                addr: RigAddr::Embedded("jack".into()),
            },
            RememberedRig {
                id: RigId(0x1),
                name: "local".into(),
                addr: RigAddr::Local,
            },
            RememberedRig {
                id: RigId(0x2),
                name: "lab".into(),
                addr: RigAddr::Endpoints {
                    ctrl: "tcp://lab:6000".into(),
                    data: "tcp://lab:6001".into(),
                },
            },
        ],
        ..UiPrefs::default()
    };
    let text = p.to_toml();
    assert!(text.contains("[[rigs]]"), "{text}");
    assert!(text.contains("id = \"0000000000009e9e\""), "{text}");
    assert_eq!(UiPrefs::from_toml(&text).expect("parses"), p);
    let bad = "[[rigs]]\nid = \"0000000000000000\"\nname = \"x\"\nreach = \"local\"\n";
    assert!(UiPrefs::from_toml(bad).expect("parses").rigs.is_empty());
}
