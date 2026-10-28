//! Plan validation, the mirror axes a room and its speakers share, and positions read from FOH.
//!
//! Edges and vertices are 0-based in the error values and 1-based in their text, as the vertex
//! table shows them: edge 1 runs from vertex 1 to vertex 2.

use std::fmt;

use ac2_proto::model::{Marker, MarkerKind, PlanSpec, Surface};

use crate::geom::{
    self, EPS, centroid, cross, dist, inside, segments_cross, segments_meet, signed_area,
    without_collinear,
};

/// Which polygon of a plan an error is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Polygon {
    /// The floor outline.
    Outline,
    /// A zone, by name.
    Zone(String),
}

impl fmt::Display for Polygon {
    /// The prefix of an error's text: nothing for the outline, `zone X: ` for a zone.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Outline => Ok(()),
            Self::Zone(name) => write!(f, "zone {name}: "),
        }
    }
}

/// Why a plan is refused.
#[derive(Debug, Clone, PartialEq)]
pub enum PlanError {
    /// Fewer than 3 vertices.
    TooFewVertices {
        /// The polygon.
        polygon: Polygon,
        /// How many it has.
        count: usize,
    },
    /// A coordinate is NaN or infinite.
    NonFiniteVertex {
        /// The polygon.
        polygon: Polygon,
        /// Vertex index.
        vertex: usize,
    },
    /// Two consecutive vertices coincide.
    ZeroLengthEdge {
        /// The polygon.
        polygon: Polygon,
        /// Edge index.
        edge: usize,
    },
    /// Two edges meet other than at their shared vertex: the polygon is not simple.
    EdgesCross {
        /// The polygon.
        polygon: Polygon,
        /// The lower edge index.
        a: usize,
        /// The higher edge index.
        b: usize,
    },
    /// The polygon encloses no area (all vertices on one line).
    ZeroArea {
        /// The polygon.
        polygon: Polygon,
    },
    /// The outline runs clockwise.
    Clockwise,
    /// The ceiling is not a height above the floor.
    Ceiling {
        /// The value given, metres.
        value: f64,
    },
    /// A zone's ear height is below the floor, at or above the ceiling, or not a number.
    EarHeight {
        /// Zone name.
        zone: String,
        /// The value given, metres.
        value: f64,
    },
    /// A zone's weight is negative or not a number.
    Weight {
        /// Zone name.
        zone: String,
        /// The value given.
        value: f64,
    },
    /// A zone vertex lies outside the outline.
    ZoneVertexOutside {
        /// Zone name.
        zone: String,
        /// Vertex index in the zone.
        vertex: usize,
    },
    /// A zone edge leaves the outline between two vertices that are inside it.
    ZoneEdgeOutside {
        /// Zone name.
        zone: String,
        /// Edge index in the zone.
        edge: usize,
    },
    /// A marker's position or heading is NaN or infinite.
    MarkerNonFinite {
        /// Marker name.
        marker: String,
    },
    /// A marker is below the floor (z < 0).
    MarkerBelowFloor {
        /// Marker name.
        marker: String,
        /// Its height, metres.
        z: f64,
    },
    /// A speaker lies outside the outline in plan view.
    SpeakerOutside {
        /// Marker name.
        marker: String,
    },
    /// A speaker is above the ceiling.
    SpeakerAboveCeiling {
        /// Marker name.
        marker: String,
        /// Its height, metres.
        z: f64,
        /// The ceiling, metres.
        ceiling: f64,
    },
    /// A wall tag names an edge the outline does not have.
    SurfaceEdge {
        /// The edge index given.
        edge: u32,
        /// How many edges the outline has.
        edges: usize,
    },
    /// The ceiling is tagged on an outdoor plan.
    SurfaceCeilingOutdoors,
    /// One surface carries two tags.
    SurfaceTwice {
        /// The surface.
        surface: Surface,
    },
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooFewVertices { polygon, count } => {
                write!(f, "{polygon}{count} vertices: a polygon needs at least 3")
            }
            Self::NonFiniteVertex { polygon, vertex } => {
                write!(f, "{polygon}vertex {} is not a number", vertex + 1)
            }
            Self::ZeroLengthEdge { polygon, edge } => {
                write!(f, "{polygon}edge {} has zero length", edge + 1)
            }
            Self::EdgesCross { polygon, a, b } => {
                write!(f, "{polygon}edge {} crosses edge {}", a + 1, b + 1)
            }
            Self::ZeroArea { polygon } => write!(f, "{polygon}the polygon encloses no area"),
            Self::Clockwise => write!(
                f,
                "the outline runs clockwise: list its vertices counter-clockwise"
            ),
            Self::Ceiling { value } => {
                write!(f, "ceiling {value} m: give a height above the floor")
            }
            Self::EarHeight { zone, value } => write!(
                f,
                "zone {zone}: ear height {value} m is not between the floor and the ceiling"
            ),
            Self::Weight { zone, value } => {
                write!(f, "zone {zone}: weight {value} is not 0 or more")
            }
            Self::ZoneVertexOutside { zone, vertex } => write!(
                f,
                "zone {zone}: vertex {} lies outside the outline",
                vertex + 1
            ),
            Self::ZoneEdgeOutside { zone, edge } => {
                write!(f, "zone {zone}: edge {} leaves the outline", edge + 1)
            }
            Self::MarkerNonFinite { marker } => {
                write!(f, "marker {marker}: position or heading is not a number")
            }
            Self::MarkerBelowFloor { marker, z } => {
                write!(f, "marker {marker}: height {z} m is below the floor")
            }
            Self::SpeakerOutside { marker } => {
                write!(f, "speaker {marker} lies outside the outline")
            }
            Self::SpeakerAboveCeiling { marker, z, ceiling } => write!(
                f,
                "speaker {marker} at {z} m is above the ceiling ({ceiling} m)"
            ),
            Self::SurfaceEdge { edge, edges } => write!(
                f,
                "wall tag for edge {}: the outline has {edges} edges",
                u64::from(*edge) + 1
            ),
            Self::SurfaceCeilingOutdoors => {
                write!(f, "the ceiling is tagged but the plan has no ceiling")
            }
            Self::SurfaceTwice { surface } => match surface {
                Surface::Wall { edge } => {
                    write!(
                        f,
                        "the wall on edge {} is tagged twice",
                        u64::from(*edge) + 1
                    )
                }
                Surface::Floor => write!(f, "the floor is tagged twice"),
                Surface::Ceiling => write!(f, "the ceiling is tagged twice"),
            },
        }
    }
}

impl std::error::Error for PlanError {}

/// Checks that `poly` is a simple polygon with non-zero area; returns its signed area.
pub fn check_polygon(poly: &[[f64; 2]], polygon: &Polygon) -> Result<f64, PlanError> {
    let n = poly.len();
    if n < 3 {
        return Err(PlanError::TooFewVertices {
            polygon: polygon.clone(),
            count: n,
        });
    }
    if let Some(vertex) = poly
        .iter()
        .position(|v| !v[0].is_finite() || !v[1].is_finite())
    {
        return Err(PlanError::NonFiniteVertex {
            polygon: polygon.clone(),
            vertex,
        });
    }
    let edge = |i: usize| (poly[i], poly[(i + 1) % n]);
    if let Some(edge) = (0..n).find(|&i| {
        let (a, b) = edge(i);
        dist(a, b) <= EPS
    }) {
        return Err(PlanError::ZeroLengthEdge {
            polygon: polygon.clone(),
            edge,
        });
    }
    for i in 0..n {
        let (a, b) = edge(i);
        // The next edge shares vertex b; it may only meet this one there, which fails when
        // it folds back along the same line.
        let c = poly[(i + 2) % n];
        if n > 3
            && cross(a, b, c).abs() <= EPS * (1.0 + dist(a, c))
            && geom::dot(geom::sub(b, a), geom::sub(c, b)) < 0.0
        {
            let (lo, hi) = (i.min((i + 1) % n), i.max((i + 1) % n));
            return Err(PlanError::EdgesCross {
                polygon: polygon.clone(),
                a: lo,
                b: hi,
            });
        }
        for j in i + 2..n {
            if i == 0 && j == n - 1 {
                continue;
            }
            let (c, d) = edge(j);
            if segments_meet(a, b, c, d) {
                return Err(PlanError::EdgesCross {
                    polygon: polygon.clone(),
                    a: i,
                    b: j,
                });
            }
        }
    }
    let area = signed_area(poly);
    let scale = poly
        .iter()
        .map(|v| v[0].abs().max(v[1].abs()))
        .fold(1.0, f64::max);
    if area.abs() <= EPS * scale {
        return Err(PlanError::ZeroArea {
            polygon: polygon.clone(),
        });
    }
    Ok(area)
}

/// Checks a plan; the first problem found is the reason it is refused.
///
/// The outline is simple, counter-clockwise, with non-zero area; every zone is a simple polygon
/// (either orientation) inside the outline; every marker has z ≥ 0; every speaker lies inside
/// the outline and below the ceiling (FOH and points may lie outside); wall tags name existing
/// edges, each surface at most once. An outdoor plan (no ceiling) is valid.
pub fn validate(plan: &PlanSpec) -> Result<(), PlanError> {
    let outline = &plan.outline;
    if check_polygon(outline, &Polygon::Outline)? < 0.0 {
        return Err(PlanError::Clockwise);
    }
    if let Some(c) = plan.ceiling
        && !(c.is_finite() && c > 0.0)
    {
        return Err(PlanError::Ceiling { value: c });
    }
    let ceiling = plan.ceiling.unwrap_or(f64::INFINITY);
    let n = outline.len();
    for zone in &plan.zones {
        let poly = &zone.outline;
        check_polygon(poly, &Polygon::Zone(zone.name.clone()))?;
        let h = zone.ear_height;
        if !(h.is_finite() && h >= 0.0 && h < ceiling) {
            return Err(PlanError::EarHeight {
                zone: zone.name.clone(),
                value: h,
            });
        }
        if let Some(w) = zone.weight
            && !(w.is_finite() && w >= 0.0)
        {
            return Err(PlanError::Weight {
                zone: zone.name.clone(),
                value: w,
            });
        }
        if let Some(vertex) = poly.iter().position(|&v| !inside(v, outline)) {
            return Err(PlanError::ZoneVertexOutside {
                zone: zone.name.clone(),
                vertex,
            });
        }
        let m = poly.len();
        for e in 0..m {
            let (a, b) = (poly[e], poly[(e + 1) % m]);
            let crosses = (0..n).any(|w| segments_cross(a, b, outline[w], outline[(w + 1) % n]));
            // An edge between two inside vertices can still pass outside through a reflex
            // corner without crossing any wall; its midpoint then lies outside.
            let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
            if crosses || !inside(mid, outline) {
                return Err(PlanError::ZoneEdgeOutside {
                    zone: zone.name.clone(),
                    edge: e,
                });
            }
        }
    }
    for marker in &plan.markers {
        let finite = marker.at.iter().all(|v| v.is_finite())
            && marker.heading_deg.is_none_or(f64::is_finite);
        if !finite {
            return Err(PlanError::MarkerNonFinite {
                marker: marker.name.clone(),
            });
        }
        let z = marker.at[2];
        if z < 0.0 {
            return Err(PlanError::MarkerBelowFloor {
                marker: marker.name.clone(),
                z,
            });
        }
        if marker.kind == MarkerKind::Speaker {
            if !inside([marker.at[0], marker.at[1]], outline) {
                return Err(PlanError::SpeakerOutside {
                    marker: marker.name.clone(),
                });
            }
            if let Some(c) = plan.ceiling
                && z > c
            {
                return Err(PlanError::SpeakerAboveCeiling {
                    marker: marker.name.clone(),
                    z,
                    ceiling: c,
                });
            }
        }
    }
    let mut seen: Vec<Surface> = Vec::with_capacity(plan.surfaces.len());
    for tag in &plan.surfaces {
        match tag.surface {
            Surface::Wall { edge } if edge as usize >= n => {
                return Err(PlanError::SurfaceEdge { edge, edges: n });
            }
            Surface::Ceiling if plan.ceiling.is_none() => {
                return Err(PlanError::SurfaceCeilingOutdoors);
            }
            _ => {}
        }
        if seen.contains(&tag.surface) {
            return Err(PlanError::SurfaceTwice {
                surface: tag.surface,
            });
        }
        seen.push(tag.surface);
    }
    Ok(())
}

/// A line in plan view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Axis {
    /// A point on the line, metres.
    pub point: [f64; 2],
    /// Unit direction.
    pub dir: [f64; 2],
}

impl Axis {
    /// The mirror image of `p` across the line.
    pub fn reflect(&self, p: [f64; 2]) -> [f64; 2] {
        let d = geom::sub(p, self.point);
        let along = geom::dot(d, self.dir);
        let foot = [
            self.point[0] + along * self.dir[0],
            self.point[1] + along * self.dir[1],
        ];
        [2.0 * foot[0] - p[0], 2.0 * foot[1] - p[1]]
    }

    /// Distance from `p` to the line.
    pub fn distance(&self, p: [f64; 2]) -> f64 {
        let d = geom::sub(p, self.point);
        (d[0] * self.dir[1] - d[1] * self.dir[0]).abs()
    }
}

/// How far a mirrored vertex or speaker may land from its partner and still count as one:
/// plans are drawn to 0.1 m and speakers are placed by hand.
pub const MIRROR_MATCH: f64 = 0.05;

/// The mirror axes the outline and the speaker markers share: lines across which the outline
/// maps onto itself and every speaker onto a speaker at the same height. Vertices that lie on
/// the line through their neighbours are ignored. A symmetry axis of a polygon crosses its
/// boundary at vertices or edge midpoints and passes through its centroid, so those are the
/// only candidates.
pub fn mirror_axes(plan: &PlanSpec) -> Vec<Axis> {
    let poly = without_collinear(&plan.outline);
    if poly.len() < 3 || signed_area(&poly).abs() <= EPS {
        return Vec::new();
    }
    let c = centroid(&poly);
    let n = poly.len();
    let through = poly.iter().copied().chain((0..n).map(|i| {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
    }));
    let speakers: Vec<&Marker> = plan
        .markers
        .iter()
        .filter(|m| m.kind == MarkerKind::Speaker)
        .collect();
    let mut axes: Vec<Axis> = Vec::new();
    for t in through {
        let d = geom::sub(t, c);
        let len = d[0].hypot(d[1]);
        if len <= MIRROR_MATCH {
            continue;
        }
        let mut dir = [d[0] / len, d[1] / len];
        // One direction per line, so the same axis found from both ends is kept once.
        if dir[1] < -EPS || (dir[1].abs() <= EPS && dir[0] < 0.0) {
            dir = [-dir[0], -dir[1]];
        }
        let axis = Axis { point: c, dir };
        if axes.iter().any(|a| dist(a.dir, dir) < 1e-6) {
            continue;
        }
        let edges_match = (0..n).all(|i| {
            let (a, b) = (axis.reflect(poly[i]), axis.reflect(poly[(i + 1) % n]));
            (0..n).any(|j| {
                let (p, q) = (poly[j], poly[(j + 1) % n]);
                (dist(a, p) <= MIRROR_MATCH && dist(b, q) <= MIRROR_MATCH)
                    || (dist(a, q) <= MIRROR_MATCH && dist(b, p) <= MIRROR_MATCH)
            })
        });
        let speakers_match = speakers.iter().all(|s| {
            let r = axis.reflect([s.at[0], s.at[1]]);
            speakers.iter().any(|o| {
                dist(r, [o.at[0], o.at[1]]) <= MIRROR_MATCH
                    && (o.at[2] - s.at[2]).abs() <= MIRROR_MATCH
            })
        });
        if edges_match && speakers_match {
            axes.push(axis);
        }
    }
    axes
}

/// What a position is read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reference {
    /// The FOH marker (with a heading), facing the stage.
    Foh,
    /// A speaker marker (with a heading), facing it from the audience.
    Speaker(String),
}

/// Where a position is, as the operator reads it.
#[derive(Debug, Clone, PartialEq)]
pub enum Readout {
    /// Metres from the reference, facing the stage.
    Relative {
        /// Metres to the left (negative: right).
        left: f64,
        /// Metres back, away from the stage (negative: toward it).
        back: f64,
        /// What it is read from.
        from: Reference,
    },
    /// Plan coordinates: the plan has no FOH or speaker marker with a heading.
    Plan {
        /// Plan x, metres.
        x: f64,
        /// Plan y, metres.
        y: f64,
    },
}

/// One decimal, without a minus sign on a value that rounds to zero.
fn tenths(v: f64) -> String {
    let r = (v * 10.0).round() / 10.0;
    format!("{:.1}", if r == 0.0 { 0.0 } else { r })
}

impl fmt::Display for Readout {
    /// *2.1 m left, 4.0 m back from FOH*; plan coordinates say that they are.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Relative { left, back, from } => {
                let lr = if (left * 10.0).round() < 0.0 {
                    "right"
                } else {
                    "left"
                };
                let bf = if (back * 10.0).round() < 0.0 {
                    "forward"
                } else {
                    "back"
                };
                let name = match from {
                    Reference::Foh => "FOH",
                    Reference::Speaker(name) => name,
                };
                write!(
                    f,
                    "{} m {lr}, {} m {bf} from {name}",
                    tenths(left.abs()),
                    tenths(back.abs())
                )
            }
            Self::Plan { x, y } => write!(
                f,
                "x {} m, y {} m in plan (no FOH or speaker heading)",
                tenths(*x),
                tenths(*y)
            ),
        }
    }
}

/// Reads plan position `p` from FOH facing the stage: the first FOH marker with a heading;
/// without one, the first speaker marker with a heading (seen from the audience, facing it);
/// without that, plan x/y.
pub fn readout(plan: &PlanSpec, p: [f64; 2]) -> Readout {
    let with_heading = |kind: MarkerKind| {
        plan.markers
            .iter()
            .find(|m| m.kind == kind && m.heading_deg.is_some())
    };
    let (origin, facing_deg, from) = if let Some(m) = with_heading(MarkerKind::Foh) {
        (m, m.heading_deg.unwrap_or_default(), Reference::Foh)
    } else if let Some(m) = with_heading(MarkerKind::Speaker) {
        // A speaker points at the audience; someone facing the stage faces the other way.
        (
            m,
            m.heading_deg.unwrap_or_default() + 180.0,
            Reference::Speaker(m.name.clone()),
        )
    } else {
        return Readout::Plan { x: p[0], y: p[1] };
    };
    let h = facing_deg.to_radians();
    let forward = [h.cos(), h.sin()];
    let left_dir = [-h.sin(), h.cos()];
    let d = geom::sub(p, [origin.at[0], origin.at[1]]);
    Readout::Relative {
        left: geom::dot(d, left_dir),
        back: -geom::dot(d, forward),
        from,
    }
}

/// Distance from `p` to the nearest wall (outline edge), metres.
pub fn wall_distance(plan: &PlanSpec, p: [f64; 2]) -> f64 {
    geom::dist_to_boundary(p, &plan.outline)
}

/// `p` is inside polygon `poly` or on its boundary.
pub fn contains(poly: &[[f64; 2]], p: [f64; 2]) -> bool {
    inside(p, poly)
}

/// Polygon area, metres² (either orientation).
pub fn area(poly: &[[f64; 2]]) -> f64 {
    signed_area(poly).abs()
}
