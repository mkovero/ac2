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

/// The Traces list: every stored trace by name with what it is, its slot, shown or hidden
/// (the dot, a click on it toggles) and the selected one (a click on the row): slot 1 of
/// Main L, slot 2 of Delay tower and a target imported from a file, all stored by the
/// daemon.
#[test]
fn traces_list() -> R {
    if !have_gpu("traces_list") {
        return Ok(());
    }
    let rig = Rig::start()?;
    let mut h = harness(options(&rig));
    step_until(&mut h, "live frames", live);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num1);
    step_until(&mut h, "slot 1", |a| a.state.slots()[0].is_some());
    tab_to(&mut h, DELAY_TOWER);
    drop_on_kind(&mut h, PaneKind::Transfer, DELAY_TOWER);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num2);
    step_until(&mut h, "slot 2", |a| a.state.slots()[1].is_some());
    // Z, then the chooser's first row: a target curve from a file.
    h.key_press(Key::Z);
    step_until(&mut h, "target chooser", |a| {
        matches!(a.state.overlay, Overlay::Choose(_))
    });
    h.key_press(Key::Enter);
    step_until(&mut h, "target prompt", |a| {
        matches!(a.state.overlay, Overlay::Prompt(_))
    });
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ac2-traces/tests/fixtures/house_curve.txt");
    h.event(Event::Text(path.to_string_lossy().into_owned()));
    h.key_press(Key::Enter);
    step_until(&mut h, "three stored traces", |a| {
        a.state.rig.traces.len() == 3
    });
    // The dot hides slot 2 (the daemon keeps it hidden); a click on the target's row
    // selects it.
    h.get_by_label("Hide Delay tower S2").click();
    step_until(&mut h, "slot 2 hidden", |a| {
        a.state.slots()[1].is_some_and(|t| !t.edit.visible)
    });
    h.get_by_label("house_curve\ntarget").click();
    step_until(&mut h, "target selected", |a| {
        a.state
            .selected_trace_meta()
            .is_some_and(|t| t.edit.name == "house_curve")
    });
    assert_eq!(
        h.state()
            .state
            .pane_caption(crate::common::pane(&h.state().state, PaneKind::Transfer))
            .as_deref(),
        Some("house_curve")
    );
    // The hidden trace's dot now offers to show it.
    settle(&mut h, 2);
    assert!(h.query_by_label("Show Delay tower S2").is_some());
    Ok(())
}

/// A drag with the primary button from `from` to `to`, in steps.
fn drag(h: &mut Harness<'_, App>, from: egui::Pos2, to: egui::Pos2) {
    let button = |pos, pressed| Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    h.event(Event::PointerMoved(from));
    settle(h, 2);
    h.event(button(from, true));
    settle(h, 2);
    for k in 1..=6 {
        h.event(Event::PointerMoved(from + (to - from) * (k as f32 / 6.0)));
        settle(h, 1);
    }
    h.event(button(to, false));
    settle(h, 3);
}

/// Two clicks at `at` in quick succession.
fn double_click(h: &mut Harness<'_, App>, at: egui::Pos2) {
    let button = |pressed| Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    h.event(Event::PointerMoved(at));
    settle(h, 2);
    // The harness gives each queued event a frame of a quarter second, longer than egui's
    // 0.3 s between the clicks of a double click: both clicks go into one frame's input.
    for _ in 0..2 {
        h.input_mut().events.push(button(true));
        h.input_mut().events.push(button(false));
    }
    settle(h, 3);
}

/// Many curves: thirty target curves imported one after another, filed under Main L (on the
/// transfer pane), fill its legend. At most 70 % of the pane tall, the rest scroll with the
/// wheel over it. A drag moves it, a drag on its grip enlarges its text, a double-click puts
/// it back, the palette snaps it to a corner and hides it, and the prefs keep all that.
#[test]
fn transfer_legend_many_curves() -> R {
    use ac2_proto::model::TraceOwner;
    use ac2_scene::legend::LegendCorner;
    use ac2_scene::plate::{PlateHover, PlateKind, PlatePlace};
    if !have_gpu("transfer_legend_many_curves") {
        return Ok(());
    }
    let rig = Rig::start()?;
    let mut h = harness(options(&rig));
    step_until(&mut h, "live frames", live);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ac2-traces/tests/fixtures/house_curve.txt");
    const CURVES: usize = 30;
    for n in 1..=CURVES {
        // Z, then the chooser's first row: a target curve from a file.
        h.key_press(Key::Z);
        step_until(&mut h, "target chooser", |a| {
            matches!(a.state.overlay, Overlay::Choose(_))
        });
        h.key_press(Key::Enter);
        step_until(&mut h, "target prompt", |a| {
            matches!(a.state.overlay, Overlay::Prompt(_))
        });
        h.event(Event::Text(path.to_string_lossy().into_owned()));
        h.key_press(Key::Enter);
        step_until(&mut h, "one more curve", |a| {
            a.state.rig.traces.len() == n && a.state.overlay == Overlay::None
        });
    }
    // Filed under the transfer measurement on screen (as the list's Move to does): under
    // Imported no pane would draw them.
    let main_l = TraceOwner::Meas { meas: MAIN_L };
    for t in rig.cli.state().ok_or("state")?.traces.iter() {
        let mut edit = t.edit.clone();
        edit.owner = main_l;
        rig.cli.call(Command::TraceUpdate { trace: t.id, edit })?;
    }
    step_until(&mut h, "every curve under Main L", |a| {
        a.state.rig.traces.len() == CURVES
            && a.state
                .rig
                .traces
                .values()
                .all(|(d, _)| d.meta.edit.owner == main_l)
            && a.state.pane_shows_stored(a.state.layout.focus)
    });
    // The transfer pane alone.
    h.key_press(Key::W);
    step_until(&mut h, "maximised", |a| a.state.layout.maximized);
    settle(&mut h, 3);
    let id = h.state().state.layout.focus;
    assert_eq!(h.state().state.layout.focus_kind(), PaneKind::Transfer);
    let place = |a: &App| a.state.layout.view(id).map(|v| v.plates.legend);
    let rects = |h: &Harness<'_, App>| h.state().plate_rects(id, PlateKind::Legend);
    let (plate, _) = rects(&h).ok_or("legend drawn")?;
    let pane = h
        .state()
        .pane_rects
        .iter()
        .find(|(p, _)| *p == id)
        .map(|(_, r)| *r)
        .ok_or("the pane")?;
    assert!(
        plate.height() <= 0.7 * pane.height() + 1.0,
        "{plate:?} in {pane:?}"
    );
    let hover = |h: &Harness<'_, App>| h.state().state.pane_local.plate_hover.map(|(_, p)| p);

    // The wheel over it scrolls its rows (the frequency axis stays).
    let freq = h.state().state.view.freq;
    h.event(Event::PointerMoved(plate.center()));
    settle(&mut h, 3);
    assert_eq!(
        hover(&h),
        Some(PlateHover {
            kind: PlateKind::Legend,
            grip: false
        })
    );
    for _ in 0..2 {
        h.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, -1.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        });
        settle(&mut h, 20);
    }
    step_until(&mut h, "scrolled", |a| a.state.view.legend.first > 0);
    assert_eq!(h.state().state.view.freq, freq);
    h.event(Event::PointerMoved(egui::pos2(900.0, 700.0)));
    settle(&mut h, 3);
    assert_eq!(hover(&h), None);

    // Dragged right and down: off its corner, the cursor not placed by the click.
    drag(
        &mut h,
        plate.center(),
        plate.center() + egui::vec2(400.0, 120.0),
    );
    step_until(&mut h, "moved", |a| {
        place(a).is_some_and(|p| p.at.is_some_and(|[x, y]| x > 0.3 && y > 0.1))
    });
    assert_eq!(h.state().state.view.cursor_hz, None);
    // Its grip dragged out: larger text.
    let (_, grip) = rects(&h).ok_or("legend drawn")?;
    drag(
        &mut h,
        grip.center(),
        grip.center() + egui::vec2(150.0, 60.0),
    );
    step_until(&mut h, "larger", |a| {
        place(a).is_some_and(|p| p.scale > 1.2)
    });
    // Double-clicked: back in its corner at the plot's text size.
    let (plate, _) = rects(&h).ok_or("legend drawn")?;
    double_click(&mut h, plate.center());
    step_until(&mut h, "put back", |a| {
        place(a) == Some(PlatePlace::default())
    });
    assert_eq!(h.state().state.view.cursor_hz, None);

    // Snapped to a corner from the palette, in the light theme.
    palette(&mut h, "legend bottom-right");
    assert_eq!(
        place(h.state()).and_then(|p| p.at),
        Some(LegendCorner::BottomRight.position())
    );
    palette(&mut h, "theme");
    step_until(&mut h, "light", |a| a.state.theme == ThemeName::Light);

    // Hidden, and kept so; the pane keeps its place.
    palette(&mut h, "hide legend");
    assert!(h.state().state.view.legend.hidden);
    assert!(rects(&h).is_none());
    assert!(h.state().state.prefs.legend.hidden);
    let kept = h.state().state.prefs.layout.panes.clone().ok_or("panes")?;
    let v = kept.views.iter().find(|v| v.id == id).ok_or("pane kept")?;
    assert_eq!(v.plates.legend.at, Some([1.0, 1.0]));
    Ok(())
}
