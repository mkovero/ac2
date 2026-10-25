//! The Programme pane and the Inputs list's programme readouts, against the fake daemon's
//! own meters (`Shared::meters`).

use super::*;
use ac2_scene::programme::loudness::{LoudnessColumn, LoudnessReadout};

/// Inputs 3 + 4 as the line pair `Main`, its right side silent, the fake's meters on.
fn pair_rig() -> common::Rig {
    use ac2_proto::model::{InputKind, InputSetup, InputSetups, Pair};
    let rig = common::Rig::start();
    {
        let mut s = rig.fake.lock();
        let mut inputs = s.state.inputs.clone();
        inputs.retain(|i| i.channel != 2 && i.channel != 3);
        inputs.extend([2, 3].map(|channel| InputSetup {
            channel,
            kind: InputKind::Line,
            pair: None,
        }));
        inputs.sort_by_key(|i| i.channel);
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

/// A reset at 19:02 UTC on a day that is never the test's: the since line reads the same on
/// every run.
const SHOW_RESET: ac2_proto::units::WallNs = ac2_proto::units::WallNs(1_767_294_120_000_000_000);

/// The readout of Programme pane `pane`'s loudness column, if it shows one.
fn loudness(a: &App, pane: ac2_ui::state::PaneId) -> Option<LoudnessReadout> {
    match a.state.programme_pane(pane)?.0.loudness? {
        LoudnessColumn::Readout(r) => Some(r),
        LoudnessColumn::Message(_) => None,
    }
}

/// What Programme pane `pane`'s loudness column says instead of a readout.
fn loudness_message(a: &App, pane: ac2_ui::state::PaneId) -> Option<String> {
    match a.state.programme_pane(pane)?.0.loudness? {
        LoudnessColumn::Message(m) => Some(m),
        LoudnessColumn::Readout(_) => None,
    }
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
    for want in ["right quiet", "−11.8 dBTP", "max"] {
        assert!(
            texts.iter().any(|t| t == want),
            "{want}: {}",
            common::scene_texts_excerpt(&texts, common::TEXTS_SHOWN)
        );
    }
    // The pair's loudness: a −12 dBFS-peak tone on one side of two reads −18 LUFS (the
    // fake's BS.1770 figure); the range settles for a minute. The since line is pinned to a
    // fixed reset so the picture does not follow the wall clock.
    for st in rig.fake.lock().loudness_starts.values_mut() {
        st.since = SHOW_RESET;
    }
    step_until(&mut h, "the pair's short-term loudness", |a| {
        loudness(a, pane).is_some_and(|l| {
            l.short_term_text() == "Short −18.0" && l.since == "since 1 Jan 19:02 · R resets"
        })
    });
    let l = loudness(h.state(), pane).expect("the pair's loudness");
    assert_eq!(l.title, "Main");
    let texts = programme_texts(&h);
    for want in [
        "Main",
        "Mom −18.0",
        "Short −18.0",
        "Int −18.0 LUFS",
        "LRA 0.0 LU (settling)",
        "TP max −11.8 dBTP",
        "since 1 Jan 19:02 · R resets",
    ] {
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

    // W gives the pane the window: room for the loudness column beside the bars, which a
    // pane a quarter of the window wide drops to keep the bars' readouts whole.
    h.key_press(Key::W);
    snapshot_when(
        &mut h,
        "programme_pair_loudness",
        |a| a.state.programme.rings.clear(),
        |a| !a.state.programme.glowing(),
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
            .any(|t| t.text == "max, overs and loudness reset on input 2"),
    );
}

/// A mic input's pane has no loudness and says why; a pane of several inputs asks for one
/// input or a pair; R's toast names what it resets.
#[test]
fn loudness_says_why_a_pane_has_none() {
    if !have_gpu("loudness_says_why_a_pane_has_none") {
        return;
    }
    let rig = pair_rig();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "input 2's levels", |a| {
        a.state.row_meters(1).is_some_and(|m| {
            a.state
                .programme_of(m)
                .inputs
                .iter()
                .all(|i| i.max.is_some())
        })
    });
    h.state_mut().dispatch(ac2_ui::state::Msg::InputPick(1));
    h.key_press(Key::P);
    step_until(&mut h, "a Programme pane", |a| {
        common::visible(&a.state).contains(&PaneKind::Programme)
    });
    let pane = common::pane(&h.state().state, PaneKind::Programme);
    step_until(&mut h, "the mic message", |a| {
        loudness_message(a, pane).as_deref() == Some("no loudness on a mic input")
    });
    assert!(
        programme_texts(&h)
            .iter()
            .any(|t| t == "no loudness on a mic input")
    );
    let several = ac2_ui::state::Metered::inputs_of(&[0, 2, 3]).expect("three inputs");
    if let Some(v) = h.state_mut().state.layout.view_mut(pane) {
        v.meters = Some(several);
    }
    h.state_mut().state_edited();
    step_until(&mut h, "the several-inputs message", |a| {
        loudness_message(a, pane).as_deref() == Some("loudness: one input or a pair")
    });
    // R on the focused pane, not on a picked row.
    h.state_mut().state.input_pick = None;
    h.key_press(Key::R);
    step_until(&mut h, "the reset toast", |a| {
        a.state
            .toasts
            .list
            .iter()
            .any(|t| t.text == "max, overs and loudness reset on input 1, 3, 4")
    });
}

/// Only the bars move in the Inputs list: the pair row's readout lines, its correlation and
/// everything under them keep their rects whatever the levels read, overs or no overs,
/// CLIP or not.
#[test]
fn inputs_rows_keep_their_geometry() {
    use egui::accesskit::Role;
    use egui_kittest::kittest::By;
    if !have_gpu("inputs_rows_keep_their_geometry") {
        return;
    }
    let rig = pair_rig();
    let mut h = harness(options(Some(&rig)));
    // Every label of the sidebar: the rows' names, readouts and the lists under them.
    let rects = |h: &Harness<'_, App>| {
        let mut r: Vec<[i32; 4]> = h
            .query_all(By::new().predicate(|n| n.role() == Role::Label))
            .map(|n| n.rect())
            .filter(|r| r.max.x <= 240.0)
            .map(|r| [r.min.x, r.min.y, r.max.x, r.max.y].map(|v| (v * 8.0).round() as i32))
            .collect();
        r.sort_unstable();
        r
    };
    let left = |a: &App| {
        a.state
            .row_meters(2)
            .map(|m| a.state.programme_of(m))
            .and_then(|p| p.inputs.first().cloned())
    };
    let mut first = None;
    for (peak, overs, clip) in [
        (-60.0, 0, false),
        (-0.1, 12, false),
        (f32::NEG_INFINITY, 999, true),
        (-12.0, 1, true),
    ] {
        {
            let mut s = rig.fake.lock();
            s.set_input_peak(2, peak);
            s.input_overs.insert(2, (overs, clip));
        }
        step_until(&mut h, "the levels shown", |a| {
            left(a).is_some_and(|i| {
                i.overs == overs
                    && i.clipped == clip
                    && i.ppm.is_some_and(|p| p.is_finite() == peak.is_finite())
            })
        });
        h.step();
        let got = rects(&h);
        let want = first.get_or_insert_with(|| got.clone());
        assert_eq!(*want, got, "{peak} dBFS, {overs} overs, clip {clip}");
    }
}
