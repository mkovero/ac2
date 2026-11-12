//! What a transfer pane shows is what was dropped on it.

use super::*;
use crate::harness::R;
use egui_kittest::kittest::Queryable as _;

/// The transfer pane's title names the measurement it shows in plain text; Delay tower's
/// row dropped on the pane switches it there, the selection staying where it was.
#[test]
fn pane_title_and_a_drop() -> R {
    if !have_gpu("pane_title_and_a_drop") {
        return Ok(());
    }
    let rig = Rig::start()?;
    let mut h = harness(options(&rig));
    step_until(&mut h, "live frames", live);
    let tf = crate::common::pane(&h.state().state, PaneKind::Transfer);
    assert!(
        h.query_by_label("1  Transfer · Main L").is_some(),
        "the transfer pane's title"
    );
    let selected = h.state().state.rig.selected;
    drop_on_kind(&mut h, PaneKind::Transfer, DELAY_TOWER);
    step_until(&mut h, "delay tower shown", |a| {
        a.state.pane_meas(tf).map(|m| m.id) == Some(DELAY_TOWER)
    });
    assert_eq!(
        h.state().state.rig.selected,
        selected,
        "a drop selects nothing"
    );
    // The pane now leads with it: first legend row, IR of it.
    let st = &h.state().state;
    let s = ac2_ui::scenes::transfer(
        st,
        tf,
        &ac2_scene::theme::Theme::dark(),
        ac2_scene::primitives::Viewport::new(1000.0, 450.0),
        ac2_ui::scenes::Now {
            instant: Instant::now(),
            wall: ac2_proto::units::WallNs(0),
        },
    );
    assert_eq!(s.legend[0].name, "Delay tower");
    assert_eq!(
        ac2_ui::scenes::focus_tf(st).map(|m| m.id),
        Some(DELAY_TOWER)
    );
    Ok(())
}
