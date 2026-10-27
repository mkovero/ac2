//! Overlays, dialogs, the palette, themes, the stimulus flow and startup.

use super::*;

#[test]
fn help_overlay() {
    if !have_gpu("help_overlay") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press(Key::H);
    step_until(&mut h, "help", |a| a.state.overlay == Overlay::Help);
    snapshot(&mut h, "help_overlay");
    // H closes it again.
    h.key_press(Key::H);
    step_until(&mut h, "help closed", |a| a.state.overlay == Overlay::None);
}

/// Drawn by a CPU rasteriser, the window says so once in the corner and paces its frames.
#[test]
fn software_rendering_is_said() {
    if !have_gpu("software_rendering_is_said") {
        return;
    }
    let mut opts = options(None);
    opts.rasterizer = Some(ac2_plot::Rasterizer::Software);
    let mut h = harness(opts);
    h.step();
    h.step();
    let toasts: Vec<_> = h
        .state()
        .state
        .toasts
        .list
        .iter()
        .map(|t| t.text.clone())
        .collect();
    assert_eq!(toasts, [ac2_scene::toast::SOFTWARE_RENDERING]);
    h.get_by_label(ac2_scene::toast::SOFTWARE_RENDERING);
}

/// Notifications in the corner: an information, a long warning that wraps at the box's
/// maximum width, an error naming a long path; stacked over the focused pane's key hints,
/// each as wide as its text. A click dismisses one; the log keeps it.
#[test]
fn toasts_wrap_and_stack() {
    if !have_gpu("toasts_wrap_and_stack") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.state_mut().state.toasts.list.clear();
    let rig = h.state().state.active();
    let reply = |what: &str, result: Result<(), String>| {
        ac2_ui::state::Msg::Conn(
            rig,
            Box::new(ac2_ui::conn::ConnEvent::Reply {
                what: what.into(),
                result,
            }),
        )
    };
    h.state_mut()
        .dispatch(reply("slot 1: Main L S1 captured", Ok(())));
    // M with fewer than two shown stored traces is refused, saying what to do.
    h.key_press(Key::M);
    step_until(&mut h, "the warning", |a| {
        a.state
            .toasts
            .list
            .iter()
            .any(|t| t.severity == Severity::Warning)
    });
    h.state_mut().dispatch(reply(
        "export /home/operator/.local/share/ac2/traces/festival-main-stage-left-hang-position-3-after-the-high-shelf.csv",
        Err("cannot write the file: permission denied (the folder belongs to another user; choose another folder or fix its permissions)".into()),
    ));
    let texts: Vec<String> = h
        .state()
        .state
        .toasts
        .list
        .iter()
        .map(|t| t.text.clone())
        .collect();
    assert_eq!(texts.len(), 3, "{texts:?}");
    // Pinned up: they expire on the wall clock. A few passes so egui has measured the
    // areas of places in the stack it had not drawn before.
    for _ in 0..3 {
        for t in &mut h.state_mut().state.toasts.list {
            t.until_s = f64::from(u32::MAX);
        }
        h.step();
    }
    snapshot_when(
        &mut h,
        "toasts_stacked",
        |a| {
            for t in &mut a.state.toasts.list {
                t.until_s = f64::from(u32::MAX);
            }
        },
        |a| a.state.toasts.list.len() == 3,
    );
    // A click on the newest dismisses it; the log still has all three.
    let newest = texts[2].clone();
    h.get_by_label(&newest).click();
    step_until(&mut h, "dismissed", |a| a.state.toasts.list.len() == 2);
    assert_eq!(h.state().state.notices.len(), 3);
}

/// The keys belong to the reducer, so egui never holds keyboard focus: Tab and the arrows in
/// an open dialog (or with none open) leave no widget focused — the sidebar's measurement
/// chip in particular — and the dialog stays open on its own focus.
#[test]
fn keyboard_focus_stays_in_the_open_dialog() {
    if !have_gpu("keyboard_focus_stays_in_the_open_dialog") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    let wander = |h: &mut Harness<'_, App>, what: &str| {
        for key in [
            Key::Tab,
            Key::ArrowDown,
            Key::ArrowRight,
            Key::ArrowLeft,
            Key::ArrowUp,
            Key::Tab,
            Key::ArrowDown,
        ] {
            h.key_press(key);
            h.step();
            h.step();
            assert_eq!(
                h.ctx.memory(|m| m.focused()),
                None,
                "{what}: egui focus after {key:?}"
            );
        }
    };
    wander(&mut h, "no dialog");
    type Open = (
        &'static str,
        fn(&mut Harness<'_, App>),
        fn(&Overlay) -> bool,
    );
    let dialogs: [Open; 4] = [
        (
            "Leq windows",
            |h| h.key_press_modifiers(Modifiers::SHIFT, Key::L),
            |o| o.leq().is_some(),
        ),
        (
            "palette",
            |h| h.key_press_modifiers(Modifiers::COMMAND, Key::K),
            |o| matches!(o, Overlay::Palette(_)),
        ),
        (
            "level prompt",
            |h| h.key_press(Key::Space),
            |o| matches!(o, Overlay::Prompt(_)),
        ),
        ("help", |h| h.key_press(Key::H), |o| *o == Overlay::Help),
    ];
    for (what, open, is_open) in dialogs {
        open(&mut h);
        step_until(&mut h, what, |a| is_open(&a.state.overlay));
        wander(&mut h, what);
        assert!(is_open(&h.state().state.overlay), "{what} stays open");
        h.key_press(Key::Escape);
        step_until(&mut h, "closed", |a| a.state.overlay == Overlay::None);
    }
}

#[test]
fn command_palette() {
    if !have_gpu("command_palette") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    step_until(&mut h, "palette", |a| {
        matches!(a.state.overlay, Overlay::Palette(_))
    });
    h.event(Event::Text("delay".into()));
    step_until(
        &mut h,
        "query",
        |a| matches!(&a.state.overlay, Overlay::Palette(p) if p.query == "delay"),
    );
    snapshot(&mut h, "command_palette");
    // Enter runs the highlighted command against the fake daemon.
    h.key_press(Key::Enter);
    step_until(&mut h, "palette closed", |a| {
        a.state.overlay == Overlay::None
    });
}

/// The calibrations view over a rig whose mic has two curves (0° chosen on its input), a
/// sensitivity calibration taken 3 h ago and the 90° curve also stored: what each input uses,
/// the library and the calibrations, with the sidebar naming the curve in use.
#[test]
fn calibrations_view() {
    use ac2_proto::model::{
        CalEntry, CalKey, CurveChoice, DeviceId, InputKind, InputSetup, InputSetups, Mic,
        MicCurveRef, SplCal,
    };
    use ac2_proto::units::{Db, DbSpl, Dbfs, Hz, WallNs};
    use ac2_proto::{Change, Patch};
    if !have_gpu("settings_calibration") {
        return;
    }
    let rig = common::Rig::start();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let curve = |label: &str, file: &str| MicCurveRef {
        label: label.into(),
        file_name: file.into(),
        content_hash: "0123456789abcdef".into(),
        points: 100,
        f_lo: Hz(50.0),
        f_hi: Hz(20_000.0),
        imported_at: WallNs(now),
        stated_sensitivity: Some(15.0),
    };
    {
        let mut f = rig.fake.lock();
        f.commit(Change::Mic(Patch::Set(Mic {
            name: "MM1 34804".into(),
            curves: vec![
                curve("0°", "449350_34804_0Grad.txt"),
                curve("90°", "449350_34804_90Grad.txt"),
            ],
        })));
        f.commit(Change::Calibration(Patch::Set(CalEntry {
            key: CalKey {
                device: DeviceId("fake:loop".into()),
                channel: 1,
                mic: "MM1 34804".into(),
            },
            spl: SplCal {
                sensitivity: Db(130.5),
                method: ac2_proto::model::CalMethod::Acoustic {
                    calibrator_level: DbSpl(94.0),
                },
                freq: Hz(1000.0),
                measured: Dbfs(-36.5),
                calibrated_at: WallNs(now - 3 * 3_600_000_000_000 - 60_000_000_000),
            },
        })));
        f.commit(Change::Inputs(InputSetups {
            inputs: vec![InputSetup {
                channel: 1,
                kind: InputKind::Mic {
                    mic: Some("MM1 34804".into()),
                    curve: CurveChoice::Curve {
                        label: "0°".into()
                    },
                },
                pair: None,
            }],
            pairs: Vec::new(),
        }));
    }
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", |a| {
        live(a) && a.state.daemon().is_some_and(|s| !s.mics.is_empty())
    });
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    step_until(&mut h, "palette", |a| {
        matches!(a.state.overlay, Overlay::Palette(_))
    });
    h.event(Event::Text("calibrations".into()));
    h.key_press(Key::Enter);
    step_until(&mut h, "calibrations view", |a| {
        a.state.overlay.cal().is_some()
    });
    h.state_mut().state.toasts.list.clear();
    snapshot(&mut h, "settings_calibration");
    // C on the mic's input: the acoustic calibration dialog, the mic prefilled.
    for _ in 0..8 {
        let on_input = match h.state().state.overlay.cal() {
            Some(v) => h
                .state()
                .state
                .daemon()
                .and_then(|s| v.focused(s))
                .is_some_and(|l| l == ac2_ui::cal_view::CalLine::Input(1)),
            None => false,
        };
        if on_input {
            break;
        }
        h.key_press(Key::ArrowDown);
    }
    h.key_press(Key::C);
    h.event(Event::Text("c".into()));
    step_until(&mut h, "the acoustic dialog", |a| {
        a.state.overlay.cal().is_some_and(|v| v.acoustic.is_some())
    });
    h.state_mut().state.toasts.list.clear();
    snapshot(&mut h, "acoustic_calibration_dialog");
}

/// X on an ambiguous finding: the candidate list over the transfer pane (decision 1c), the
/// banner saying tracking waits for the pick, and a key inserting a candidate.
#[test]
fn ambiguous_delay_candidate_list() {
    if !have_gpu("ambiguous_delay_candidate_list") {
        return;
    }
    let rig = common::Rig::start();
    rig.fake.lock().finding = ac2_client::fake::FakeFinding::Ambiguous;
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press_modifiers(Modifiers::ALT, Key::Num1);
    h.key_press(Key::X);
    step_until(&mut h, "candidate list", |a| {
        matches!(a.state.overlay, Overlay::DelayPick(_))
            && a.state
                .meas(MeasId(1))
                .is_some_and(|m| m.delay.as_ref().is_some_and(|d| d.awaiting_pick))
    });
    assert_eq!(
        rig.fake.lock().last_find,
        Some((ac2_proto::model::FinderBand::Auto, None))
    );
    // Toasts expire on the wall clock; the snapshot shows the list and the plots.
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "delay_pick_candidates");
    // 2 inserts the second candidate (12.7 ms) and resolves the finding.
    h.key_press(Key::Num2);
    step_until(&mut h, "inserted", |a| {
        a.state.overlay == Overlay::None
            && a.state.meas(MeasId(1)).is_some_and(|m| {
                m.delay
                    .as_ref()
                    .is_some_and(|d| !d.awaiting_pick && (d.applied.0 - 0.0127).abs() < 1e-9)
            })
    });
}

#[test]
fn themes() {
    if !have_gpu("themes") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    palette(&mut h, "theme");
    step_until(&mut h, "light", |a| a.state.theme == ThemeName::Light);
    snapshot(&mut h, "theme_light");
    palette(&mut h, "theme");
    step_until(&mut h, "high contrast", |a| {
        a.state.theme == ThemeName::HighContrast
    });
    snapshot(&mut h, "theme_high_contrast");
}

#[test]
fn stimulus_flow_against_the_fake_daemon() {
    if !have_gpu("stimulus_flow_against_the_fake_daemon") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    // Space without a level: the prompt opens; nothing reaches the daemon.
    h.key_press(Key::Space);
    step_until(&mut h, "level prompt", |a| {
        matches!(a.state.overlay, Overlay::Prompt(_))
    });
    assert_eq!(rig.fake.executions("gen.acquire"), 0);
    h.event(Event::Text("-24".into()));
    h.key_press(Key::Enter);
    step_until(&mut h, "level", |a| a.state.rig.stimulus.level.is_some());
    h.key_press(Key::Space);
    step_until(&mut h, "armed", |a| {
        a.state.daemon().is_some_and(|s| s.generator.armed)
    });
    assert!(!h.state().state.daemon().is_some_and(|s| s.generator.firing));
    h.key_press(Key::Enter);
    step_until(&mut h, "firing", |a| {
        a.state.daemon().is_some_and(|s| s.generator.firing)
    });
    h.key_press(Key::ArrowDown);
    step_until(&mut h, "level −25", |a| {
        a.state.daemon().is_some_and(|s| {
            s.generator
                .settings
                .as_ref()
                .is_some_and(|g| g.level.0 == -25.0)
        })
    });
    h.key_press(Key::Escape);
    step_until(&mut h, "stopped", |a| {
        a.state.daemon().is_some_and(|s| {
            !s.generator.firing && !s.generator.armed && s.generator.owner.is_none()
        })
    });
    assert!(rig.fake.executions("gen.stop") >= 1);
}

/// Full screen is the pane alone: with the stimulus armed and then playing, the stage view
/// draws exactly what it drew with the stimulus off (one snapshot, taken three times); Esc
/// still stops it from there.
#[test]
fn the_stage_view_stays_the_pane_while_firing() {
    if !have_gpu("the_stage_view_stays_the_pane_while_firing") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.state_mut().state.rig.stimulus.level = Some(ac2_proto::units::Dbfs(-24.0));
    h.key_press(Key::W);
    h.key_press(Key::W);
    step_until(&mut h, "the stage view", |a| {
        a.state.stage_view() && a.state.layout.focus_kind() == PaneKind::Transfer
    });
    let quiet = |h: &mut Harness<'_, App>| {
        h.state_mut().state.toasts.list.clear();
        h.step();
    };
    quiet(&mut h);
    snapshot(&mut h, "transfer_stage");
    h.key_press(Key::Space);
    step_until(&mut h, "armed", |a| {
        a.state.daemon().is_some_and(|s| s.generator.armed)
    });
    quiet(&mut h);
    snapshot(&mut h, "transfer_stage");
    h.key_press(Key::Enter);
    step_until(&mut h, "firing", |a| {
        a.state.daemon().is_some_and(|s| s.generator.firing) && a.state.stage_view()
    });
    quiet(&mut h);
    snapshot(&mut h, "transfer_stage");
    h.key_press(Key::Escape);
    step_until(&mut h, "stopped", |a| {
        a.state.daemon().is_some_and(|s| {
            !s.generator.firing && !s.generator.armed && s.generator.owner.is_none()
        }) && a.state.stage_view()
    });
}

/// F twice: the panes alone, every pane of the layout, full screen (no top bar, lists, pane
/// titles or key hints; each plot with its one-line caption).
#[test]
fn panes_only_full_screen() {
    if !have_gpu("panes_only_full_screen") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press(Key::F);
    h.key_press(Key::F);
    step_until(&mut h, "panes only", |a| {
        a.state.stage_view() && !a.state.layout.maximized && a.state.layout.panes().len() > 1
    });
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "panes_only");
}

#[test]
fn startup_first_frame() {
    if !have_gpu("startup_first_frame") {
        return;
    }
    // The bound is on the app's own startup: from `App::new` (fonts, state, the link's
    // thread) through its first UI pass, which never waits for the daemon. GPU device
    // creation and the first render are timed and printed but not bounded: on a software
    // rasteriser they are dominated by shader compilation, whose wall time scales with
    // whatever else the machine is running (3–7 s at load 30 on 12 cores).
    let t0 = Instant::now();
    let mut h = Harness::builder()
        .with_size(SIZE)
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(move |cc| {
            let mut opts = options(None);
            opts.started = Instant::now();
            App::new(cc, opts)
        });
    h.step();
    let app = h.state().startup.first_ui.expect("first UI pass");
    let t_render = Instant::now();
    let img = h.render().expect("render");
    let render = t_render.elapsed();
    let total = t0.elapsed();
    assert_eq!(img.width(), SIZE.x as u32);
    println!(
        "startup: app {:.1} ms to its first UI pass; first frame rendered headless in {:.1} ms \
         (render {:.1} ms; target < 300 ms on the real window, `--bench-startup`)",
        app.as_secs_f64() * 1e3,
        total.as_secs_f64() * 1e3,
        render.as_secs_f64() * 1e3,
    );
    // Tens of milliseconds unloaded; a blocking call on the startup path (a connect, a
    // device or file wait) takes the bound out at once.
    assert!(app < Duration::from_secs(1), "{app:?}");
}
