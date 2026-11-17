//! The rig monitor on a live rig.

use super::*;

/// The rig monitor's look: an SPL meter kept by the rig leads the tree under the Rig header
/// with its cue chip; the Rig monitor page shows the targets, monitor at start, the test
/// message, how sending fares and the rig's meters.
#[test]
fn rig_monitor_views() {
    if !have_gpu("rig_monitor_views") {
        return;
    }
    let _rig = Rig::scene("monitor");
    let mut h = harness(options());
    step_until(&mut h, "live frames", |a| live_with(a, 5));
    step_until(&mut h, "the Rig header", |a| {
        a.state.tree_rows().first().map(|r| r.name.as_str()) == Some("Rig")
    });
    h.event(Event::PointerGone);
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "tree_rig_group");
    h.state_mut().dispatch(ac2_ui::state::Msg::OpenRig);
    step_until(&mut h, "the Rig monitor page", |a| {
        a.state
            .overlay
            .settings()
            .is_some_and(|s| s.page == ac2_ui::settings::Page::Rig)
    });
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "settings_rig");
}
