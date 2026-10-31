//! Room simulation samples: requests, a fitted state with one source at one point.

use crate::model::*;
use crate::units::*;

/// A run of the hall's speaker at two points.
pub fn sim_request() -> SimRequest {
    SimRequest {
        plan: "Hall".into(),
        sources: vec!["Main L".into()],
        receivers: SimReceivers::Points {
            points: vec![WalkPointSpec {
                label: "P1".into(),
                at: [5.0, 12.0, 1.2],
            }],
        },
        settings: SimSettings::default(),
    }
}

/// A fit to walk 2 from the hall's speaker.
pub fn sim_fit_request() -> SimFitRequest {
    SimFitRequest {
        walk: WalkId(2),
        source: "Main L".into(),
        settings: SimSettings {
            image_order: 2,
            rays_per_band: 5_000,
            budget: Seconds(2.5),
            seed: 7,
        },
    }
}

/// Two octave bands are enough to show every field's shape.
fn bands() -> Vec<Hz> {
    vec![Hz(500.0), Hz(1000.0)]
}

/// A finished fit: one source, one receiver, a direct sound and a floor reflection.
pub fn sim_state() -> SimState {
    let room = super::traces::room_band;
    SimState {
        id: SimId(3),
        plan: "Hall".into(),
        kind: SimKind::Fit { walk: WalkId(2) },
        walk: Some(WalkId(2)),
        settings: SimSettings::default(),
        status: SimStatus::Done,
        bands: bands(),
        absorption: SimAbsorption::Fitted {
            walk: WalkId(2),
            positions: 4,
            bands: vec![
                SimFitBand {
                    scale: Some(1.25),
                    measured: Some(Seconds(1.5)),
                    simulated: Some(Seconds(1.5)),
                    converged: true,
                },
                SimFitBand {
                    scale: None,
                    measured: None,
                    simulated: Some(Seconds(1.75)),
                    converged: false,
                },
            ],
        },
        sources: vec![SimSource {
            name: "Main L".into(),
            at: [10.0, 1.0, 4.0],
            receivers: vec![SimReceiver {
                label: "P1".into(),
                at: [5.0, 12.0, 1.2],
                arrivals: vec![
                    SimArrival {
                        time: Seconds(0.0375),
                        energy: vec![0.0078125, 0.0078125],
                        path: vec![],
                    },
                    SimArrival {
                        time: Seconds(0.04),
                        energy: vec![0.005, 0.00375],
                        path: vec![Surface::Floor, Surface::Wall { edge: 3 }],
                    },
                ],
                late: SimEchogram {
                    bin: Seconds(0.01),
                    energy: vec![vec![0.0, 0.25, 0.125], vec![0.0, 0.25, 0.0625]],
                },
                room: RoomAcoustics {
                    broadband: room(None),
                    octave: vec![room(Some(Hz(1000.0)))],
                    third: Vec::new(),
                    span_end: Seconds(1.75),
                },
            }],
            room: None,
        }],
        schroeder: Some(Hz(75.0)),
        valid_above: Some(Hz(75.0)),
        eyring: vec![Seconds(1.5), Seconds(1.25)],
        rays_per_band: 20_480,
        budget_exhausted: false,
        lost_rays: 2,
        map: None,
    }
}

/// A run in its ray tracing.
pub fn sim_running() -> SimState {
    SimState {
        id: SimId(4),
        kind: SimKind::Run,
        status: SimStatus::Running {
            stage: SimStage::RayTracing,
            done: 4_096,
            total: 20_480,
            source: 0,
            pass: 0,
        },
        absorption: SimAbsorption::Assumed,
        sources: Vec::new(),
        schroeder: None,
        valid_above: None,
        eyring: Vec::new(),
        rays_per_band: 0,
        lost_rays: 0,
        ..sim_state()
    }
}

/// A coverage map of the hall's speaker at the default spacing, 1 kHz.
pub fn sim_map_request() -> SimMapRequest {
    SimMapRequest {
        plan: "Hall".into(),
        sources: vec!["Main L".into()],
        spacing: SimMapRequest::DEFAULT_SPACING_M,
        band: Some(Hz(1000.0)),
        window: Seconds(SimMapRequest::DEFAULT_WINDOW_S),
        image_order: SimMapRequest::DEFAULT_IMAGE_ORDER,
    }
}

/// A finished coverage map: three cells of a zone.
pub fn sim_map_state() -> SimState {
    SimState {
        id: SimId(5),
        kind: SimKind::Map,
        walk: None,
        absorption: SimAbsorption::Assumed,
        sources: vec![SimSource {
            name: "Main L".into(),
            at: [10.0, 1.0, 4.0],
            receivers: Vec::new(),
            room: None,
        }],
        rays_per_band: 0,
        lost_rays: 0,
        map: Some(SimMap {
            spacing: 1.0,
            origin: [0.5, 4.5],
            band: Some(Hz(1000.0)),
            window: Seconds(0.05),
            cells: vec![
                SimMapCell {
                    col: 0,
                    row: 0,
                    db: 0.0,
                },
                SimMapCell {
                    col: 1,
                    row: 0,
                    db: -1.5,
                },
                SimMapCell {
                    col: 1,
                    row: 1,
                    db: -6.25,
                },
            ],
        }),
        ..sim_state()
    }
}

/// The fit kept from [`sim_state`].
pub fn sim_fit_kept() -> SimFitKept {
    SimFitKept {
        plan: "Hall".into(),
        walk: WalkId(2),
        absorption: sim_state().absorption,
    }
}
