#![allow(clippy::unwrap_used)]

use std::sync::atomic::AtomicBool;

use ac2_proto::model::{
    Marker, MarkerKind, PlanSpec, PositionMean, SimAbsorption, SimRefusal, SimSettings, Surface,
    SurfaceTag, WalkRoom, WalkRoomBand,
};
use ac2_proto::units::{Hz, Seconds, WalkId};
use ac2_roomsim::{BAND_COUNT, Band, MaterialId, SurfaceId};

use super::run::Job;
use super::{Mode, fit, plan, work};

fn tag(surface: Surface, material: &str) -> SurfaceTag {
    SurfaceTag {
        surface,
        material: material.into(),
    }
}

fn speaker(name: &str, at: [f64; 3]) -> Marker {
    Marker {
        name: name.into(),
        kind: MarkerKind::Speaker,
        at,
        heading_deg: None,
    }
}

/// A 10 × 7 × 3.5 m room with a carpeted floor and a heavy curtain on the back wall: a
/// decay of about half a second, quick to trace.
pub(crate) fn shoebox() -> PlanSpec {
    PlanSpec {
        name: "Box".into(),
        outline: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 7.0], [0.0, 7.0]],
        ceiling: Some(3.5),
        zones: vec![],
        markers: vec![speaker("Main", [1.0, 3.5, 2.0])],
        surfaces: vec![
            tag(Surface::Floor, "carpet_on_concrete"),
            tag(Surface::Wall { edge: 1 }, "curtain_heavy"),
        ],
    }
}

/// The shoebox panelled in wood on every surface.
pub(crate) fn panelled() -> PlanSpec {
    PlanSpec {
        surfaces: [
            Surface::Floor,
            Surface::Ceiling,
            Surface::Wall { edge: 0 },
            Surface::Wall { edge: 1 },
            Surface::Wall { edge: 2 },
            Surface::Wall { edge: 3 },
        ]
        .into_iter()
        .map(|s| tag(s, "wood_panel"))
        .collect(),
        ..shoebox()
    }
}

fn l_shape() -> PlanSpec {
    PlanSpec {
        name: "Bar".into(),
        outline: vec![
            [0.0, 0.0],
            [12.0, 0.0],
            [12.0, 5.0],
            [5.0, 5.0],
            [5.0, 10.0],
            [0.0, 10.0],
        ],
        ceiling: Some(3.0),
        zones: vec![],
        markers: vec![speaker("Corner", [2.0, 2.0, 2.0])],
        surfaces: vec![],
    }
}

#[test]
fn a_shoebox_becomes_a_six_surface_prism_with_its_materials() {
    let p = plan::prism(&shoebox()).unwrap();
    assert_eq!(p.surface_count(), 6);
    assert!((p.volume() - 245.0).abs() < 1e-9);
    let coeff = |s| p.coefficients(s).absorption;
    assert_eq!(
        coeff(SurfaceId::Floor),
        MaterialId::CarpetOnConcrete.coefficients().absorption
    );
    assert_eq!(
        coeff(SurfaceId::Wall { edge: 1 }),
        MaterialId::CurtainHeavy.coefficients().absorption
    );
    // Untagged: the stated defaults.
    let default = |n| MaterialId::from_name(n).unwrap().coefficients().absorption;
    assert_eq!(
        coeff(SurfaceId::Wall { edge: 0 }),
        default(ac2_proto::model::DEFAULT_WALL_MATERIAL)
    );
    assert_eq!(
        coeff(SurfaceId::Ceiling),
        default(ac2_proto::model::DEFAULT_WALL_MATERIAL)
    );
}

#[test]
fn an_l_shaped_plan_keeps_its_eight_surfaces_and_its_concave_corner() {
    let plan = l_shape();
    let p = plan::prism(&plan).unwrap();
    assert_eq!(p.surface_count(), 8);
    assert!((p.volume() - 3.0 * (60.0 + 25.0)).abs() < 1e-9);
    assert_eq!(
        p.coefficients(SurfaceId::Floor).absorption,
        MaterialId::from_name(ac2_proto::model::DEFAULT_FLOOR_MATERIAL)
            .unwrap()
            .coefficients()
            .absorption
    );
    // The cut-away corner is outside: a receiver there is refused by name.
    assert_eq!(
        plan::inside(&p, "P1", [8.0, 8.0, 1.2]),
        Err(SimRefusal::Outside { name: "P1".into() })
    );
    assert!(plan::speaker(&plan, &p, "Corner").is_ok());
    assert_eq!(
        plan::speaker(&plan, &p, "Sub"),
        Err(SimRefusal::NoSpeaker { name: "Sub".into() })
    );
}

#[test]
fn an_outdoor_plan_is_refused() {
    let mut p = l_shape();
    p.ceiling = None;
    assert_eq!(plan::prism(&p).err(), Some(SimRefusal::Outdoor));
}

#[test]
fn an_unknown_material_is_refused_with_its_surface() {
    let mut p = shoebox();
    p.surfaces.push(tag(Surface::Wall { edge: 3 }, "lino"));
    assert_eq!(
        plan::prism(&p).err(),
        Some(SimRefusal::UnknownMaterial {
            surface: Surface::Wall { edge: 3 },
            name: "lino".into(),
        })
    );
}

#[test]
fn a_plan_that_does_not_validate_is_refused_with_the_reason() {
    let mut p = shoebox();
    p.outline.reverse();
    assert!(matches!(plan::prism(&p), Err(SimRefusal::Plan { .. })));
}

pub(crate) fn job(plan: &PlanSpec, rays: u32) -> Job {
    let prism = plan::prism(plan).unwrap();
    let source = plan::speaker(plan, &prism, "Main").unwrap();
    Job {
        prism,
        sources: vec![("Main".into(), source)],
        receivers: vec![
            ("P1".into(), [6.0, 2.0, 1.2]),
            ("P2".into(), [8.0, 5.0, 1.2]),
        ],
        settings: SimSettings {
            image_order: 2,
            rays_per_band: rays,
            budget: Seconds(20.0),
            seed: 1,
        },
    }
}

/// A walk's room whose T20 is `t` per simulator band, at four positions.
fn walk_room(t: &[f64; BAND_COUNT]) -> WalkRoom {
    let none = WalkRoomBand {
        centre: None,
        edt: None,
        t20: None,
        c80: None,
        d50: None,
    };
    WalkRoom {
        positions: 4,
        sources: 1,
        broadband: none.clone(),
        octave: Band::ALL
            .iter()
            .map(|b| WalkRoomBand {
                centre: Some(Hz(b.exact_hz())),
                t20: Some(PositionMean {
                    mean: t[b.index()],
                    min: t[b.index()],
                    max: t[b.index()],
                    n: 4,
                }),
                ..none.clone()
            })
            .collect(),
        third: Vec::new(),
    }
}

#[test]
fn the_measured_t20_is_matched_to_the_simulator_bands_by_centre() {
    let mut room = walk_room(&[1.0, 1.1, 1.2, 1.3, 1.4, 1.5, 1.6]);
    room.octave.remove(0);
    let m = fit::measured_t20(&room);
    assert_eq!(m[0], None);
    assert_eq!(m[3], Some(1.3));
}

#[test]
fn the_fit_converges_to_a_measured_t20() {
    let plan = panelled();
    let j = job(&plan, 6_000);
    let cancel = AtomicBool::new(false);
    // The "measured" room absorbs 1.6 times the table, simulated under another seed; the
    // walk gave no T20 at 125 Hz.
    let truth = 1.6;
    let measured_room = work(
        &Job {
            settings: SimSettings {
                seed: 99,
                ..j.settings
            },
            prism: plan::prism(&plan).unwrap(),
            sources: j.sources.clone(),
            receivers: j.receivers.clone(),
        },
        Mode::Run {
            scale: Some([truth; BAND_COUNT]),
            absorption: SimAbsorption::Assumed,
        },
        &cancel,
        &mut |_| {},
    )
    .unwrap();
    let t = fit::simulated_t20(&measured_room.pass).map(Option::unwrap);
    let mut room = walk_room(&t);
    room.octave.remove(0);
    let measured = fit::measured_t20(&room);
    let mut statuses = Vec::new();
    let done = work(
        &j,
        Mode::Fit {
            walk: WalkId(1),
            positions: 4,
            measured,
        },
        &cancel,
        &mut |s| statuses.push(s),
    )
    .unwrap();
    assert!(!statuses.is_empty(), "progress");
    let SimAbsorption::Fitted {
        walk,
        positions,
        bands,
    } = &done.absorption
    else {
        panic!("{:?}", done.absorption)
    };
    assert_eq!((*walk, *positions), (WalkId(1), 4));
    assert_eq!(bands[0].scale, None, "no measured T20 at 125 Hz: assumed");
    for (b, f) in bands.iter().enumerate().skip(1) {
        let (Some(s), Some(m)) = (f.simulated, f.measured) else {
            panic!("band {b}: {f:?}")
        };
        assert!(f.converged, "band {b}: {f:?}");
        assert!((s.0 / m.0 - 1.0).abs() <= fit::TOLERANCE, "band {b}: {f:?}");
        // One material everywhere: T follows the scale, and the fit finds the room's.
        let scale = f.scale.unwrap();
        assert!((scale / truth - 1.0).abs() < 0.15, "band {b}: {f:?}");
    }
    assert!(done.fitted.is_some());
}

#[test]
fn a_raised_cancel_flag_stops_a_run() {
    let j = job(&shoebox(), 200_000);
    let cancel = AtomicBool::new(true);
    let r = work(
        &j,
        Mode::Run {
            scale: None,
            absorption: SimAbsorption::Assumed,
        },
        &cancel,
        &mut |_| {},
    );
    assert!(
        matches!(r, Err(super::run::SimEnd::Cancelled)),
        "{:?}",
        r.err()
    );
}
