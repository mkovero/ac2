//! The walk pane's control bar: its buttons under the pointer.

use super::*;

/// The recorded walk "Mains" (two mics at P1 and P2, nothing captured) opened in its pane.
fn walk_pane(h: &mut Harness<'_, App>) {
    step_until(h, "live frames", live);
    h.key_press_modifiers(Modifiers::SHIFT, Key::W);
    step_until(h, "the walks", |a| {
        matches!(&a.state.overlay, Overlay::Choose(_))
    });
    h.key_press(Key::ArrowDown);
    h.key_press(Key::Enter);
    step_until(h, "the walk pane", |a| {
        a.state.overlay == Overlay::None && a.state.layout.focus_kind() == PaneKind::Walk
    });
    h.state_mut().state.toasts.list.clear();
    settle(h, 3);
}

/// The bar has a button per action, the capture the largest. A press on one sends its
/// command (the skip, with two mics standing, asks which point); a dimmed one (nothing is
/// captured, so nothing to redo) does nothing, not even the command's refusal. **T** strips
/// the pane to its picture and the bar stays; no button covers the plan above it.
#[test]
fn walk_pane_control_bar() {
    use ac2_scene::walk::controls::Action;
    use ac2_ui::state::Msg;
    if !have_gpu("walk_pane_control_bar") {
        return;
    }
    let _rig = Rig::scene("walk");
    let mut h = harness(options());
    walk_pane(&mut h);
    let rects: Vec<egui::Rect> = Action::ALL
        .iter()
        .map(|a| h.get_by_label(a.label()).rect())
        .collect();
    let capture = rects[2];
    for (a, r) in Action::ALL.iter().zip(&rects) {
        assert!(r.height() >= 40.0, "{a:?}: a fingertip");
        assert!(capture.area() >= r.area(), "{a:?}");
    }
    snapshot(&mut h, "walk_pane");

    // Dimmed: the press is nobody's.
    h.get_by_label(Action::Redo.label()).click();
    settle(&mut h, 3);
    assert_eq!(h.state().state.overlay, Overlay::None);
    assert!(h.state().state.toasts.list.is_empty(), "no refusal either");
    // The command itself, as the palette runs it, refuses in the button's words.
    h.state_mut()
        .dispatch(Msg::Command(ac2_ui::keys::CommandId::WalkRedo));
    let said = h.state().state.toasts.list.last().map(|t| t.text.clone());
    assert_eq!(
        said.as_deref(),
        Some("no point is captured yet: nothing to redo")
    );
    h.state_mut().state.toasts.list.clear();

    // T through every chrome step: the bar is a control, never hidden.
    for _ in 0..4 {
        h.key_press(Key::T);
        settle(&mut h, 2);
        for a in Action::ALL {
            h.get_by_label(a.label());
        }
    }

    // Another pane focused: a press on the walk's button acts on the walk pane.
    let other = crate::common::pane(&h.state().state, PaneKind::Transfer);
    h.state_mut().dispatch(Msg::FocusPane(other));
    settle(&mut h, 2);
    h.get_by_label(Action::Skip.label()).click();
    step_until(
        &mut h,
        "which point to skip",
        |a| matches!(&a.state.overlay, Overlay::Choose(c) if c.title == "Skip which point?"),
    );
    assert_eq!(h.state().state.layout.focus_kind(), PaneKind::Walk);
    h.key_press(Key::Escape);
    step_until(&mut h, "closed", |a| a.state.overlay == Overlay::None);
}
