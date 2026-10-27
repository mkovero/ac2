//! EQ what-if sets on the transfer pane: the prediction and the set's dashed filter curve
//! in the legend, and the Alt+E editor over the plot.

use super::*;

fn has_prediction(app: &App) -> bool {
    live_with(app, 5)
        && app.state.rig.data.as_ref().is_some_and(|d| {
            d.latest
                .get(&Topic::Data {
                    meas: MeasId(6),
                    stream: Stream::Tf,
                })
                .is_some()
        })
        && app.state.trace_list().iter().any(|t| t.id.0 == 40)
}

fn legend(st: &ac2_ui::state::AppState) -> Vec<String> {
    ac2_ui::scenes::transfer(
        st,
        crate::common::pane(st, PaneKind::Transfer),
        &ac2_scene::theme::Theme::dark(),
        ac2_scene::primitives::Viewport {
            width: 1000.0,
            height: 450.0,
        },
        ac2_ui::scenes::Now {
            instant: Instant::now(),
            wall: ac2_proto::units::WallNs(0),
        },
    )
    .legend
    .iter()
    .map(|e| e.text.clone())
    .collect()
}

/// Main L, its prediction after EQ 1 and EQ 1's dashed curve; then the editor over them.
#[test]
fn transfer_eq_and_editor() {
    if !have_gpu("transfer_eq_and_editor") {
        return;
    }
    let rig = common::Rig::start();
    rig.add_eq();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "the prediction's frames", has_prediction);
    step_until(&mut h, "the legend", |a| {
        let l = legend(&a.state);
        l.iter()
            .any(|l| l.starts_with("Main L after EQ 1 · assumes none of it in"))
            && l.iter().any(|l| l.starts_with("EQ 1"))
    });
    snapshot(&mut h, "transfer_eq");

    h.key_press_modifiers(Modifiers::ALT, Key::E);
    step_until(&mut h, "the EQ editor", |a| {
        matches!(&a.state.overlay, Overlay::Eq(_))
    });
    // The first section's gain focused, as after ↓ and Tab Tab.
    h.key_press(Key::ArrowDown);
    h.key_press(Key::Tab);
    h.key_press(Key::Tab);
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "eq_editor");
}
