#![allow(clippy::unwrap_used)]

use std::sync::atomic::AtomicBool;

use ac2_proto::model::{
    PlanSpec, SimAbsorption, SimMap, SimMapRequest, SimRefusal, SimSettings, SimStatus, ZoneSpec,
};
use ac2_proto::units::{Hz, Seconds};

use super::super::run::Job;
use super::super::tests::shoebox;
use super::super::{Mode, plan, work};
use super::*;

/// The shoebox with its floor from x = 2 m to the back a zone at ear height.
fn seated() -> PlanSpec {
    PlanSpec {
        zones: vec![ZoneSpec {
            name: "Floor".into(),
            outline: vec![[2.0, 0.5], [9.0, 0.5], [9.0, 6.5], [2.0, 6.5]],
            ear_height: 1.2,
            weight: None,
            seats: None,
        }],
        ..shoebox()
    }
}

fn mapped(plan_spec: &PlanSpec, spacing: f64, band: Option<Hz>) -> SimMap {
    let prism = plan::prism(plan_spec).unwrap();
    let source = plan::speaker(plan_spec, &prism, "Main").unwrap();
    let grid = grid(plan_spec, &prism, spacing).unwrap();
    let job = Job {
        prism,
        sources: vec![("Main".into(), source)],
        receivers: Vec::new(),
        settings: SimSettings {
            image_order: 2,
            rays_per_band: 0,
            ..SimSettings::default()
        },
    };
    let mut statuses = Vec::new();
    let done = work(
        &job,
        Mode::Map {
            spec: Box::new(MapSpec {
                grid,
                band: band.and_then(band_index),
                band_hz: band,
                window: Seconds(0.05),
                image_order: 2,
            }),
            scale: None,
            absorption: SimAbsorption::Assumed,
        },
        &AtomicBool::new(false),
        &mut |s| statuses.push(s),
    )
    .unwrap();
    assert!(
        matches!(statuses.last(), Some(SimStatus::Running { done, total, .. }) if done == total),
        "{statuses:?}"
    );
    assert!(done.pass.schroeder_hz > 0.0);
    done.map.unwrap()
}

#[test]
fn the_grid_covers_the_zones_only() {
    let p = seated();
    let prism = plan::prism(&p).unwrap();
    let g = grid(&p, &prism, 1.0).unwrap();
    // 7 × 6 m of zone in 1 m cells: 7 columns by 6 rows.
    assert_eq!(g.origin, [2.5, 1.0]);
    assert_eq!(g.cells.len(), 7 * 6);
    for &(col, row, at) in &g.cells {
        assert!(
            at[0] > 2.0 && at[0] < 9.0 && at[1] > 0.5 && at[1] < 6.5,
            "{at:?}"
        );
        assert_eq!(at[2], 1.2);
        assert_eq!(at[0], 2.5 + f64::from(col));
        assert_eq!(at[1], 1.0 + f64::from(row));
    }
}

#[test]
fn direct_energy_falls_with_distance() {
    let m = mapped(&seated(), 1.0, Some(Hz(1000.0)));
    assert_eq!(m.cells.len(), 42);
    assert_eq!(m.band, Some(Hz(1000.0)));
    let db = |col: u16, row: u16| {
        m.cells
            .iter()
            .find(|c| c.col == col && c.row == row)
            .unwrap()
            .db
    };
    // The speaker stands at (1, 3.5): along the room's middle row (y = 3.0, row 2) the
    // level falls cell by cell away from it, and the loudest cell is the nearest.
    for col in 0..6 {
        assert!(db(col, 2) > db(col + 1, 2), "col {col}: {}", db(col, 2));
    }
    assert!(m.cells.iter().any(|c| c.db == 0.0));
    assert!(
        m.cells
            .iter()
            .all(|c| c.db <= 0.0 && c.db >= SimMap::FLOOR_DB)
    );
    // Near the source the direct sound dominates: from 2.5 m to 8.5 m the level drops by a
    // good part of the inverse square law's 12.6 dB, less what the reflections fill in.
    let drop = db(0, 2) - db(6, 2);
    assert!(drop > 4.0 && drop < 12.6, "{drop}");
    // Broadband sums the bands and maps the same cells.
    let b = mapped(&seated(), 1.0, None);
    assert_eq!(b.cells.len(), m.cells.len());
    assert_eq!(b.band, None);
}

#[test]
fn a_map_without_zones_or_too_fine_is_refused() {
    let bare = shoebox();
    let prism = plan::prism(&bare).unwrap();
    assert_eq!(grid(&bare, &prism, 1.0), Err(SimRefusal::NoZones));
    let p = seated();
    let prism = plan::prism(&p).unwrap();
    // 42 m² at 0.1 m: 4200 cells, over the 4096 a map holds.
    match grid(&p, &prism, 0.1) {
        Err(SimRefusal::MapTooFine { cells, max }) => {
            assert!((4100..=4300).contains(&cells), "{cells}");
            assert_eq!(max as usize, SimMapRequest::MAX_CELLS);
        }
        other => panic!("{other:?}"),
    }
    // Far too fine is refused from the zone area alone.
    assert!(matches!(
        grid(&p, &prism, 0.001),
        Err(SimRefusal::MapTooFine { .. })
    ));
    // Coarser than the zone: no cell centre in it.
    assert_eq!(grid(&p, &prism, 50.0), Err(SimRefusal::NoZones));
    assert_eq!(band_index(Hz(1000.0)), Some(3));
    assert_eq!(band_index(Hz(1001.5)), None);
}
