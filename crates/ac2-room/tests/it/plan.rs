use ac2_proto::model::{MarkerKind, PlanSpec, Surface, SurfaceTag};
use ac2_room::plan::{
    PlanError, Polygon, Readout, Reference, WallSide, check_polygon, mirror_axes, readout,
    validate, wall_side,
};

use crate::{hall, marker, rect, zone};

#[test]
fn a_valid_hall_passes() {
    assert_eq!(validate(&hall()), Ok(()));
}

#[test]
fn self_intersection_names_the_edges() {
    let mut p = hall();
    // A bow tie: edge 1 (v1→v2) crosses edge 3 (v3→v4).
    p.outline = vec![[0.0, 0.0], [10.0, 10.0], [10.0, 0.0], [0.0, 10.0]];
    let e = validate(&p).unwrap_err();
    assert_eq!(
        e,
        PlanError::EdgesCross {
            polygon: Polygon::Outline,
            a: 0,
            b: 2
        }
    );
    assert_eq!(e.to_string(), "edge 1 crosses edge 3");
    let shown = PlanError::EdgesCross {
        polygon: Polygon::Outline,
        a: 2,
        b: 5,
    };
    assert_eq!(shown.to_string(), "edge 3 crosses edge 6");
}

#[test]
fn a_vertex_touching_another_edge_is_not_simple() {
    // Vertex 5 sits on edge 1: two lobes joined at a point.
    let poly = vec![
        [0.0, 0.0],
        [10.0, 0.0],
        [10.0, 5.0],
        [6.0, 5.0],
        [5.0, 0.0],
        [0.0, 5.0],
    ];
    assert!(matches!(
        check_polygon(&poly, &Polygon::Outline),
        Err(PlanError::EdgesCross { .. })
    ));
}

#[test]
fn a_zone_error_names_the_zone() {
    let mut p = hall();
    p.zones[0].outline = vec![[2.0, 8.0], [8.0, 14.0], [8.0, 8.0], [2.0, 14.0]];
    assert_eq!(
        validate(&p).unwrap_err().to_string(),
        "zone Stalls: edge 1 crosses edge 3"
    );
}

#[test]
fn a_clockwise_outline_is_refused() {
    let mut p = hall();
    p.outline.reverse();
    assert_eq!(validate(&p), Err(PlanError::Clockwise));
}

#[test]
fn a_clockwise_zone_is_accepted() {
    let mut p = hall();
    p.zones[0].outline.reverse();
    assert_eq!(validate(&p), Ok(()));
}

#[test]
fn degenerate_edges_are_refused() {
    let mut p = hall();
    p.outline = vec![
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 0.0],
        [20.0, 30.0],
        [0.0, 30.0],
    ];
    assert_eq!(
        validate(&p),
        Err(PlanError::ZeroLengthEdge {
            polygon: Polygon::Outline,
            edge: 1
        })
    );
    p.outline = vec![[0.0, 0.0], [10.0, 0.0], [20.0, 0.0]];
    assert_eq!(
        validate(&p),
        Err(PlanError::ZeroArea {
            polygon: Polygon::Outline
        })
    );
    // A spike: edge 2 runs out and edge 3 folds straight back along it.
    p.outline = vec![
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 40.0],
        [20.0, 30.0],
        [0.0, 30.0],
    ];
    assert!(matches!(validate(&p), Err(PlanError::EdgesCross { .. })));
    p.outline = vec![[0.0, 0.0], [20.0, 0.0]];
    assert!(matches!(
        validate(&p),
        Err(PlanError::TooFewVertices { count: 2, .. })
    ));
    p.outline = vec![[0.0, 0.0], [f64::NAN, 0.0], [0.0, 1.0]];
    assert!(matches!(
        validate(&p),
        Err(PlanError::NonFiniteVertex { vertex: 1, .. })
    ));
}

#[test]
fn an_outdoor_plan_is_valid_and_has_no_height_limit() {
    let mut p = hall();
    p.ceiling = None;
    p.markers = vec![marker(
        "Delay tower",
        MarkerKind::Speaker,
        [10.0, 15.0, 25.0],
        None,
    )];
    assert_eq!(validate(&p), Ok(()));
    p.ceiling = Some(10.0);
    assert!(matches!(
        validate(&p),
        Err(PlanError::SpeakerAboveCeiling { .. })
    ));
    p.ceiling = Some(0.0);
    assert!(matches!(validate(&p), Err(PlanError::Ceiling { .. })));
}

#[test]
fn zones_must_lie_inside_the_outline() {
    // An L: the notch x > 10, y > 10 is outside.
    let mut p = PlanSpec {
        outline: vec![
            [0.0, 0.0],
            [20.0, 0.0],
            [20.0, 10.0],
            [10.0, 10.0],
            [10.0, 20.0],
            [0.0, 20.0],
        ],
        ..hall()
    };
    p.zones = vec![zone("Bar", rect(12.0, 12.0, 18.0, 18.0))];
    assert!(matches!(
        validate(&p),
        Err(PlanError::ZoneVertexOutside { vertex: 0, .. })
    ));
    // Every vertex inside, but the edge from (18, 5) to (5, 18) cuts across the notch.
    p.zones = vec![zone("Bar", vec![[2.0, 2.0], [18.0, 5.0], [5.0, 18.0]])];
    assert!(matches!(
        validate(&p),
        Err(PlanError::ZoneEdgeOutside { edge: 1, .. })
    ));
    // Touching the walls is inside.
    p.zones = vec![zone("Floor", p.outline.clone())];
    assert_eq!(validate(&p), Ok(()));
}

#[test]
fn markers_follow_their_kind() {
    let mut p = hall();
    p.markers = vec![marker(
        "FOH",
        MarkerKind::Foh,
        [10.0, 40.0, 0.0],
        Some(-90.0),
    )];
    assert_eq!(validate(&p), Ok(()), "FOH may be outside (a truck)");
    p.markers = vec![marker("Side", MarkerKind::Point, [-5.0, 10.0, 0.0], None)];
    assert_eq!(validate(&p), Ok(()), "a point may be outside");
    p.markers = vec![marker(
        "Main L",
        MarkerKind::Speaker,
        [-1.0, 1.0, 5.0],
        None,
    )];
    assert_eq!(
        validate(&p).unwrap_err().to_string(),
        "speaker Main L lies outside the outline"
    );
    p.markers = vec![marker("Mic", MarkerKind::Point, [5.0, 10.0, -0.1], None)];
    assert!(matches!(
        validate(&p),
        Err(PlanError::MarkerBelowFloor { .. })
    ));
    p.markers = vec![marker(
        "Mic",
        MarkerKind::Point,
        [5.0, 10.0, 1.0],
        Some(f64::NAN),
    )];
    assert!(matches!(
        validate(&p),
        Err(PlanError::MarkerNonFinite { .. })
    ));
}

#[test]
fn zone_numbers_are_checked() {
    let mut p = hall();
    p.zones[0].ear_height = 10.0;
    assert!(matches!(validate(&p), Err(PlanError::EarHeight { .. })));
    p.zones[0].ear_height = 1.2;
    p.zones[0].weight = Some(-1.0);
    assert!(matches!(validate(&p), Err(PlanError::Weight { .. })));
}

#[test]
fn surface_tags_name_real_surfaces_once() {
    let tag = |surface| SurfaceTag {
        surface,
        material: "brick".into(),
    };
    let mut p = hall();
    p.surfaces = vec![tag(Surface::Wall { edge: 3 }), tag(Surface::Floor)];
    assert_eq!(validate(&p), Ok(()));
    p.surfaces = vec![tag(Surface::Wall { edge: 4 })];
    assert_eq!(
        validate(&p).unwrap_err().to_string(),
        "wall tag for edge 5: the outline has 4 edges"
    );
    p.surfaces = vec![tag(Surface::Floor), tag(Surface::Floor)];
    assert!(matches!(validate(&p), Err(PlanError::SurfaceTwice { .. })));
    p.surfaces = vec![tag(Surface::Ceiling)];
    p.ceiling = None;
    assert_eq!(validate(&p), Err(PlanError::SurfaceCeilingOutdoors));
}

fn axis_angles(p: &PlanSpec) -> Vec<f64> {
    let mut a: Vec<f64> = mirror_axes(p)
        .iter()
        .map(|ax| ax.dir[1].atan2(ax.dir[0]).to_degrees().round())
        .collect();
    a.sort_by(f64::total_cmp);
    a
}

#[test]
fn mirror_axes_of_room_and_speakers() {
    let mut p = hall();
    assert_eq!(
        axis_angles(&p),
        vec![0.0, 90.0],
        "an empty rectangle has two"
    );
    p.markers = vec![marker(
        "Centre",
        MarkerKind::Speaker,
        [10.0, 1.0, 6.0],
        None,
    )];
    assert_eq!(
        axis_angles(&p),
        vec![90.0],
        "a centred speaker keeps the long axis"
    );
    let ax = mirror_axes(&p)[0];
    assert!((ax.point[0] - 10.0).abs() < 1e-9);
    p.markers = vec![
        marker("L", MarkerKind::Speaker, [3.0, 1.0, 6.0], None),
        marker("R", MarkerKind::Speaker, [17.0, 1.0, 6.0], None),
        marker("FOH", MarkerKind::Foh, [4.0, 20.0, 0.0], None),
    ];
    assert_eq!(
        axis_angles(&p),
        vec![90.0],
        "a L/R pair; FOH does not count"
    );
    p.markers[1].at[2] = 7.0;
    assert!(
        mirror_axes(&p).is_empty(),
        "the pair hangs at different heights"
    );
    p.markers = vec![marker("Off", MarkerKind::Speaker, [4.0, 1.0, 6.0], None)];
    assert!(mirror_axes(&p).is_empty());
}

#[test]
fn mirror_axes_ignore_collinear_vertices_and_find_diagonals() {
    let mut p = hall();
    p.outline = vec![
        [0.0, 0.0],
        [10.0, 0.0],
        [20.0, 0.0],
        [20.0, 30.0],
        [0.0, 30.0],
    ];
    assert_eq!(axis_angles(&p), vec![0.0, 90.0]);
    // An equal-armed L is symmetric about its diagonal only.
    p.outline = vec![
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 10.0],
        [10.0, 10.0],
        [10.0, 20.0],
        [0.0, 20.0],
    ];
    assert_eq!(axis_angles(&p), vec![45.0]);
}

#[test]
fn readout_from_foh_facing_the_stage() {
    let mut p = hall();
    p.markers = vec![
        marker("Main", MarkerKind::Speaker, [10.0, 1.0, 6.0], Some(90.0)),
        marker("FOH", MarkerKind::Foh, [10.0, 20.0, 0.0], Some(-90.0)),
    ];
    // Facing the stage (−y), left is +x and back is +y.
    let r = readout(&p, [12.1, 24.0]);
    assert_eq!(r.to_string(), "2.1 m left, 4.0 m back from FOH");
    match r {
        Readout::Relative { left, back, from } => {
            assert!((left - 2.1).abs() < 1e-9 && (back - 4.0).abs() < 1e-9);
            assert_eq!(from, Reference::Foh);
        }
        Readout::Plan { .. } => panic!("FOH has a heading"),
    }
    assert_eq!(
        readout(&p, [7.0, 18.0]).to_string(),
        "3.0 m right, 2.0 m forward from FOH"
    );
    assert_eq!(
        readout(&p, [10.02, 19.97]).to_string(),
        "0.0 m left, 0.0 m back from FOH"
    );
}

#[test]
fn readout_falls_back_to_a_speaker_then_plan() {
    let mut p = hall();
    p.markers = vec![
        marker("FOH", MarkerKind::Foh, [10.0, 20.0, 0.0], None),
        marker("Main", MarkerKind::Speaker, [10.0, 1.0, 6.0], Some(90.0)),
    ];
    assert_eq!(
        readout(&p, [12.0, 5.0]).to_string(),
        "2.0 m left, 4.0 m back from Main"
    );
    p.markers[1].heading_deg = None;
    let r = readout(&p, [12.0, 5.0]);
    assert_eq!(r, Readout::Plan { x: 12.0, y: 5.0 });
    assert_eq!(
        r.to_string(),
        "x 12.0 m, y 5.0 m in plan (no FOH or speaker heading)"
    );
}

#[test]
fn walls_are_named_by_side_facing_the_stage() {
    let mut p = hall();
    p.outline = vec![[0.0, 0.0], [20.0, 0.0], [20.0, 30.0], [0.0, 30.0]];
    p.markers = vec![marker(
        "FOH",
        MarkerKind::Foh,
        [10.0, 20.0, 0.0],
        Some(-90.0),
    )];
    // Facing −y: the wall at y = 0 is ahead, +x is left.
    let sides: Vec<_> = (0..4).map(|e| wall_side(&p, e)).collect();
    assert_eq!(
        sides,
        [
            Some(WallSide::Stage),
            Some(WallSide::Left),
            Some(WallSide::Back),
            Some(WallSide::Right)
        ]
    );
    assert_eq!(wall_side(&p, 4), None);
    // A speaker at the stage pointing at the audience reads the same.
    p.markers = vec![marker(
        "Main",
        MarkerKind::Speaker,
        [10.0, 1.0, 6.0],
        Some(90.0),
    )];
    assert_eq!(wall_side(&p, 2), Some(WallSide::Back));
    p.markers.clear();
    assert_eq!(wall_side(&p, 0), None);
}
