use super::*;
use ac2_proto::frame::{IrFrame, IrMeta};
use ac2_proto::model::{Marker, MarkerKind, SimArrival, SimKind, SimMap};
use ac2_proto::samples::{plan, sim_state};
use ac2_proto::units::{Hz, MeasId, Seconds, WalkId};

use crate::banner::Status;
use crate::primitives::Viewport;
use crate::view::{Style, ViewState};

/// A 20 × 30 m room, FOH at (10, 20) facing the stage at y = 0.
fn room() -> PlanSpec {
    PlanSpec {
        outline: vec![[0.0, 0.0], [20.0, 0.0], [20.0, 30.0], [0.0, 30.0]],
        markers: vec![Marker {
            name: "FOH".into(),
            kind: MarkerKind::Foh,
            at: [10.0, 20.0, 0.0],
            heading_deg: Some(-90.0),
        }],
        ..plan()
    }
}

fn at(t: f64, ratio: f32, path: Vec<Surface>) -> SimArrival {
    SimArrival {
        time: Seconds(t),
        energy: vec![0.01 * ratio, 0.01 * ratio],
        path,
    }
}

/// The sample fit, its receiver P1 with a direct sound at 37.5 ms, the back wall 10 ms
/// later at −3 dB, wall 2 then the ceiling 20 ms later at −25 dB and a −40 dB path.
fn sim() -> SimState {
    let mut s = sim_state();
    s.sources[0].receivers[0].arrivals = vec![
        at(0.0375, 1.0, vec![]),
        at(0.0475, 0.5, vec![Surface::Wall { edge: 2 }]),
        at(
            0.0575,
            0.003_162,
            vec![Surface::Wall { edge: 1 }, Surface::Ceiling],
        ),
        at(0.0675, 0.0001, vec![Surface::Floor]),
    ];
    s
}

fn position() -> TracePosition {
    TracePosition {
        label: "P1".into(),
        at: [5.0, 12.0, 1.2],
        plan: "Hall".into(),
        walk: WalkId(2),
    }
}

#[test]
fn surfaces_are_named_in_plan_terms() {
    let p = room();
    let name = |s| surface_name(Some(&p), s);
    assert_eq!(name(Surface::Wall { edge: 0 }), "stage wall");
    assert_eq!(name(Surface::Wall { edge: 1 }), "left wall");
    assert_eq!(name(Surface::Wall { edge: 2 }), "back wall");
    assert_eq!(name(Surface::Wall { edge: 3 }), "right wall");
    assert_eq!(name(Surface::Floor), "floor");
    assert_eq!(name(Surface::Ceiling), "ceiling");
    assert_eq!(
        path_name(Some(&p), &[Surface::Wall { edge: 2 }, Surface::Ceiling]),
        "back wall → ceiling"
    );
    assert_eq!(path_name(Some(&p), &[]), "direct sound");
    // Without a heading: by number, 1-based as the vertex table counts edges.
    let mut bare = room();
    bare.markers.clear();
    assert_eq!(
        surface_name(Some(&bare), Surface::Wall { edge: 2 }),
        "wall 3"
    );
    assert_eq!(surface_name(None, Surface::Wall { edge: 0 }), "wall 1");
    // Two walls on one side (a notch in the back wall): numbered, with the side.
    let mut notched = room();
    notched.outline = vec![
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 30.0],
        [10.0, 30.0],
        [10.0, 28.0],
        [0.0, 28.0],
    ];
    assert_eq!(
        surface_name(Some(&notched), Surface::Wall { edge: 2 }),
        "wall 3, back"
    );
    assert_eq!(
        surface_name(Some(&notched), Surface::Wall { edge: 4 }),
        "wall 5, back"
    );
    assert_eq!(
        surface_name(Some(&notched), Surface::Wall { edge: 3 }),
        "wall 4, right"
    );
}

#[test]
fn marks_are_aligned_on_the_direct_sound() {
    let p = room();
    let m = reflection_marks(&sim(), Some(&p), &position()).expect("heard at P1");
    let got: Vec<(i64, i64, &str)> = m
        .marks
        .iter()
        .map(|k| {
            (
                (k.t_ms * 1000.0).round() as i64,
                (k.level_db * 10.0).round() as i64,
                k.name.as_str(),
            )
        })
        .collect();
    // The −40 dB floor path is under MARK_FLOOR_DB: not marked.
    assert_eq!(
        got,
        [
            (10_000, -30, "back wall"),
            (20_000, -250, "left wall → ceiling")
        ]
    );
    assert_eq!(
        m.legend,
        "simulated · aligned on direct sound · absorption fitted to 4 positions · assumed at \
         1k Hz · Main L"
    );
}

#[test]
fn only_a_finished_simulation_heard_at_the_trace_marks_it() {
    let pos = position();
    let mut s = sim();
    s.status = ac2_proto::model::SimStatus::Done;
    // Another walk's P1, elsewhere: not this point.
    let elsewhere = TracePosition {
        walk: WalkId(9),
        at: [1.0, 1.0, 1.2],
        ..pos.clone()
    };
    assert!(reflection_marks(&s, None, &elsewhere).is_none());
    // Given points (no walk) at the same place: matched by label and place.
    let mut points = s.clone();
    points.walk = None;
    points.kind = SimKind::Run;
    assert!(reflection_marks(&points, None, &pos).is_some());
    let mut other_plan = s.clone();
    other_plan.plan = "Club".into();
    assert!(reflection_marks(&other_plan, None, &pos).is_none());
    let mut running = s.clone();
    running.status = ac2_proto::samples::sim_running().status;
    assert!(reflection_marks(&running, None, &pos).is_none());
    let mut map = s;
    map.map = Some(SimMap {
        spacing: 1.0,
        origin: [0.0, 0.0],
        band: None,
        window: Seconds(0.05),
        cells: Vec::new(),
    });
    assert!(reflection_marks(&map, None, &pos).is_none());
}

#[test]
fn the_overlay_draws_dashed_marks_names_and_the_legend() {
    let theme = crate::theme::Theme::dark();
    let frame = IrFrame {
        meas: MeasId(1),
        meta: IrMeta {
            sample_rate: Hz(48_000.0),
            t0: Seconds(-0.005),
            dt: Seconds(0.001),
            inserted_delay: Seconds(0.0),
        },
        linear: (0..60).map(|i| if i == 5 { 1.0 } else { 0.01 }).collect(),
        etc: None,
    };
    let view = ViewState::default();
    let mut sc = crate::ir::ir_scene(
        &frame,
        theme.traces[0],
        None,
        &crate::ir::IrCtx {
            status: &Status::default(),
            view: &view,
            axes: &view.ir.axes,
            style: Style {
                chrome: view.chrome,
                theme: &theme,
            },
            size: Viewport {
                width: 800.0,
                height: 300.0,
            },
        },
    );
    let before = sc.scene.layers[2].polylines.len();
    let p = room();
    let marks = reflection_marks(&sim(), Some(&p), &position()).unwrap();
    draw_marks(&mut sc, marks, &theme);
    let layer = &sc.scene.layers[2];
    let lines = &layer.polylines[before..];
    assert_eq!(lines.len(), 2);
    let x10 = sc.x_axis.mapping.to_px(10.0);
    assert!((lines[0].points[0][0] - x10).abs() < 1e-3);
    assert!(lines.iter().all(|l| l.stroke.dash.is_some()));
    let texts: Vec<&str> = layer.labels.iter().map(|l| l.text.as_str()).collect();
    // −3 dB is named; −25 dB is under LABEL_FLOOR_DB: a line without a name.
    assert!(texts.contains(&"back wall"), "{texts:?}");
    assert!(!texts.contains(&"left wall → ceiling"), "{texts:?}");
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("simulated · aligned on direct sound")),
        "{texts:?}"
    );
    assert!(sc.sim.is_some());
}
