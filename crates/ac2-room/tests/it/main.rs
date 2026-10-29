//! The crate's integration tests, one binary with a module per area. A single area:
//! `cargo nextest run -p ac2-room -E 'test(/^pattern::/)'`.

#![allow(clippy::unwrap_used)]

mod pattern;
mod plan;
mod steps;

use ac2_proto::model::{Marker, MarkerKind, PlanSpec, ZoneSpec};

pub fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<[f64; 2]> {
    vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

pub fn zone(name: &str, outline: Vec<[f64; 2]>) -> ZoneSpec {
    ZoneSpec {
        name: name.into(),
        outline,
        ear_height: 1.2,
        weight: None,
        seats: None,
    }
}

pub fn marker(name: &str, kind: MarkerKind, at: [f64; 3], heading_deg: Option<f64>) -> Marker {
    Marker {
        name: name.into(),
        kind,
        at,
        heading_deg,
    }
}

/// A 20 m × 30 m hall, stage at y = 0, with one zone over the audience area.
pub fn hall() -> PlanSpec {
    PlanSpec {
        name: "Hall".into(),
        outline: rect(0.0, 0.0, 20.0, 30.0),
        ceiling: Some(10.0),
        zones: vec![zone("Stalls", rect(0.0, 6.0, 20.0, 30.0))],
        markers: vec![],
        surfaces: vec![],
    }
}
