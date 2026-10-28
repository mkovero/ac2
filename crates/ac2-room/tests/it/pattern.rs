use ac2_proto::model::{MarkerKind, PlanSpec};
use ac2_room::pattern::{
    Broken, Preset, PresetError, PresetKind, Relaxed, SuggestError, Suggestion, breaks,
    min_spacing, mirrored_pairs, quotas, suggest,
};
use ac2_room::plan::{contains, mirror_axes, wall_distance};

use crate::{hall, marker, rect, zone};

/// Every rule holds on the suggestion, or `relaxed` names it with what holds instead.
fn assert_rules_or_relaxed(plan: &PlanSpec, preset: &Preset, s: &Suggestion) {
    let got = min_spacing(&s.points);
    match s.relaxed.iter().find_map(|r| match r {
        Relaxed::Spacing { got, .. } => Some(*got),
        _ => None,
    }) {
        Some(named) => assert!((named - got).abs() < 1e-9 && got < preset.spacing),
        None => assert!(got >= preset.spacing - 1e-9, "spacing {got} not named"),
    }
    for p in &s.points {
        let z = &plan.zones[p.zone];
        assert!(
            contains(&z.outline, [p.at[0], p.at[1]]),
            "{p:?} left {}",
            z.name
        );
        assert_eq!(p.at[2], z.ear_height);
        let wall = s
            .relaxed
            .iter()
            .find_map(|r| match r {
                Relaxed::WallDistance { got, zone, .. } if *zone == z.name => Some(*got),
                _ => None,
            })
            .unwrap_or(preset.wall_distance);
        assert!(
            wall_distance(plan, [p.at[0], p.at[1]]) >= wall - 1e-9,
            "{p:?} closer than {wall} m to a wall"
        );
        if let Some(d) = preset.speaker_distance {
            let keep = s
                .relaxed
                .iter()
                .find_map(|r| match r {
                    Relaxed::SpeakerDistance { got, zone, .. } if *zone == z.name => Some(*got),
                    _ => None,
                })
                .unwrap_or(d);
            for m in plan
                .markers
                .iter()
                .filter(|m| m.kind == MarkerKind::Speaker)
            {
                let r = ((m.at[0] - p.at[0]).powi(2)
                    + (m.at[1] - p.at[1]).powi(2)
                    + (m.at[2] - p.at[2]).powi(2))
                .sqrt();
                assert!(r >= keep - 1e-9, "{p:?} {r} m from {}", m.name);
            }
        }
    }
    let pairs = if preset.no_mirrored_pairs {
        mirrored_pairs(&s.points, &mirror_axes(plan))
    } else {
        0
    };
    let named = s.relaxed.iter().find_map(|r| match r {
        Relaxed::MirroredPairs { pairs } => Some(*pairs),
        _ => None,
    });
    assert_eq!(named.unwrap_or(0), pairs);
}

#[test]
fn the_same_seed_gives_the_same_suggestion() {
    let p = hall();
    let t = Preset::tuning();
    let a = suggest(&p, &[0], 8, &t, 7).unwrap();
    assert_eq!(a, suggest(&p, &[0], 8, &t, 7).unwrap());
    assert_ne!(
        a,
        suggest(&p, &[0], 8, &t, 8).unwrap(),
        "a reshuffle moves them"
    );
}

#[test]
fn tuning_rules_hold_over_many_seeds() {
    let mut p = hall();
    p.markers = vec![
        marker("L", MarkerKind::Speaker, [3.0, 1.0, 6.0], None),
        marker("R", MarkerKind::Speaker, [17.0, 1.0, 6.0], None),
    ];
    let t = Preset::tuning();
    for seed in 0..10 {
        let s = suggest(&p, &[0], 8, &t, seed).unwrap();
        assert_eq!(s.points.len(), 8);
        assert!(s.relaxed.is_empty(), "seed {seed}: {:?}", s.relaxed);
        assert_rules_or_relaxed(&p, &t, &s);
    }
}

#[test]
fn iso3382_keeps_the_source_distance() {
    let mut p = hall();
    p.markers = vec![marker(
        "Source",
        MarkerKind::Speaker,
        [10.0, 2.0, 1.5],
        None,
    )];
    // V = 6000 m³, T = 1.5 s: d_min = 2·√(6000 / (343 · 1.5)) ≈ 6.83 m.
    let iso = Preset::iso3382(&p, 1.5).unwrap();
    let d = iso.speaker_distance.unwrap();
    assert!((d - 2.0 * (6000.0f64 / (343.0 * 1.5)).sqrt()).abs() < 1e-12);
    for seed in 0..5 {
        let s = suggest(&p, &[0], 8, &iso, seed).unwrap();
        assert!(s.relaxed.is_empty(), "{:?}", s.relaxed);
        assert_rules_or_relaxed(&p, &iso, &s);
    }
}

#[test]
fn iso3382_distance_in_a_large_hall_is_about_eleven_metres() {
    let mut p = hall();
    p.outline = rect(0.0, 0.0, 25.0, 40.0);
    p.zones = vec![zone("All", rect(0.0, 0.0, 25.0, 40.0))];
    let iso = Preset::iso3382(&p, 1.0).unwrap();
    assert!((iso.speaker_distance.unwrap() - 10.8).abs() < 0.05);
    p.ceiling = None;
    assert_eq!(Preset::iso3382(&p, 1.0), Err(PresetError::Outdoors));
    p.ceiling = Some(10.0);
    assert_eq!(Preset::iso3382(&p, 0.0), Err(PresetError::ReverbTime(0.0)));
}

#[test]
fn default_counts_follow_the_table() {
    let mut p = hall();
    assert_eq!(Preset::tuning().default_n(&p, &[0]), Some(8));
    let iso = Preset::iso3382(&p, 1.0).unwrap();
    assert_eq!(iso.kind, PresetKind::Iso3382);
    assert_eq!(iso.default_n(&p, &[0]), None, "seats unset: ask");
    for (seats, n) in [(300, 6), (500, 6), (900, 8), (2000, 10), (4000, 10)] {
        p.zones[0].seats = Some(seats);
        assert_eq!(iso.default_n(&p, &[0]), Some(n), "{seats} seats");
    }
}

#[test]
fn a_symmetric_rectangle_with_a_centred_speaker_gets_no_mirrored_pair() {
    let mut p = hall();
    p.markers = vec![marker(
        "Centre",
        MarkerKind::Speaker,
        [10.0, 1.0, 6.0],
        None,
    )];
    let axes = mirror_axes(&p);
    assert_eq!(axes.len(), 1);
    let t = Preset::tuning();
    for n in [2, 4, 6, 8] {
        for seed in 0..10 {
            let s = suggest(&p, &[0], n, &t, seed).unwrap();
            assert_eq!(mirrored_pairs(&s.points, &axes), 0, "n {n} seed {seed}");
            assert!(s.relaxed.is_empty(), "n {n} seed {seed}: {:?}", s.relaxed);
        }
    }
    // Without the rule, Lloyd's even spread puts mirrored pairs there.
    let free = Preset {
        no_mirrored_pairs: false,
        ..t
    };
    let any = (0..10).any(|seed| {
        let s = suggest(&p, &[0], 8, &free, seed).unwrap();
        mirrored_pairs(&s.points, &axes) > 0
    });
    assert!(any, "the test room never produces a mirrored pair");
}

#[test]
fn concave_and_l_shaped_zones_keep_every_point_inside() {
    let l = vec![
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 8.0],
        [8.0, 8.0],
        [8.0, 20.0],
        [0.0, 20.0],
    ];
    let p = PlanSpec {
        outline: l.clone(),
        zones: vec![zone("Bar", l)],
        ..hall()
    };
    // A U-shaped zone in the hall: its centroid lies in the gap.
    let mut u = hall();
    u.zones = vec![zone(
        "Horseshoe",
        vec![
            [1.0, 6.0],
            [19.0, 6.0],
            [19.0, 29.0],
            [13.0, 29.0],
            [13.0, 12.0],
            [7.0, 12.0],
            [7.0, 29.0],
            [1.0, 29.0],
        ],
    )];
    let t = Preset::tuning();
    for plan in [&p, &u] {
        for n in [1, 3, 8, 12] {
            for seed in 0..5 {
                let s = suggest(plan, &[0], n, &t, seed).unwrap();
                assert_eq!(s.points.len(), n);
                assert_rules_or_relaxed(plan, &t, &s);
            }
        }
    }
}

#[test]
fn weights_split_n_as_stated() {
    assert_eq!(quotas(&[3.0, 1.0], 8), vec![6, 2]);
    assert_eq!(quotas(&[1.0, 1.0, 1.0], 8), vec![3, 3, 2]);
    assert_eq!(quotas(&[100.0, 1.0], 4), vec![3, 1], "at least one each");
    assert_eq!(
        quotas(&[5.0, 0.0, 1.0], 6),
        vec![5, 0, 1],
        "weight 0 gets none"
    );
    assert_eq!(
        quotas(&[1.0, 3.0, 2.0], 2),
        vec![0, 1, 1],
        "n too small: heaviest"
    );

    // Default weight is area: 20 × 24 stalls against a 20 × 4 balcony strip.
    let mut p = hall();
    p.zones.push(zone("Balcony", rect(0.0, 26.0, 20.0, 30.0)));
    p.zones[0].outline = rect(0.0, 2.0, 20.0, 26.0);
    p.zones[1].ear_height = 4.0;
    let count = |s: &Suggestion, z: usize| s.points.iter().filter(|q| q.zone == z).count();
    let s = suggest(&p, &[0, 1], 7, &Preset::tuning(), 3).unwrap();
    assert_eq!((count(&s, 0), count(&s, 1)), (6, 1));
    p.zones[0].weight = Some(1.0);
    p.zones[1].weight = Some(1.0);
    let s = suggest(&p, &[0, 1], 8, &Preset::tuning(), 3).unwrap();
    assert_eq!((count(&s, 0), count(&s, 1)), (4, 4));
    let s = suggest(&p, &[1], 3, &Preset::tuning(), 3).unwrap();
    assert!(s.points.iter().all(|q| q.zone == 1 && q.at[2] == 4.0));
}

#[test]
fn spacing_is_relaxed_before_wall_distance() {
    // 10 m × 10 m: the area 1 m from the walls is 8 m × 8 m, room for 25 points at 2 m.
    let mut p = hall();
    p.outline = rect(0.0, 0.0, 10.0, 10.0);
    p.zones = vec![zone("Room", rect(0.0, 0.0, 10.0, 10.0))];
    let t = Preset::tuning();
    let s = suggest(&p, &[0], 40, &t, 1).unwrap();
    assert_eq!(s.relaxed.len(), 1, "{:?}", s.relaxed);
    let Relaxed::Spacing { got, wanted, n } = s.relaxed[0] else {
        panic!("{:?}", s.relaxed);
    };
    assert_eq!((wanted, n), (2.0, 40));
    assert!(got > 0.8 && got < 2.0, "{got}");
    assert_eq!(
        s.relaxed[0].to_string(),
        format!("spacing {got:.1} m: 40 positions do not fit at 2.0 m")
    );
    assert_rules_or_relaxed(&p, &t, &s);
}

#[test]
fn wall_distance_is_relaxed_where_a_zone_has_no_room() {
    // A 1.6 m corridor: no point is 1 m from both walls; the best keeps 0.8 m.
    let mut p = hall();
    p.outline = rect(0.0, 0.0, 30.0, 1.6);
    p.zones = vec![zone("Corridor", rect(0.0, 0.0, 30.0, 1.6))];
    let t = Preset::tuning();
    let s = suggest(&p, &[0], 30, &t, 2).unwrap();
    let kinds: Vec<&str> = s
        .relaxed
        .iter()
        .map(|r| match r {
            Relaxed::Spacing { .. } => "spacing",
            Relaxed::WallDistance { .. } => "wall",
            Relaxed::FloorDistance { .. } => "floor",
            Relaxed::SpeakerDistance { .. } => "speaker",
            Relaxed::MirroredPairs { .. } => "mirror",
        })
        .collect();
    assert_eq!(kinds, vec!["spacing", "wall"], "the order is fixed");
    let Relaxed::WallDistance { got, .. } = &s.relaxed[1] else {
        unreachable!()
    };
    assert!(*got > 0.75 && *got <= 0.8, "{got}");
    assert_rules_or_relaxed(&p, &t, &s);
    // Few enough to fit at 2 m: only the wall distance is relaxed.
    let s = suggest(&p, &[0], 5, &t, 2).unwrap();
    assert!(matches!(
        s.relaxed.as_slice(),
        [Relaxed::WallDistance { .. }]
    ));
    assert_rules_or_relaxed(&p, &t, &s);
}

#[test]
fn speaker_distance_is_relaxed_last() {
    let mut p = hall();
    p.markers = vec![marker(
        "Source",
        MarkerKind::Speaker,
        [10.0, 10.0, 1.2],
        None,
    )];
    let iso = Preset {
        speaker_distance: Some(40.0),
        ..Preset::iso3382(&p, 1.0).unwrap()
    };
    let s = suggest(&p, &[0], 4, &iso, 0).unwrap();
    assert!(
        s.relaxed
            .iter()
            .any(|r| matches!(r, Relaxed::SpeakerDistance { .. })),
        "{:?}",
        s.relaxed
    );
    assert!(
        !s.relaxed
            .iter()
            .any(|r| matches!(r, Relaxed::WallDistance { .. })),
        "the wall distance still fits"
    );
    assert_rules_or_relaxed(&p, &iso, &s);
}

#[test]
fn bad_requests_are_refused() {
    let p = hall();
    let t = Preset::tuning();
    assert_eq!(suggest(&p, &[], 8, &t, 0), Err(SuggestError::NoZones));
    assert_eq!(suggest(&p, &[1], 8, &t, 0), Err(SuggestError::ZoneIndex(1)));
    assert_eq!(
        suggest(&p, &[0, 0], 8, &t, 0),
        Err(SuggestError::ZoneTwice(0))
    );
    assert_eq!(suggest(&p, &[0], 0, &t, 0), Err(SuggestError::NoPositions));
    let mut q = hall();
    q.zones[0].weight = Some(0.0);
    assert_eq!(suggest(&q, &[0], 8, &t, 0), Err(SuggestError::NoWeight));
    q.outline.reverse();
    assert!(matches!(
        suggest(&q, &[0], 8, &t, 0),
        Err(SuggestError::Plan(_))
    ));
}

#[test]
fn iso3382_keeps_the_floor_distance_or_names_the_zone() {
    let mut p = hall();
    p.zones
        .push(zone("Floor seats", rect(2.0, 20.0, 8.0, 28.0)));
    p.zones[0].outline = rect(0.0, 6.0, 20.0, 18.0);
    p.zones[1].ear_height = 0.8;
    let iso = Preset::iso3382(&p, 1.5).unwrap();
    assert_eq!(iso.floor_distance, Some(1.0));
    assert_eq!(Preset::tuning().floor_distance, None);
    let s = suggest(&p, &[0, 1], 6, &iso, 3).unwrap();
    let floor: Vec<&Relaxed> = s
        .relaxed
        .iter()
        .filter(|r| matches!(r, Relaxed::FloorDistance { .. }))
        .collect();
    assert_eq!(
        floor,
        vec![&Relaxed::FloorDistance {
            wanted: 1.0,
            got: 0.8,
            zone: "Floor seats".into()
        }]
    );
    assert_eq!(
        floor[0].to_string(),
        "floor distance 0.8 m: the ear height of zone Floor seats is under 1.0 m"
    );
    // The points stand at the zone's ear height still.
    assert!(
        s.points
            .iter()
            .filter(|q| q.zone == 1)
            .all(|q| q.at[2] == 0.8)
    );
    // Tuning has no floor rule; a zone given no positions is not named.
    let t = suggest(&p, &[0, 1], 6, &Preset::tuning(), 3).unwrap();
    assert!(
        !t.relaxed
            .iter()
            .any(|r| matches!(r, Relaxed::FloorDistance { .. }))
    );
    let s = suggest(&p, &[0], 4, &iso, 3).unwrap();
    assert!(
        !s.relaxed
            .iter()
            .any(|r| matches!(r, Relaxed::FloorDistance { .. }))
    );
}

/// An operator's edits are checked rule by rule: a point dragged next to another breaks the
/// spacing (and names it), one by the wall the wall distance, one out of the audience lies in
/// no zone, a suggestion left as it is breaks nothing.
#[test]
fn edited_points_are_marked_with_the_rules_they_break() {
    let plan = hall();
    let preset = Preset::tuning();
    let s = suggest(&plan, &[0], 6, &preset, 3).unwrap();
    let mut pts: Vec<[f64; 3]> = s.points.iter().map(|p| p.at).collect();
    assert!(breaks(&plan, &pts, &preset).iter().all(Vec::is_empty));

    pts[1] = [pts[0][0] + 0.5, pts[0][1], pts[0][2]];
    pts.push([0.4, 20.0, 1.2]);
    pts.push([10.0, 2.0, 1.2]);
    let b = breaks(&plan, &pts, &preset);
    assert!(
        b[1].iter()
            .any(|r| matches!(r, Broken::Spacing { other: 0, .. }))
    );
    let wall = &b[pts.len() - 2];
    assert!(
        wall.iter()
            .any(|r| matches!(r, Broken::Wall { got, .. } if (*got - 0.4).abs() < 1e-9)),
        "{wall:?}"
    );
    assert_eq!(
        wall.iter()
            .find(|r| matches!(r, Broken::Wall { .. }))
            .unwrap()
            .text(|i| format!("P{}", i + 1)),
        "0.4 m from a wall (rule 1.0 m)"
    );
    assert!(b[pts.len() - 1].contains(&Broken::OutsideZones));
    assert_eq!(
        b[1].iter()
            .find(|r| matches!(r, Broken::Spacing { .. }))
            .unwrap()
            .text(|i| format!("P{}", i + 1)),
        "0.5 m from P1 (spacing 2.0 m)"
    );
}
