//! Keys and pointer against the window's overlays: focus stays with the reducer, the
//! stimulus keys drive the daemon's generator, a pane's hold ends under a window.

use super::*;
use crate::harness::R;

/// The keys belong to the reducer, so egui never holds keyboard focus: Tab and the arrows in
/// an open dialog (or with none open) leave no widget focused — the sidebar's measurement
/// chip in particular — and the dialog stays open on its own focus.
#[test]
fn keyboard_focus_stays_in_the_open_dialog() -> R {
    if !have_gpu("keyboard_focus_stays_in_the_open_dialog") {
        return Ok(());
    }
    let rig = Rig::start()?;
    let mut h = harness(options(&rig));
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
    Ok(())
}

/// The first Space asks what to play (Enter: pink), then for the level, and takes nothing;
/// typed, Space arms, Enter plays, ↓ steps the level a dB, Esc stops and releases: each
/// step as the daemon's generator says.
#[test]
fn stimulus_flow_against_the_daemon() -> R {
    if !have_gpu("stimulus_flow_against_the_daemon") {
        return Ok(());
    }
    let rig = Rig::start()?;
    let mut h = harness(options(&rig));
    step_until(&mut h, "live frames", live);
    h.key_press(Key::Space);
    step_until(
        &mut h,
        "signal prompt",
        |a| matches!(&a.state.overlay, Overlay::Prompt(p) if p.kind == ac2_ui::state::PromptKind::StimulusSignal),
    );
    h.key_press(Key::Enter);
    step_until(
        &mut h,
        "level prompt",
        |a| matches!(&a.state.overlay, Overlay::Prompt(p) if p.kind == ac2_ui::state::PromptKind::StimulusLevel),
    );
    // Nothing reached the daemon: the generator has no owner.
    let owner = rig
        .cli
        .state()
        .and_then(|s| s.generator.owner.clone())
        .map(|c| c.0);
    assert_eq!(owner, None);
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
    Ok(())
}

/// A button pressed on the spectrum pane and released while a window is over the panes (the
/// pane's own pointer check is not run then) still ends the hold on its level axis.
#[test]
fn a_release_under_a_window_ends_the_pane_hold() -> R {
    if !have_gpu("a_release_under_a_window_ends_the_pane_hold") {
        return Ok(());
    }
    let rig = Rig::start()?;
    let mut h = harness(options(&rig));
    step_until(&mut h, "live frames", live);
    let (p, rect) = h
        .state()
        .pane_rects
        .iter()
        .copied()
        .find(|(p, _)| h.state().state.layout.kind(*p) == PaneKind::Spectrum)
        .expect("a spectrum pane");
    let at = rect.center();
    let button = |pressed| Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    h.event(Event::PointerMoved(at));
    settle(&mut h, 2);
    h.event(button(true));
    settle(&mut h, 2);
    assert!(
        h.state().state.level_held(p),
        "held while the button is down"
    );
    h.key_press(Key::H);
    step_until(&mut h, "help", |a| a.state.overlay == Overlay::Help);
    h.event(button(false));
    settle(&mut h, 3);
    assert!(!h.state().state.level_held(p), "released under the window");
    Ok(())
}

/// What the stage view shows besides the curves: every widget of the window (role and text)
/// and the transfer plot's legend and banners, digits masked (live readouts move).
fn drawn_texts(h: &Harness<'_, App>) -> Vec<String> {
    use egui_kittest::kittest::{By, NodeT as _, Queryable as _};
    let mask = |l: &str| -> String {
        l.chars()
            .map(|c| if c.is_ascii_digit() { '#' } else { c })
            .collect()
    };
    let mut t: Vec<String> = h
        .query_all(By::new().predicate(|_| true))
        .map(|n| {
            let n = n.accesskit_node();
            // A label widget carries its text as the node's value; others as its label.
            let text = n.label().or_else(|| n.value()).unwrap_or_default();
            format!("widget {:?} {}", n.role(), mask(&text))
        })
        .collect();
    let s = &h.state().state;
    let tf = ac2_ui::scenes::transfer(
        s,
        crate::common::pane(s, PaneKind::Transfer),
        &ac2_scene::theme::Theme::dark(),
        ac2_scene::primitives::Viewport::new(1280.0, 800.0),
        ac2_ui::scenes::Now {
            instant: Instant::now(),
            wall: ac2_proto::units::WallNs(0),
        },
    );
    t.extend(
        tf.legend
            .iter()
            .map(|e| format!("legend {}", mask(&e.text))),
    );
    t.extend(
        tf.banners
            .iter()
            .map(|b| format!("banner {}", mask(&b.text))),
    );
    t.sort();
    t
}

/// Full screen is the pane alone: with the stimulus armed and then playing on the daemon,
/// the stage view draws the same texts as with it off (no badge, chip or strip joins it);
/// Esc still stops the stimulus from there, the stage view staying.
#[test]
fn the_stage_view_stays_the_pane_while_firing() -> R {
    if !have_gpu("the_stage_view_stays_the_pane_while_firing") {
        return Ok(());
    }
    let rig = Rig::start()?;
    let mut h = harness(options(&rig));
    step_until(&mut h, "live frames", live);
    h.state_mut().state.rig.stimulus.level = Some(ac2_proto::units::Dbfs(-24.0));
    h.state_mut().state.rig.stimulus.generator = Some(ac2_scene::stimulus::SignalKind::Pink);
    h.key_press(Key::W);
    h.key_press(Key::W);
    step_until(&mut h, "the stage view", |a| {
        a.state.stage_view() && a.state.layout.focus_kind() == PaneKind::Transfer
    });
    let quiet = |h: &mut Harness<'_, App>| {
        h.state_mut().state.toasts.list.clear();
        settle(h, 3);
        drawn_texts(h)
    };
    let off = quiet(&mut h);
    assert!(
        off.iter().any(|t| t.starts_with("legend Main L")),
        "the pane's legend among {off:?}"
    );
    h.key_press(Key::Space);
    step_until(&mut h, "armed", |a| {
        a.state.daemon().is_some_and(|s| s.generator.armed)
    });
    assert_eq!(quiet(&mut h), off, "armed");
    h.key_press(Key::Enter);
    step_until(&mut h, "firing", |a| {
        a.state.daemon().is_some_and(|s| s.generator.firing) && a.state.stage_view()
    });
    assert_eq!(quiet(&mut h), off, "firing");
    h.key_press(Key::Escape);
    step_until(&mut h, "stopped, the stage view staying", |a| {
        a.state.daemon().is_some_and(|s| {
            !s.generator.firing && !s.generator.armed && s.generator.owner.is_none()
        }) && a.state.stage_view()
    });
    Ok(())
}

/// The key hints on a crowded grid: the sweep pane shown beside the others (four narrow
/// panes under the transfer pane), the SPL meter focused with its hint line, and the
/// transfer title naming the selected stored trace. Then the SPL hint line's tooltip: the
/// pane's hints in full.
#[test]
fn key_hints() -> R {
    use egui_kittest::kittest::Queryable as _;
    if !have_gpu("key_hints") {
        return Ok(());
    }
    let rig = Rig::start()?;
    let mut opts = options(&rig);
    opts.prefs.layout.panes = Some(crate::common::rig_picks(
        crate::common::grid_with_sweep_prefs(),
    ));
    let mut h = harness(opts);
    step_until(&mut h, "live frames", live);
    // A capture of the transfer measurement.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num1);
    step_until(&mut h, "slot 1", |a| a.state.slots()[0].is_some());
    // The sweep pane focused (Alt+5), then the SPL meter.
    h.key_press_modifiers(Modifiers::ALT, Key::Num5);
    h.key_press_modifiers(Modifiers::ALT, Key::Num4);
    step_until(&mut h, "five panes, SPL focused", |a| {
        crate::common::visible(&a.state).len() == 5 && a.state.layout.focus_kind() == PaneKind::Spl
    });
    // The capture selected (the focus stays): the transfer title names it.
    h.key_press(Key::V);
    step_until(&mut h, "slot 1 selected", |a| {
        a.state.selected_trace_meta().is_some() && a.state.layout.focus_kind() == PaneKind::Spl
    });
    let names = |a: &App| {
        a.state
            .pane_caption_variants(crate::common::pane(&a.state, PaneKind::Transfer))
    };
    assert!(
        names(h.state())
            .last()
            .is_some_and(|n| n.starts_with("slot 1 (")),
        "{:?}",
        names(h.state())
    );
    let line = h
        .state()
        .state
        .key_hint_line(
            &h.state().keymap,
            crate::common::pane(&h.state().state, PaneKind::Spl),
            ac2_ui::keys::LabelStyle::Pc,
        )
        .ok_or("the SPL hint line")?;
    assert_eq!(line[0].text(), "G/Shift+G view");
    // The SPL hint line's tooltip (the title has none: it would cover the plot's top).
    settle(&mut h, 2);
    h.get_by_label("SPL keys").hover();
    settle(&mut h, 6);
    assert!(
        h.query_by_label_contains("most used keys").is_some(),
        "the tooltip"
    );
    Ok(())
}
