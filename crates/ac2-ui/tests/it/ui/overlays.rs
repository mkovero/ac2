//! Overlays, dialogs, the palette, themes, the stimulus flow and startup.

use super::*;

#[test]
fn help_overlay() {
    if !have_gpu("help_overlay") {
        return;
    }
    let _rig = Rig::start();
    let mut h = harness(options());
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
    let mut opts = options();
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
    let _rig = Rig::start();
    let mut h = harness(options());
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

#[test]
fn command_palette() {
    if !have_gpu("command_palette") {
        return;
    }
    let _rig = Rig::start();
    let mut h = harness(options());
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
    // Enter runs the highlighted command (its request goes nowhere: no link) and closes.
    h.key_press(Key::Enter);
    step_until(&mut h, "palette closed", |a| {
        a.state.overlay == Overlay::None
    });
}

/// The calibrations view over a rig whose mic has two curves (0° chosen on its input) and a
/// sensitivity calibration just taken with the 1 kHz calibrator tone: what each input uses,
/// the library and the calibrations, with the sidebar naming the curve in use.
#[test]
fn calibrations_view() {
    if !have_gpu("settings_calibration") {
        return;
    }
    let _rig = Rig::scene("calibrations");
    let mut h = harness(options());
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

#[test]
fn themes() {
    if !have_gpu("themes") {
        return;
    }
    let _rig = Rig::start();
    let mut h = harness(options());
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

/// F twice: the panes alone, every pane of the layout, full screen (no top bar, lists, pane
/// titles or key hints; each plot with its one-line caption).
#[test]
fn panes_only_full_screen() {
    if !have_gpu("panes_only_full_screen") {
        return;
    }
    let _rig = Rig::start();
    let mut h = harness(options());
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
            let mut opts = options();
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
