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

/// Space without a level asks for one and takes nothing; typed, Space arms, Enter plays, ↓
/// steps the level a dB, Esc stops and releases: each step as the daemon's generator says.
#[test]
fn stimulus_flow_against_the_daemon() -> R {
    if !have_gpu("stimulus_flow_against_the_daemon") {
        return Ok(());
    }
    let rig = Rig::start()?;
    let mut h = harness(options(&rig));
    step_until(&mut h, "live frames", live);
    h.key_press(Key::Space);
    step_until(&mut h, "level prompt", |a| {
        matches!(a.state.overlay, Overlay::Prompt(_))
    });
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
