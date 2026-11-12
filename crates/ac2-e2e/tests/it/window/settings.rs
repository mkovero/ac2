//! Settings pages against the rig's own updates.

use super::*;
use crate::harness::R;
use ac2_proto::model::BandLeqPreset;
use egui_kittest::kittest::Queryable as _;

/// The SPL / Leq page taller than the window (the band meter's 11 windows) holds where the
/// wheel scrolled it while the rig's updates arrive: the Rig section at its bottom stays in view. A key moving the
/// focus brings the focused row back into view.
#[test]
fn leq_page_holds_its_scroll_while_the_rig_meters() -> R {
    if !have_gpu("leq_page_holds_its_scroll_while_the_rig_meters") {
        return Ok(());
    }
    let rig = Rig::start()?;
    // The rig's SPL meter, the page's, with the band meter's 545 windows.
    let state = rig.cli.state().ok_or("no state")?;
    let m = state
        .measurements
        .iter()
        .find(|m| m.id == FOH_SPL)
        .ok_or("no FOH SPL")?;
    let mut config = m.config.clone();
    if let MeasKind::Spl { config } = &mut config.kind {
        config.bands = Some(Box::new(BandLeqPreset::Finland545Lf.apply(None)));
    }
    config.scope = MeasScope::Rig;
    rig.call(Command::MeasUpdate {
        meas: FOH_SPL,
        base: m.config_rev,
        config,
    })?;
    let mut h = harness(options(&rig));
    step_until(&mut h, "live frames", live);
    h.state_mut().dispatch(ac2_ui::state::Msg::OpenRig);
    h.state_mut().dispatch(ac2_ui::state::Msg::Settings(
        ac2_ui::state::SettingsMsg::Page(ac2_ui::settings::Page::Leq),
    ));
    step_until(&mut h, "the SPL / Leq page with 11 band windows", |a| {
        a.state
            .overlay
            .leq()
            .is_some_and(|d| d.bands.rows.len() == 11)
    });
    h.step();
    let keep = |h: &Harness<'_, App>| {
        h.query_by_label(ac2_scene::osc::KEEP_ON_RIG)
            .map(|n| n.rect())
            .expect("the Keep on rig row")
    };
    let below = keep(&h);
    assert!(
        below.min.y > SIZE.y,
        "the Rig section starts below the window: {below:?}"
    );
    h.event(Event::PointerMoved(egui::pos2(700.0, 400.0)));
    for _ in 0..20 {
        h.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -400.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        });
        h.step();
    }
    settle(&mut h, 10);
    let scrolled = keep(&h);
    assert!(
        scrolled.max.y < SIZE.y,
        "the wheel brought the Rig section up: {scrolled:?}"
    );
    // The rig's updates, one a frame (another client setting its Art-Net fade): the page
    // stays where the wheel left it.
    let mut rev = rig.cli.state().ok_or("no state")?.rig.rev;
    for n in 1..=20 {
        let r = rig.call(Command::ArtnetFade {
            base: rev,
            seconds: ac2_proto::units::Seconds(f64::from(n) * 0.1),
        })?;
        rev = match r {
            ReplyBody::Ack { rev } => rev,
            other => return Err(format!("artnet.fade: {other:?}").into()),
        };
        h.step();
    }
    step_until(&mut h, "the rig's updates", |a| {
        a.state.daemon().is_some_and(|s| s.rig.rev == rev)
    });
    settle(&mut h, 5);
    assert_eq!(keep(&h), scrolled, "a mirror update moved the page");
    // ↑ from the preset row wraps to the last row; ↓ back to the preset brings it into view.
    h.key_press(Key::ArrowUp);
    h.key_press(Key::ArrowDown);
    settle(&mut h, 10);
    assert!(
        keep(&h).min.y > SIZE.y,
        "the focused preset row came back into view"
    );
    Ok(())
}
