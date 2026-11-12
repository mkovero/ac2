//! The Programme pane and the Inputs list's programme readouts, against the daemon's own
//! meters of the simulated rig's inputs.

use super::*;
use crate::harness::R;
use ac2_proto::model::{
    Ballistic, CurveChoice, GeneratorDesired, GeneratorSettings, InputKind, InputSetup, Pair,
    Signal,
};
use ac2_proto::units::{Dbfs, Hz};
use ac2_scene::programme::loudness::LoudnessReadout;

/// A steady 1 kHz sine on out 1 at `level` dBFS (0 dBFS: a full-scale sine), playing.
fn tone(level: f64) -> GeneratorDesired {
    GeneratorDesired {
        settings: GeneratorSettings {
            signal: Signal::Sine { freq: Hz(1000.0) },
            level: Dbfs(level),
            band: None,
            outputs: vec![0],
        },
        armed: true,
        firing: true,
    }
}

fn mic(channel: u16) -> InputSetup {
    InputSetup {
        channel,
        kind: InputKind::Mic {
            mic: None,
            curve: CurveChoice::Off,
        },
        pair: None,
    }
}

fn line(channel: u16) -> InputSetup {
    InputSetup {
        channel,
        kind: InputKind::Line,
        pair: None,
    }
}

/// In 2 a mic; in 3 + 4 the line pair `Main`, its right side (nothing patched to in 4)
/// silent.
fn pair_rig_on(rig: &Rig) -> R {
    rig.call(Command::SessionInputs {
        inputs: vec![mic(1), line(2), line(3)],
        pairs: vec![Pair {
            left: 2,
            right: 3,
            name: Some("Main".into()),
        }],
    })?;
    Ok(())
}

fn pair_rig() -> R<Rig> {
    let rig = Rig::start()?;
    pair_rig_on(&rig)?;
    Ok(rig)
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
    let st = &h.state().state;
    let scene = ac2_ui::scenes::programme(
        st,
        st.layout.focus,
        &ac2_scene::theme::Theme::dark(),
        ac2_scene::primitives::Viewport::new(900.0, 450.0),
    )
    .expect("a Programme scene");
    crate::harness::scene_texts(&scene.scene)
}

/// The readout of Programme pane `pane`'s loudness plate, if it has one.
fn loudness(a: &App, pane: ac2_ui::state::PaneId) -> Option<LoudnessReadout> {
    a.state.programme_pane(pane)?.0.loudness
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
        ac2_scene::primitives::Viewport::new(900.0, 450.0),
    )
    .expect("a Programme scene")
    .plates()
}

/// The number of a `max −12.8 dBTP` readout.
fn max_value(text: &str) -> Option<f64> {
    text.strip_prefix("max ")?
        .strip_suffix(" dBTP")?
        .replace('\u{2212}', "-")
        .parse()
        .ok()
}

/// R on a picked row resets that input's max: the next frame holds the level since. The
/// tone steps 20 dB down before the reset, so the max held after it is the quieter tone's,
/// far under the one before.
#[test]
fn r_resets_the_picked_inputs_max() -> R {
    if !have_gpu("r_resets_the_picked_inputs_max") {
        return Ok(());
    }
    let rig = pair_rig()?;
    let mut h = harness(options(&rig));
    let input = |a: &App| {
        a.state
            .row_meters(1)
            .map(|m| a.state.programme_of(m))
            .and_then(|p| p.inputs.first().map(|i| (i.max_text(p.ballistic), i.ppm)))
    };
    let max_of = |a: &App| input(a).and_then(|(m, _)| max_value(&m));
    let lease = rig.cli.lease(tone(-10.0))?;
    step_until(&mut h, "in 2's max", |a| {
        max_of(a).is_some_and(|m| m > -20.0)
    });
    let loud = max_of(h.state()).ok_or("no max")?;
    rig.cli.block_on(lease.set(tone(-30.0)))?;
    step_until(&mut h, "in 2 reading the quieter tone", |a| {
        input(a).is_some_and(|(_, ppm)| ppm.is_some_and(|p| p < loud as f32 - 18.0))
    });
    assert_eq!(max_of(h.state()), Some(loud), "the max holds until reset");
    h.state_mut().dispatch(ac2_ui::state::Msg::InputPick(1));
    h.key_press(Key::R);
    step_until(&mut h, "the max reset", |a| {
        max_of(a).is_some_and(|m| m < loud - 15.0)
    });
    assert!(
        h.state()
            .state
            .toasts
            .list
            .iter()
            .any(|t| t.text == "max, overs and loudness reset on input 2"),
    );
    rig.cli.block_on(lease.end())?;
    Ok(())
}

/// A mic input's pane has no loudness plate; the mic beside a pair's inputs leaves one
/// source, so that pane has the pair's; R's toast names what it resets.
#[test]
fn loudness_plate_only_for_one_source() -> R {
    if !have_gpu("loudness_plate_only_for_one_source") {
        return Ok(());
    }
    let rig = pair_rig()?;
    let mut h = harness(options(&rig));
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
    drop_input(&mut h, 1);
    step_until(&mut h, "a Programme pane", |a| {
        crate::common::visible(&a.state).contains(&PaneKind::Programme)
    });
    let pane = crate::common::pane(&h.state().state, PaneKind::Programme);
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
    Ok(())
}

/// The simulated rig with in 3 a hot line: the room path to it 18 dB up and noiseless, so
/// the tone at the system's maximum drives it past full scale and silence reads −∞.
fn hot_line_rig() -> R<std::sync::Arc<dyn ac2_audio::Backend>> {
    use ac2_audio::fake::{FakeConfig, FakeDrive, FakePath, Pace};
    const LOOP: u32 = 32;
    let names = |n: &[&str]| Some(n.iter().map(|s| (*s).to_owned()).collect());
    let backend = ac2_audio::FakeBackend::new(FakeConfig {
        drive: FakeDrive::Thread(Pace::Realtime),
        paths: vec![
            FakePath::loopback(0, 0, LOOP),
            FakePath::acoustic(0, 1, LOOP + 240, vec![0.5], 1e-4),
            FakePath::acoustic(0, 2, LOOP + 480, vec![8.0], 0.0),
        ],
        input_names: names(&["Loop return", "Room mic", "Hot line", "Line 4"]),
        output_names: names(&["Out 1 (speaker + loop)", "Out 2"]),
        ..FakeConfig::default()
    })?;
    Ok(std::sync::Arc::new(backend))
}

/// Only the bars move in the Inputs list: the pair row's readout lines, its correlation and
/// everything under them keep their rects whatever the levels read: silence, a level over
/// the over threshold, CLIP, and silence with CLIP and the overs still held.
#[test]
fn inputs_rows_keep_their_geometry() -> R {
    use egui::accesskit::Role;
    use egui_kittest::kittest::{By, Queryable as _};
    if !have_gpu("inputs_rows_keep_their_geometry") {
        return Ok(());
    }
    let backend = hot_line_rig()?;
    let rig = Rig::start_with(move |c| c.backends = vec![backend])?;
    pair_rig_on(&rig)?;
    let mut h = harness(options(&rig));
    // Every label of the sidebar: the rows' names, readouts and the lists under them (not
    // the top bar's, which shortens its texts for the other client's stimulus).
    let rects = |h: &Harness<'_, App>| {
        let mut r: Vec<[i32; 4]> = h
            .query_all(By::new().predicate(|n| n.role() == Role::Label))
            .map(|n| n.rect())
            .filter(|r| r.max.x <= 240.0 && r.min.y > 34.0)
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
    let mut check = |h: &mut Harness<'_, App>, what: &str| {
        h.step();
        let got = rects(h);
        let want = first.get_or_insert_with(|| got.clone());
        assert_eq!(*want, got, "{what}");
    };
    step_until(&mut h, "silence", |a| {
        left(a).is_some_and(|i| i.overs == 0 && !i.clipped && i.ppm.is_none_or(|p| !p.is_finite()))
    });
    check(&mut h, "silence");
    // The threshold at its floor, −20 dBTP: a tone at −35 dBFS (a sine's level, its peak)
    // peaks at −17 dBFS on in 3, over it and far from full scale; one at −10 at +8 dBFS.
    let rev = rig.cli.state().map(|s| s.rig.rev).ok_or("no state")?;
    rig.call(Command::LevelsSet {
        base: rev,
        ballistic: None,
        correlation_window: None,
        over_threshold: Some(ac2_proto::units::Dbtp(-20.0)),
        correlation_gate: None,
        loudness_target: None,
    })?;
    let lease = rig.cli.lease(tone(-35.0))?;
    step_until(&mut h, "overs, no clip", |a| {
        left(a).is_some_and(|i| i.overs > 0 && !i.clipped && i.ppm.is_some_and(|p| p.is_finite()))
    });
    check(&mut h, "overs");
    rig.cli.block_on(lease.set(tone(-10.0)))?;
    step_until(&mut h, "CLIP", |a| left(a).is_some_and(|i| i.clipped));
    check(&mut h, "CLIP");
    rig.cli.block_on(lease.end())?;
    step_until(&mut h, "silence, CLIP held", |a| {
        left(a).is_some_and(|i| i.clipped && i.overs > 0 && i.ppm.is_none_or(|p| p < -60.0))
    });
    check(&mut h, "silence after CLIP");
    Ok(())
}

/// From the pair: Mom loudness chosen on Settings › Rig monitor turns the pair's two peak
/// bars into one LUFS bar of the pair, titled so, with the target; the loudness plate is a
/// plate the mouse can take over the whole pane; T's third step takes it (and its box) away
/// and the next brings both back.
#[test]
fn mom_loudness_reads_lufs_bars() -> R {
    use ac2_proto::model::Ballistic as Wire;
    use ac2_scene::programme::ProgrammeBar;
    use ac2_ui::settings::RigRow;
    use ac2_ui::state::{Msg, SettingsMsg};
    if !have_gpu("mom_loudness_reads_lufs_bars") {
        return Ok(());
    }
    let rig = pair_rig()?;
    let mut h = harness(options(&rig));
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
    // The pair opens on its goniometer: G steps it to the meters.
    drop_input(&mut h, 2);
    h.key_press(Key::G);
    h.step();
    step_until(&mut h, "a Programme pane", |a| {
        crate::common::visible(&a.state).contains(&PaneKind::Programme)
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
    let shown = crate::common::scene_texts_excerpt(&texts, crate::common::TEXTS_SHOWN);
    for want in ["target \u{2212}23", "Int", "Mom", "Short"] {
        assert!(texts.iter().any(|t| t == want), "{want}: {shown}");
    }
    assert!(
        texts
            .iter()
            .any(|t| t.ends_with(" LUFS") && t.starts_with('\u{2212}')),
        "the bar's reading: {shown}"
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
    Ok(())
}

/// Two mics as a pair with the rig on Short loudness: a mic has no loudness source, so its
/// Inputs row reads each side's true peak on a bar line of its own, with the pair's
/// correlation under them; P opens the goniometer and the meters, the meters true-peak bars
/// alone: no LUFS column, no loudness plate.
#[test]
fn a_mic_pair_on_short_loudness_reads_true_peak() -> R {
    use ac2_scene::programme::{ProgrammeBar, row_block};
    use ac2_scene::view::ProgrammeMode as M;
    if !have_gpu("a_mic_pair_on_short_loudness_reads_true_peak") {
        return Ok(());
    }
    let rig = Rig::start()?;
    rig.call(Command::SessionInputs {
        inputs: vec![mic(2), mic(3)],
        pairs: vec![Pair {
            left: 2,
            right: 3,
            name: Some("Mics".into()),
        }],
    })?;
    let rev = rig.cli.state().map(|s| s.rig.rev).ok_or("no state")?;
    rig.call(Command::LevelsSet {
        base: rev,
        ballistic: Some(Ballistic::ShortTerm),
        correlation_window: None,
        over_threshold: None,
        correlation_gate: None,
        loudness_target: None,
    })?;
    let mut h = harness(options(&rig));
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
    drop_input(&mut h, 2);
    drop_input(&mut h, 2);
    h.key_press(Key::G);
    h.step();
    step_until(&mut h, "two Programme panes", |a| {
        crate::common::visible(&a.state)
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
        crate::common::scene_texts_excerpt(&texts, crate::common::TEXTS_SHOWN)
    );
    assert!(plate_boxes(&h, pane).is_empty(), "no plate to drag");
    Ok(())
}
