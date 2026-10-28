//! Room plans: a floor polygon extruded to an optional ceiling (a prism), with the zones where
//! people are and the markers (speakers, FOH, points) placed in it. Metres throughout; plan x/y
//! in the floor plane, z up from the floor. Validation and geometry live in `ac2-room`.

use serde::{Deserialize, Serialize};

/// A room plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanSpec {
    /// The plan's name, unique in the plan library.
    pub name: String,
    /// The floor outline, metres: a simple polygon, counter-clockwise, at least 3 vertices.
    /// Edge `i` runs from vertex `i` to vertex `i + 1` (the last back to the first).
    pub outline: Vec<[f64; 2]>,
    /// Ceiling height above the floor, metres. `None`: outdoors (no ceiling, no simulation).
    pub ceiling: Option<f64>,
    /// Where people are.
    pub zones: Vec<ZoneSpec>,
    /// Speakers, the FOH position and other named points.
    pub markers: Vec<Marker>,
    /// A material per outline edge, the floor and the ceiling; used by the simulation only.
    /// A surface without a tag takes the simulator's default material.
    pub surfaces: Vec<SurfaceTag>,
}

/// An audience zone: a floor area at one ear height.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZoneSpec {
    /// The zone's name ("Stalls", "Balcony").
    pub name: String,
    /// The zone in plan view, metres: a simple polygon inside the plan's outline.
    pub outline: Vec<[f64; 2]>,
    /// Ear height above the floor, metres; a balcony is a zone with a higher ear height.
    pub ear_height: f64,
    /// Share of the positions this zone gets. `None`: its area.
    pub weight: Option<f64>,
    /// Number of seats, when known (sets the ISO 3382-1 position count).
    pub seats: Option<u32>,
}

/// A named point in the room.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Marker {
    /// The marker's name ("Main L", "FOH").
    pub name: String,
    /// What the marker is.
    pub kind: MarkerKind,
    /// Position, metres: plan x, plan y, height above the floor.
    pub at: [f64; 3],
    /// Plan direction, degrees counter-clockwise from the plan +x axis. For FOH: the direction
    /// the operator faces at the desk (toward the stage). For a speaker: where it points.
    pub heading_deg: Option<f64>,
}

/// What a marker stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerKind {
    /// A loudspeaker (or a cluster's acoustic centre); inside the room.
    Speaker,
    /// The mix position; positions for people read from it. May lie outside (a truck).
    Foh,
    /// Any other named point; may lie outside (an open side).
    Point,
}

/// The material of one surface of the prism.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceTag {
    /// Which surface.
    pub surface: Surface,
    /// The material's name in the simulator's material table.
    pub material: String,
}

/// One surface of the prism a plan describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Surface {
    /// The wall standing on outline edge `edge` (0-based: vertex `edge` to `edge + 1`).
    Wall {
        /// Outline edge index.
        edge: u32,
    },
    /// The floor.
    Floor,
    /// The ceiling.
    Ceiling,
}
