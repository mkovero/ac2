//! Stored traces, the measurement and trace lists.

use super::*;

/// A on the selected measurement hides its curve (its list row says `hidden`, dimmed; the
/// transfer pane's title says so and its legend loses it), and Backspace asks before it is
/// deleted, naming it and what goes.
#[test]
fn measurement_hidden_and_delete_confirm() {
    if !have_gpu("measurement_hidden_and_delete_confirm") {
        return;
    }
    let _rig = Rig::start();
    let mut h = harness(options());
    step_until(&mut h, "live frames", live);
    // On the transfer pane first: its row then only selects, the panes staying as they are.
    drop_on_kind(&mut h, PaneKind::Transfer, MeasId(2));
    // Past the first measurement's live curve row to the next measurement.
    h.key_press(Key::Tab);
    h.key_press(Key::Tab);
    step_until(&mut h, "delay tower", |a| {
        a.state.rig.selected == Some(MeasId(2))
    });
    h.key_press(Key::A);
    step_until(&mut h, "delay tower hidden", |a| {
        a.state.rig.hidden_meas.contains(&MeasId(2))
    });
    let caption = h
        .state()
        .state
        .pane_caption(crate::common::pane(&h.state().state, PaneKind::Transfer));
    assert!(
        caption
            .as_deref()
            .is_some_and(|c| c.starts_with("Delay tower hidden · ")),
        "{caption:?}"
    );
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "measurement_hidden");
    h.key_press(Key::Backspace);
    step_until(
        &mut h,
        "the confirmation",
        |a| matches!(&a.state.overlay, Overlay::Delete(p) if p.target == ac2_ui::state::DeleteTarget::Meas(MeasId(2))),
    );
    h.step();
    snapshot(&mut h, "measurement_delete_confirm");
    h.key_press(Key::Escape);
    step_until(&mut h, "kept", |a| {
        a.state.overlay == Overlay::None && a.state.measurements().len() == 4
    });
}
