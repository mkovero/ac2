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

/// Reset while stopped, a transfer's kept result goes: its curve goes from the views, which
/// say it is stopped as with nothing measured, and Ctrl+1 is refused in the daemon's words
/// while slot 1's earlier capture stays.
#[test]
fn a_stopped_transfer_reset_has_nothing_to_capture_and_keeps_its_slot() -> R {
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
    let first = |s: &AppState| {
        s.rig
            .traces
            .values()
            .find(|(t, _)| t.meta.edit.slot == Some(1) && t.meta.edit.name == "Main L S1")
            .map(|(t, _)| t.meta.id)
    };
    d.until("the capture in slot 1", |s| first(s).is_some())?;
    let kept = first(&d.st).ok_or("the capture")?;

    rig.cli
        .call(ac2_proto::Command::MeasReset { meas: MAIN_L })?;
    d.until("Main L's IR gone", |s| {
        s.ir_frame_of(IrPane::Live).is_none()
    })?;
    d.key("1");
    d.until("the capture hidden", |s| {
        s.rig
            .traces
            .get(&kept)
            .is_some_and(|(t, _)| !t.meta.edit.visible)
    })?;
    let s = scene(&d.st, &d.keys);
    assert_eq!(s.note.as_deref(), Some("Main L stopped — S starts it"));

    d.key("Ctrl+1");
    let refused = "slot 1: nothing captured · Main L: no result to capture (it has not \
                   measured with its current settings)";
    d.until("the refusal", |s| {
        s.toasts.list.iter().any(|t| t.text == refused)
    })?;
    d.pump();
    assert_eq!(first(&d.st), Some(kept), "slot 1's capture stays");
    Ok(())
}
