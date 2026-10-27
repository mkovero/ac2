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
        ac2_scene::primitives::Viewport {
            width: 900.0,
            height: 450.0,
        },
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

/// A click picks the pair's Inputs row, P meters it in two new Programme panes: the
/// goniometer on the left, the meters on the right with the focus. The meters' title says
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
    h.key_press(Key::P);
    step_until(&mut h, "a Programme pane", |a| {
        common::visible(&a.state).contains(&PaneKind::Programme)
    });
    let st = &h.state().state;
    let panes: Vec<_> = st
        .visible_panes()
        .into_iter()
        .filter(|p| st.layout.kind(*p) == PaneKind::Programme)
        .collect();
    let [gonio, pane] = panes[..] else {
        panic!("two Programme panes: {panes:?}")
    };
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

/// A mic input's pane has no loudness plate; the mic beside a pair's inputs leaves one
/// source, so that pane has the pair's; R's toast names what it resets.
#[test]
fn loudness_plate_only_for_one_source() {
    if !have_gpu("loudness_plate_only_for_one_source") {
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
    assert_eq!(loudness(h.state(), pane), None, "a mic has no source");
    assert!(!programme_texts(&h).iter().any(|t| t.contains("LUFS")));
    let several = ac2_ui::state::Metered::inputs_of(&[1, 2, 3]).expect("three inputs");
    if let Some(v) = h.state_mut().state.layout.view_mut(pane) {
        v.meters = Some(several);
    }
    h.state_mut().state_edited();
    step_until(&mut h, "the pair's plate beside the mic", |a| {
        a.state.programme_pane(pane).is_some_and(|(p, _)| {
            p.bars.len() == 3 && p.loudness.is_some_and(|l| l.title == "Main")
        })
    });
    // R on the focused pane, not on a picked row.
    h.state_mut().state.rig.input_pick = None;
    h.key_press(Key::R);
    step_until(&mut h, "the reset toast", |a| {
        a.state
            .toasts
            .list
            .iter()
            .any(|t| t.text == "max, overs and loudness reset on input 2, 3, 4")
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

/// From the fake daemon's pair: Mom loudness chosen on Settings › Rig monitor turns the
/// pair's two peak bars into one LUFS bar of the pair, titled so, with the target; the
/// loudness plate is a plate the mouse can take over the whole pane; T's third step takes
/// it (and its box) away and the next brings both back.
#[test]
fn mom_loudness_reads_lufs_bars() {
    use ac2_proto::model::Ballistic as Wire;
    use ac2_scene::programme::ProgrammeBar;
    use ac2_ui::settings::RigRow;
    use ac2_ui::state::{Msg, SettingsMsg};
    if !have_gpu("mom_loudness_reads_lufs_bars") {
        return;
    }
    let rig = pair_rig();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "the pair's levels", |a| {
        a.state.row_meters(2).is_some_and(|m| {
            a.state
                .programme_of(m)
                .inputs
                .iter()
                .all(|i| i.max.is_some())
        })
    });
    h.state_mut().dispatch(Msg::InputPick(2));
    h.key_press(Key::P);
    step_until(&mut h, "a Programme pane", |a| {
        common::visible(&a.state).contains(&PaneKind::Programme)
    });
    let pane = h.state().state.layout.focus;
    h.state_mut().dispatch(Msg::OpenRig);
    h.state_mut()
        .dispatch(Msg::Settings(SettingsMsg::RigFocus(RigRow::Ballistic)));
    // Each step is sent from the rig's value, so the next waits for it to come back.
    for want in [Wire::Type1, Wire::Type2, Wire::Vu, Wire::Momentary] {
        h.key_press(Key::ArrowRight);
        step_until(&mut h, want.word(), |a| {
            a.state
                .daemon()
                .is_some_and(|s| s.rig.levels.ballistic == want)
        });
    }
    h.key_press(Key::Escape);
    step_until(&mut h, "Settings closed", |a| {
        a.state.overlay == Overlay::None
    });
    // The pair's two true-peak bars, then its one LUFS bar.
    step_until(&mut h, "one LUFS bar of the pair", |a| {
        a.state.programme_pane(pane).is_some_and(|(p, _)| {
            matches!(p.bars.as_slice(), [ProgrammeBar::Peak { .. }, ProgrammeBar::Peak { .. },
                ProgrammeBar::Source(b)] if b.name == "Main" && b.reading.is_some())
        })
    });
    let st = &h.state().state;
    assert_eq!(
        st.pane_title(pane),
        "Mom loudness · target \u{2212}23 LUFS · Main · 1 s"
    );
    let texts = programme_texts(&h);
    for want in ["target \u{2212}23", "Int", "Mom", "Short"] {
        assert!(
            texts.iter().any(|t| t == want),
            "{want}: {}",
            common::scene_texts_excerpt(&texts, common::TEXTS_SHOWN)
        );
    }
    assert!(
        texts
            .iter()
            .any(|t| t.ends_with(" LUFS") && t.starts_with('\u{2212}')),
        "the bar's reading: {}",
        common::scene_texts_excerpt(&texts, common::TEXTS_SHOWN)
    );
    // Every input's own reading is its true peak.
    let (p, _) = st.programme_pane(pane).expect("the pane");
    assert!(p.inputs.iter().all(|i| i.unit(p.ballistic) == "dBTP"));
    let boxes = |h: &Harness<'_, App>| plate_boxes(h, pane);
    let b = boxes(&h);
    assert_eq!(b.len(), 1, "the loudness plate's box");
    assert_eq!(b[0].kind, ac2_scene::plate::PlateKind::Loudness);
    // Its room is the whole pane (inside the plot's inset), docked or not.
    let (a, r) = (b[0].area, b[0].rect);
    assert_eq!(
        (a.x + a.w + a.x, a.y + a.h + a.y),
        (900.0, 450.0),
        "the pane is its room: {a:?}"
    );
    assert!(r.x >= a.x && r.y >= a.y && r.x + r.w <= a.x + a.w && r.y + r.h <= a.y + a.h);
    use ac2_scene::view::PlotChrome;
    let chrome = |c| move |a: &App| a.state.kind_modes(PaneKind::Programme).chrome == c;
    h.key_press(Key::T);
    step_until(&mut h, "no grid", chrome(PlotChrome::NoGrid));
    assert_eq!(boxes(&h).len(), 1, "no grid keeps the plate");
    h.key_press(Key::T);
    step_until(&mut h, "traces only", chrome(PlotChrome::Bare));
    assert!(boxes(&h).is_empty(), "the third step hides it");
    assert!(!programme_texts(&h).iter().any(|t| t == "Int"));
    h.key_press(Key::T);
    step_until(&mut h, "full", chrome(PlotChrome::Full));
    assert_eq!(boxes(&h).len(), 1);
}

/// The plates Programme pane `pane`'s scene offers the mouse at the test's size.
fn plate_boxes(
    h: &Harness<'_, App>,
    pane: ac2_ui::state::PaneId,
) -> Vec<ac2_scene::plate::PlateBox> {
    ac2_ui::scenes::programme(
        &h.state().state,
        pane,
        &ac2_scene::theme::Theme::dark(),
        ac2_scene::primitives::Viewport {
            width: 900.0,
            height: 450.0,
        },
    )
    .expect("a Programme scene")
    .plates()
}

/// Two mics as a pair with the rig on Short loudness: a mic has no loudness source, so its
/// Inputs row reads each side's true peak on a bar line of its own, with the pair's
/// correlation under them; P opens the goniometer and the meters, the meters true-peak bars
/// alone: no LUFS column, no loudness plate.
#[test]
fn a_mic_pair_on_short_loudness_reads_true_peak() {
    use ac2_proto::model::{Ballistic, CurveChoice, InputKind, InputSetup, InputSetups, Pair};
    use ac2_scene::programme::{ProgrammeBar, row_block};
    use ac2_scene::view::ProgrammeMode as M;
    if !have_gpu("a_mic_pair_on_short_loudness_reads_true_peak") {
        return;
    }
    let rig = common::Rig::start();
    {
        let mut s = rig.fake.lock();
        let mut inputs = s.state.inputs.clone();
        inputs.retain(|i| i.channel != 2 && i.channel != 3);
        inputs.extend([2, 3].map(|channel| InputSetup {
            channel,
            kind: InputKind::Mic {
                mic: None,
                curve: CurveChoice::Off,
            },
            pair: None,
        }));
        inputs.sort_by_key(|i| i.channel);
        s.commit(ac2_proto::Change::Inputs(InputSetups {
            inputs,
            pairs: vec![Pair {
                left: 2,
                right: 3,
                name: Some("Mics".into()),
            }],
        }));
        let mut r = s.state.rig.clone();
        r.levels.ballistic = Ballistic::ShortTerm;
        s.commit(ac2_proto::Change::Rig(r));
        s.meters = true;
    }
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "the pair's levels on Short", |a| {
        a.state
            .daemon()
            .is_some_and(|s| s.rig.levels.ballistic == Ballistic::ShortTerm)
            && a.state.row_meters(2).is_some_and(|m| {
                let p = a.state.programme_of(m);
                p.pair.is_some() && p.inputs.iter().all(|i| i.max.is_some())
            })
    });
    let st = &h.state().state;
    let p = st.programme_of(st.row_meters(2).expect("the pair's row"));
    let block = row_block(
        &p.inputs,
        p.ballistic,
        p.pair.as_ref().map(|x| x.correlation),
    );
    let sides: Vec<_> = block.sides.iter().map(|s| s.mark).collect();
    assert_eq!(sides, ["L", "R"]);
    for s in &block.sides {
        assert!(s.reading.text.contains("dBTP"), "{:?}", s.reading.text);
    }
    assert!(block.correlation.is_some(), "the pair's correlation");

    h.state_mut().dispatch(ac2_ui::state::Msg::InputPick(2));
    h.key_press(Key::P);
    step_until(&mut h, "two Programme panes", |a| {
        common::visible(&a.state)
            .iter()
            .filter(|k| **k == PaneKind::Programme)
            .count()
            == 2
    });
    let st = &h.state().state;
    let pane = st.layout.focus;
    let views: Vec<_> = st
        .visible_panes()
        .into_iter()
        .filter(|p| st.layout.kind(*p) == PaneKind::Programme)
        .map(|p| st.layout.view(p).expect("view").modes.programme)
        .collect();
    assert_eq!(views, [M::Goniometer, M::Meters]);
    let (p, _) = st.programme_pane(pane).expect("the meters");
    assert!(
        p.bars
            .iter()
            .all(|b| matches!(b, ProgrammeBar::Peak { .. })),
        "true-peak bars only: {:?}",
        p.bars
    );
    assert_eq!(p.loudness, None, "no loudness plate");
    let texts = programme_texts(&h);
    assert!(
        !texts.iter().any(|t| t.contains("LUFS")),
        "no LUFS column: {}",
        common::scene_texts_excerpt(&texts, common::TEXTS_SHOWN)
    );
    assert!(plate_boxes(&h, pane).is_empty(), "no plate to drag");
}
