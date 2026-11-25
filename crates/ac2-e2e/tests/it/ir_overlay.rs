//! A stopped transfer captured with Ctrl+1: its kept result goes into slot 1 with its IR,
//! and the IR view draws the stopped live IR and the capture's together, a legend naming
//! both.

use crate::harness::R;
use crate::window::{MAIN_L, Rig};
use ac2_scene::theme::Theme;
use ac2_scene::view::IrPane;
use ac2_ui::state::AppState;
use std::time::Instant;

fn scene(s: &AppState, keys: &ac2_ui::keys::Keymap) -> ac2_scene::ir::IrScene {
    ac2_ui::scenes::ir(
        s,
        crate::common::ir_pane(s),
        keys,
        &Theme::dark(),
        ac2_scene::primitives::Viewport::new(1000.0, 450.0),
        ac2_ui::scenes::Now {
            instant: Instant::now(),
            wall: ac2_proto::units::WallNs(0),
        },
    )
}

#[test]
fn a_stopped_transfer_captured_draws_beside_its_live_ir() -> R {
    let rig = Rig::start()?;
    let _noise = crate::harness::noise(&rig.cli)?;
    let mut d = crate::harness::on_rig(&rig)?;
    d.until("Main L's IR", |s| s.ir_frame_of(IrPane::Live).is_some())?;
    d.key("Alt+3");
    d.key("S");
    d.until("Main L stopped", |s| {
        s.meas(MAIN_L).is_some_and(|m| !m.running)
    })?;

    d.key("Ctrl+1");
    d.until("the capture in slot 1 with its data", |s| {
        s.rig
            .traces
            .values()
            .any(|(t, _)| t.meta.edit.slot == Some(1) && t.meta.edit.name == "Main L S1")
    })?;
    let said: Vec<&str> = d.st.toasts.list.iter().map(|t| t.text.as_str()).collect();
    assert!(said.contains(&"slot 1: Main L captured"), "{said:?}");

    let s = scene(&d.st, &d.keys);
    let rows: Vec<&str> = s.legend.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(rows, ["Main L · stopped", "Main L S1"]);
    assert_eq!(s.tag.as_deref(), Some("stopped"));
    let (id, _) =
        d.st.rig
            .traces
            .iter()
            .find(|(_, (t, _))| t.meta.edit.name == "Main L S1")
            .ok_or("the capture")?;
    let colours = d.st.curve_colours(&Theme::dark());
    let live = ac2_ui::scenes::meas_color(&d.st, &Theme::dark(), MAIN_L);
    let stored = colours.trace(*id);
    let lines: Vec<_> = s.scene.layers[1].polylines.iter().collect();
    assert!(lines.iter().any(|p| p.stroke.color == live), "the live IR");
    assert!(
        lines.iter().any(|p| p.stroke.color == stored),
        "the capture's IR"
    );
    Ok(())
}
