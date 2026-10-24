//! The Programme pane and the Inputs list's programme readouts, against the fake daemon's
//! own meters (`Shared::meters`).

use super::*;

/// Inputs 3 + 4 as the pair `Main`, its right side silent, the fake's meters on.
fn pair_rig() -> common::Rig {
    use ac2_proto::model::{InputSetups, Pair};
    let rig = common::Rig::start();
    {
        let mut s = rig.fake.lock();
        let inputs = s.state.inputs.clone();
        s.commit(ac2_proto::Change::Inputs(InputSetups {
            inputs,
            pairs: vec![Pair {
                left: 2,
                right: 3,
                name: Some("Main".into()),
            }],
        }));
        s.set_input_peak(3, f32::NEG_INFINITY);
        s.meters = true;
    }
    rig
}

fn programme_texts(h: &Harness<'_, App>) -> Vec<String> {
    let st = &h.state().state;
    let pane = common::pane(st, PaneKind::Programme);
    let scene = ac2_ui::scenes::programme(
        st,
        pane,
        &ac2_scene::theme::Theme::dark(),
        ac2_scene::primitives::Viewport {
            width: 900.0,
            height: 450.0,
        },
    )
    .expect("a Programme scene");
    scene
        .layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|l| l.text.clone()))
        .collect()
}

/// A click picks the pair's Inputs row, P meters it in a new Programme pane: the title says
/// the ballistic, the pair and the correlator window, the chip that the right side is too
/// quiet to correlate. The pane outlives a save and load of the layout.
#[test]
fn programme_pane_of_a_pair() {
    if !have_gpu("programme_pane_of_a_pair") {
        return;
    }
    let rig = pair_rig();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "the pair's levels", |a| {
        a.state.session_inputs().iter().any(|r| r.right.is_some())
            && a.state.row_meters(2).is_some_and(|m| {
                a.state
                    .programme_of(m)
                    .inputs
                    .iter()
                    .all(|i| i.max.is_some())
            })
    });
    let label = h
        .state()
        .state
        .session_inputs()
        .into_iter()
        .find(|r| r.channel == 2)
        .map(|r| r.label)
        .expect("the pair's row");
    h.get_by_label(&label).click();
    step_until(&mut h, "the pair's row picked", |a| {
        a.state.input_pick == Some(2)
    });
    h.key_press(Key::P);
    step_until(&mut h, "a Programme pane", |a| {
        common::visible(&a.state).contains(&PaneKind::Programme)
    });
    let pane = common::pane(&h.state().state, PaneKind::Programme);
    assert_eq!(h.state().state.layout.focus, pane);
    assert_eq!(h.state().state.pane_title(pane), "true peak · Main · 1 s");
    step_until(&mut h, "the chip gated", |a| {
        a.state
            .programme_pane(pane)
            .and_then(|(p, _)| p.pair)
            .is_some_and(|p| p.correlation.chip().text == "right quiet")
    });
    let texts = programme_texts(&h);
    // The bar reads the true peak in dBTP, its max too, and no second true-peak line.
    assert!(!texts.iter().any(|t| t.ends_with("dBFS")), "{texts:?}");
    for want in ["right quiet", "−11.8 dBTP", "max −11.8 dBTP"] {
        assert!(
            texts.iter().any(|t| t == want),
            "{want}: {}",
            common::scene_texts_excerpt(&texts, common::TEXTS_SHOWN)
        );
    }
    h.state_mut().state.toasts.list.clear();
    // The embers' brightness follows the wall clock between frames: the picture shows the
    // pane between two goniometer frames, the field empty (ac2-plot tests the embers).
    snapshot_when(
        &mut h,
        "programme_pair",
        |a| a.state.programme.rings.clear(),
        |a| !a.state.programme.glowing(),
    );

    // The layout keeps the pane and what it meters, through the preferences file.
    let mut prefs = h.state().state.prefs.clone();
    prefs.layout = h.state().state.layout_prefs();
    let read = ac2_ui::prefs::UiPrefs::from_toml(&prefs.to_toml()).expect("prefs read back");
    let view = read
        .layout
        .panes
        .as_ref()
        .expect("a layout")
        .views
        .iter()
        .find(|v| v.kind == PaneKind::Programme)
        .expect("the Programme pane saved");
    assert_eq!(
        view.meters,
        Some(ac2_ui::state::Metered::Pair { left: 2, right: 3 })
    );
}

/// R on a picked row resets that input's max: the next frame holds the level since.
#[test]
fn r_resets_the_picked_inputs_max() {
    if !have_gpu("r_resets_the_picked_inputs_max") {
        return;
    }
    let rig = pair_rig();
    rig.fake.lock().set_input_peak(1, -3.0);
    let mut h = harness(options(Some(&rig)));
    let max_of = |a: &App| {
        a.state
            .row_meters(1)
            .map(|m| a.state.programme_of(m))
            .and_then(|p| p.inputs.first().map(|i| i.max_text(p.ballistic)))
    };
    step_until(&mut h, "in 2's max", |a| {
        max_of(a).as_deref() == Some("max −2.8 dBTP")
    });
    rig.fake.lock().set_input_peak(1, -20.0);
    h.state_mut().dispatch(ac2_ui::state::Msg::InputPick(1));
    h.key_press(Key::R);
    step_until(&mut h, "the max reset", |a| {
        max_of(a).as_deref() == Some("max −19.8 dBTP")
    });
    assert!(
        h.state()
            .state
            .toasts
            .list
            .iter()
            .any(|t| t.text == "max and overs reset on input 2"),
    );
}
