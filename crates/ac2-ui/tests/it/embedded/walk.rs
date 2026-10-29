//! A spatial walk from an empty daemon through the app alone: the transfer measurement made
//! from the empty session, Shift+W, a room plan drawn in the plan editor, the setup window's
//! suggestion, the walk pane, two points captured and the walk's average made.

use super::harness::{Driver, NAME, R, measure_from_empty, scene_texts};
use ac2_proto::model::PointState;
use ac2_scene::primitives::Viewport;
use ac2_scene::theme::Theme;
use ac2_ui::embedded::{EmbeddedBackend, Setup, start_embedded_with};
use ac2_ui::state::{AppState, Msg, Overlay, PaneKind, StimPhase};
use ac2_ui::walk_setup::PlanMsg;

fn captured(s: &AppState) -> usize {
    s.walks().first().map_or(0, |w| {
        w.points
            .iter()
            .filter(|p| matches!(p.state, PointState::Captured { .. }))
            .count()
    })
}

fn click(d: &mut Driver, at: [f64; 2]) {
    d.send(Msg::Plan(PlanMsg::Press { at, reach_m: 0.3 }));
    d.send(Msg::Plan(PlanMsg::Release));
}

fn walk_texts(s: &AppState) -> Vec<String> {
    ac2_ui::scenes::walk(
        s,
        s.layout.focus,
        &Theme::dark(),
        Viewport {
            width: 900.0,
            height: 600.0,
        },
        ac2_ui::scenes::Now {
            instant: std::time::Instant::now(),
            wall: ac2_proto::units::WallNs(1_793_275_200_000_000_000),
        },
    )
    .map(|w| scene_texts(&w.scene))
    .unwrap_or_default()
}

#[test]
fn spatial_walk_from_an_empty_daemon() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let panes = d.st.layout.views.len();

    // Shift+W: New walk… first; no plan in the library, so its plan row is New plan….
    d.key("Shift+W");
    assert!(
        matches!(&d.st.overlay, Overlay::Choose(_)),
        "{:?}",
        d.st.overlay
    );
    d.key("Enter");
    let Overlay::WalkSetup(s) = &d.st.overlay else {
        return Err(format!("{:?}", d.st.overlay).into());
    };
    assert_eq!(s.plan, None);
    d.key("Enter");
    assert!(
        matches!(&d.st.overlay, Overlay::PlanEditor(_)),
        "{:?}",
        d.st.overlay
    );

    // The floor typed, a vertex per row; a zone and the FOH clicked on the plan.
    d.send(Msg::Text("0 0".into()));
    for v in ["14 0", "14 10", "0 10"] {
        d.key("Insert");
        d.send(Msg::Text(v.into()));
    }
    for _ in 0..4 {
        d.key("ArrowUp");
    }
    d.key("Insert");
    for at in [[2.0, 2.0], [12.0, 2.0], [12.0, 8.0], [2.0, 8.0]] {
        click(&mut d, at);
    }
    // From the zone's last vertex up past its rows, ear height and name to the editing row.
    for _ in 0..6 {
        d.key("ArrowUp");
    }
    d.key("ArrowRight");
    click(&mut d, [7.0, 1.0]);
    let Overlay::PlanEditor(e) = &d.st.overlay else {
        return Err(format!("{:?}", d.st.overlay).into());
    };
    assert_eq!(e.verdict(), Ok(()), "{:?}", e.plan);
    assert_eq!((e.plan.zones.len(), e.plan.markers.len()), (1, 1));
    d.key("Enter");
    d.until("the plan in the library", |s| {
        s.daemon()
            .is_some_and(|x| x.plans.iter().any(|p| p.name == "Room 1"))
    })?;

    // Back in the setup on its zones row: the new plan, three positions typed, Enter.
    let Overlay::WalkSetup(s) = &d.st.overlay else {
        return Err(format!("{:?}", d.st.overlay).into());
    };
    assert_eq!(s.plan.as_deref(), Some("Room 1"));
    // From the zones row past the preset to the count.
    for _ in 0..2 {
        d.key("ArrowDown");
    }
    d.send(Msg::Text("3".into()));
    d.key("ArrowDown");
    let Overlay::WalkSetup(s) = &d.st.overlay else {
        return Err(format!("{:?}", d.st.overlay).into());
    };
    assert_eq!(s.points.len(), 3, "{:?}", s.error);
    d.key("Enter");
    d.until("the walk pane", |s| {
        s.layout.focus_kind() == PaneKind::Walk && !s.walks().is_empty()
    })?;
    assert_eq!(d.st.overlay, Overlay::None);
    assert_eq!(d.st.layout.views.len(), panes + 1, "a pane of its own");
    let texts = walk_texts(&d.st);
    assert!(
        texts.iter().any(|t| t.starts_with("Move mic to P1 · ")),
        "{texts:?}"
    );
    assert!(
        texts.iter().any(|t| t == "NO AVERAGE · 0 OF 3 POSITIONS"),
        "{texts:?}"
    );

    // Two points captured with the stimulus on: the average is made at the second.
    // The level typed for the first measurement stays: Space arms, Enter fires.
    d.key("Space");
    d.until("armed", |s| s.rig.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|x| x.generator.firing))?;
    d.key("X");
    d.until("P1 captured", |s| captured(s) == 1)?;
    d.until("on to P2", |s| {
        s.walks().first().and_then(|w| w.step) == Some(1)
    })?;
    d.key("X");
    d.until("the average", |s| {
        captured(s) == 2
            && s.walks()
                .first()
                .and_then(|w| w.average)
                .is_some_and(|m| s.meas(m).is_some())
    })?;
    let texts = walk_texts(&d.st);
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("Walk 1 · average · 2 positions")),
        "{texts:?}"
    );
    let at: Vec<String> =
        d.st.trace_list()
            .iter()
            .filter_map(|t| t.position.as_ref().map(|p| p.label.clone()))
            .collect();
    assert_eq!(at, ["P1", "P2"]);
    d.stop()
}
