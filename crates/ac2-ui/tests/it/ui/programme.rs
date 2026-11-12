//! The Programme pane and the Inputs list's programme readouts, against the fake daemon's
//! own meters (`Shared::meters`).

use super::*;
use ac2_scene::programme::loudness::{LoudnessReadout, value_text};

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

/// Input row `channel` dragged onto the focused pane's right edge: a Programme pane
/// beside it metering that row (a pair's on its goniometer), as the mouse would.
fn drop_input(h: &mut Harness<'_, App>, channel: u16) {
    use ac2_ui::state::{DragItem, DragMsg, DropTarget, Msg};
    let a = h.state_mut();
    let pane = a.state.layout.focus;
    a.dispatch(Msg::RowDrag(DragMsg::Start(DragItem::Input(channel))));
    a.dispatch(Msg::RowDrag(DragMsg::Over(Some(DropTarget {
        pane,
        edge: Some(ac2_scene::pane_edit::Edge::Right),
    }))));
    a.dispatch(Msg::RowDrag(DragMsg::Drop));
    h.step();
}

/// The scene text of the focused pane (the meters P opens).
fn programme_texts(h: &Harness<'_, App>) -> Vec<String> {
    pane_texts(h, h.state().state.layout.focus)
}

fn pane_texts(h: &Harness<'_, App>, pane: ac2_ui::state::PaneId) -> Vec<String> {
    let st = &h.state().state;
    let scene = ac2_ui::scenes::programme(
        st,
        pane,
        &ac2_scene::theme::Theme::dark(),
        ac2_scene::primitives::Viewport::new(900.0, 450.0),
    )
    .expect("a Programme scene");
    scene
        .scene
        .layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|l| l.text.clone()))
        .collect()
}

/// A reset at 19:02 UTC on a day that is never the test's: the since line reads the same on
/// every run.
const SHOW_RESET: ac2_proto::units::WallNs = ac2_proto::units::WallNs(1_767_294_120_000_000_000);

/// The readout of Programme pane `pane`'s loudness plate, if it has one.
fn loudness(a: &App, pane: ac2_ui::state::PaneId) -> Option<LoudnessReadout> {
    a.state.programme_pane(pane)?.0.loudness
}

/// A click picks the pair's Inputs row, dropped twice it meters the pair in two new
/// Programme panes: the goniometer on the left, the meters (G) on the right with the focus. The meters' title says
/// the ballistic, the pair and the correlator window, the chip that the right side is too
/// quiet to correlate. Both panes and their views outlive a save and load of the layout.
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
        a.state.rig.input_pick == Some(2)
    });
    // Dropped beside the focused pane: the goniometer; dropped again beside that and
    // stepped (G): the meters, with the focus.
    drop_input(&mut h, 2);
    step_until(&mut h, "a Programme pane", |a| {
        common::visible(&a.state).contains(&PaneKind::Programme)
    });
    let gonio = h.state().state.layout.focus;
    drop_input(&mut h, 2);
    h.key_press(Key::G);
    h.step();
    let st = &h.state().state;
    let panes: Vec<_> = st
        .visible_panes()
        .into_iter()
        .filter(|p| st.layout.kind(*p) == PaneKind::Programme)
        .collect();
    let [left, pane] = panes[..] else {
        panic!("two Programme panes: {panes:?}")
    };
    assert_eq!(left, gonio);
    assert_eq!(st.layout.focus, pane);
    assert_eq!(st.pane_title(pane), "true peak · Main · 1 s");
    // The goniometer fills its pane: its axes, the correlation scale and the pair's chip,
    // no bars.
    step_until(&mut h, "the chip gated", |a| {
        a.state
            .programme_pane(pane)
            .and_then(|(p, _)| p.pair)
            .is_some_and(|p| p.correlation.chip().text == "right quiet")
    });
    assert_eq!(
        pane_texts(&h, gonio),
        ["L", "R", "correlation", "right quiet"]
    );
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
            value_text(Some(l.short_term)) == "−18.0" && l.since == "since 1 Jan 19:02 · R resets"
        })
    });
    let l = loudness(h.state(), pane).expect("the pair's loudness");
    assert_eq!(l.title, "Main");
    let texts = programme_texts(&h);
    for want in [
        "Main",
        "Int",
        "−18.0",
        "LUFS",
        "Mom",
        "Short",
        "LRA",
        "0.0",
        "settling",
        "TP max",
        "−11.8",
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
        |a| a.state.rig.programme.rings.clear(),
        |a| !a.state.rig.programme.glowing(),
    );

    // The layout keeps the pane and what it meters, through the preferences file.
    let mut prefs = h.state().state.prefs.clone();
    prefs.layout = h.state().state.layout_prefs();
    let read = ac2_ui::prefs::UiPrefs::from_toml(&prefs.to_toml()).expect("prefs read back");
    let saved: Vec<_> = read
        .layout
        .panes
        .as_ref()
        .expect("a layout")
        .views
        .iter()
        .filter(|v| v.kind == PaneKind::Programme)
        .map(|v| (v.meters, v.modes.programme))
        .collect();
    let pair = Some(ac2_ui::state::Metered::Pair { left: 2, right: 3 });
    use ac2_scene::view::ProgrammeMode as M;
    assert_eq!(saved.len(), 2, "{saved:?}");
    for want in [(pair, M::Goniometer), (pair, M::Meters)] {
        assert!(saved.contains(&want), "{saved:?}");
    }

    // W gives the pane the window: room for the loudness plate beside the bars, which a
    // pane a quarter of the window wide docks above them or drops.
    h.key_press(Key::W);
    snapshot_when(
        &mut h,
        "programme_pair_loudness",
        |a| a.state.rig.programme.rings.clear(),
        |a| !a.state.rig.programme.glowing(),
    );
}
