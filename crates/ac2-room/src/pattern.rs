//! Suggested measurement positions: `n` points spread over a plan's zones by a centroidal
//! Voronoi (Lloyd) relaxation, then checked against the preset's rules.

use std::fmt;

use ac2_proto::model::{MarkerKind, PlanSpec};

use crate::geom::{self, P, signed_area};
use crate::plan::{Axis, PlanError, mirror_axes, validate};
use crate::rng::SplitMix64;

/// Speed of sound for the ISO 3382-1 source distance, m/s (20 °C).
pub const SPEED_OF_SOUND: f64 = 343.0;

/// Two points closer than this to each other's mirror image are a mirrored pair, metres.
pub const MIRROR_PAIR: f64 = 0.3;

/// Which table column a preset came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetKind {
    /// Tuning a system: the audience area, front rows included.
    Tuning,
    /// ISO 3382-1 room parameters.
    Iso3382,
}

/// The rules a suggestion follows. Start from [`Preset::tuning`] or [`Preset::iso3382`]; every
/// field may be edited.
#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    /// The column the rules came from.
    pub kind: PresetKind,
    /// Least distance between two positions, metres.
    pub spacing: f64,
    /// Least distance from a wall (outline edge) in plan view, metres.
    pub wall_distance: f64,
    /// Least distance from the floor, metres: a zone's ear height is its positions' height,
    /// so a zone below it is reported, not moved. `None`: no limit.
    pub floor_distance: Option<f64>,
    /// Least distance from every speaker marker, metres, at ear height; `None`: no limit.
    pub speaker_distance: Option<f64>,
    /// No two positions mirrored across a mirror axis of the room and its speakers.
    pub no_mirrored_pairs: bool,
}

/// Why a preset cannot be made.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PresetError {
    /// ISO 3382-1 needs a room volume; the plan has no ceiling.
    Outdoors,
    /// The reverberation time is not a positive number of seconds.
    ReverbTime(f64),
}

impl fmt::Display for PresetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Outdoors => write!(f, "ISO 3382-1 needs a ceiling: the plan is outdoors"),
            Self::ReverbTime(t) => write!(f, "reverberation time {t} s: give a time above 0 s"),
        }
    }
}

impl std::error::Error for PresetError {}

impl Preset {
    /// 2 m is half a wavelength at 86 Hz, 1 m a quarter: positions closer than that see the
    /// same low-frequency field. No speaker distance: the front rows are seats too.
    pub fn tuning() -> Self {
        Self {
            kind: PresetKind::Tuning,
            spacing: 2.0,
            wall_distance: 1.0,
            floor_distance: None,
            speaker_distance: None,
            no_mirrored_pairs: true,
        }
    }

    /// ISO 3382-1: as tuning, plus the source distance `d_min = 2·√(V/(c·T))` (about twice the
    /// critical distance), so the positions sample the reverberant field, and 1 m from every
    /// reflecting surface, the floor too (a mic nearer a surface sees its reflection as part
    /// of the direct sound). `V` is the prism's volume, `reverb_time_s` the reverberation
    /// time given or measured.
    pub fn iso3382(plan: &PlanSpec, reverb_time_s: f64) -> Result<Self, PresetError> {
        let ceiling = plan.ceiling.ok_or(PresetError::Outdoors)?;
        if !(reverb_time_s.is_finite() && reverb_time_s > 0.0) {
            return Err(PresetError::ReverbTime(reverb_time_s));
        }
        let volume = signed_area(&plan.outline).abs() * ceiling;
        Ok(Self {
            kind: PresetKind::Iso3382,
            spacing: 2.0,
            wall_distance: 1.0,
            floor_distance: Some(1.0),
            speaker_distance: Some(2.0 * (volume / (SPEED_OF_SOUND * reverb_time_s)).sqrt()),
            no_mirrored_pairs: true,
        })
    }

    /// The default position count over `zones`: 8 for tuning; for ISO 3382-1 6 / 8 / 10 for
    /// up to 500 / 1000 / 2000 seats (10 above), `None` when a zone's seats are unset (ask).
    pub fn default_n(&self, plan: &PlanSpec, zones: &[usize]) -> Option<usize> {
        match self.kind {
            PresetKind::Tuning => Some(8),
            PresetKind::Iso3382 => {
                let seats = zones.iter().try_fold(0u64, |sum, &z| {
                    plan.zones.get(z)?.seats.map(|s| sum + u64::from(s))
                })?;
                Some(match seats {
                    0..=500 => 6,
                    501..=1000 => 8,
                    _ => 10,
                })
            }
        }
    }
}

/// One suggested position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    /// Plan x, plan y, ear height; metres.
    pub at: [f64; 3],
    /// Index of its zone in the plan.
    pub zone: usize,
}

/// A rule the suggestion could not keep, and what it kept instead.
#[derive(Debug, Clone, PartialEq)]
pub enum Relaxed {
    /// The closest two positions are `got` apart.
    Spacing {
        /// The rule, metres.
        wanted: f64,
        /// The least spacing on the suggestion, metres.
        got: f64,
        /// How many positions were asked for.
        n: usize,
    },
    /// A zone has no room at the wall distance; its positions keep `got`.
    WallDistance {
        /// The rule, metres.
        wanted: f64,
        /// The distance its positions keep, metres.
        got: f64,
        /// Zone name.
        zone: String,
    },
    /// A zone's ear height is below the floor distance; its positions stand at `got`.
    FloorDistance {
        /// The rule, metres.
        wanted: f64,
        /// The zone's ear height, metres.
        got: f64,
        /// Zone name.
        zone: String,
    },
    /// A zone has no room at the speaker distance; its positions keep `got`.
    SpeakerDistance {
        /// The rule, metres.
        wanted: f64,
        /// The distance its positions keep, metres.
        got: f64,
        /// Zone name.
        zone: String,
    },
    /// Mirrored pairs left: their zones have no other place that keeps the other rules.
    MirroredPairs {
        /// How many pairs.
        pairs: usize,
    },
}

impl fmt::Display for Relaxed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spacing { wanted, got, n } => write!(
                f,
                "spacing {got:.1} m: {n} positions do not fit at {wanted:.1} m"
            ),
            Self::WallDistance { wanted, got, zone } => write!(
                f,
                "wall distance {got:.1} m: zone {zone} has no room at {wanted:.1} m"
            ),
            Self::FloorDistance { wanted, got, zone } => write!(
                f,
                "floor distance {got:.1} m: the ear height of zone {zone} is under {wanted:.1} m"
            ),
            Self::SpeakerDistance { wanted, got, zone } => write!(
                f,
                "speaker distance {got:.1} m: zone {zone} has no room at {wanted:.1} m"
            ),
            Self::MirroredPairs { pairs } => write!(
                f,
                "{pairs} mirrored pair{}: no other place keeps the rules",
                if *pairs == 1 { "" } else { "s" }
            ),
        }
    }
}

/// Suggested positions and the rules relaxed to place them, in the relaxation order: spacing,
/// wall distance, floor distance, speaker distance, mirrored pairs.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    /// The positions, zone by zone in the order the zones were given.
    pub points: Vec<Position>,
    /// Every rule that does not hold on `points`, with what holds instead.
    pub relaxed: Vec<Relaxed>,
}

/// Why no suggestion can be made.
#[derive(Debug, Clone, PartialEq)]
pub enum SuggestError {
    /// The plan is refused.
    Plan(PlanError),
    /// No zone was given.
    NoZones,
    /// A zone index is not in the plan.
    ZoneIndex(usize),
    /// A zone is given twice.
    ZoneTwice(usize),
    /// No positions were asked for.
    NoPositions,
    /// Every zone given has weight 0.
    NoWeight,
}

impl fmt::Display for SuggestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plan(e) => e.fmt(f),
            Self::NoZones => write!(f, "pick at least one zone"),
            Self::ZoneIndex(z) => write!(f, "zone {} is not in the plan", z + 1),
            Self::ZoneTwice(z) => write!(f, "zone {} is picked twice", z + 1),
            Self::NoPositions => write!(f, "ask for at least one position"),
            Self::NoWeight => write!(f, "every zone picked has weight 0"),
        }
    }
}

impl std::error::Error for SuggestError {}

impl From<PlanError> for SuggestError {
    fn from(e: PlanError) -> Self {
        Self::Plan(e)
    }
}

/// Splits `n` positions over `weights` in proportion (largest remainder), at least one to
/// every zone with a non-zero weight while `n` allows; when it does not, the heaviest zones
/// get one each.
pub fn quotas(weights: &[f64], n: usize) -> Vec<usize> {
    let total: f64 = weights.iter().filter(|w| **w > 0.0).sum();
    let mut out = vec![0usize; weights.len()];
    if total <= 0.0 || n == 0 {
        return out;
    }
    let exact: Vec<f64> = weights
        .iter()
        .map(|&w| if w > 0.0 { n as f64 * w / total } else { 0.0 })
        .collect();
    let mut live: Vec<usize> = (0..weights.len()).filter(|&i| weights[i] > 0.0).collect();
    if live.len() > n {
        live.sort_by(|&a, &b| weights[b].total_cmp(&weights[a]).then(a.cmp(&b)));
        for &i in &live[..n] {
            out[i] = 1;
        }
        return out;
    }
    for &i in &live {
        out[i] = (exact[i].floor() as usize).max(1);
    }
    let mut sum: usize = out.iter().sum();
    while sum > n {
        // Take from the zone furthest over its share that keeps one.
        let Some(&i) = live.iter().filter(|&&i| out[i] > 1).max_by(|&&a, &&b| {
            (out[a] as f64 - exact[a])
                .total_cmp(&(out[b] as f64 - exact[b]))
                .then(b.cmp(&a))
        }) else {
            break;
        };
        out[i] -= 1;
        sum -= 1;
    }
    while sum < n {
        let Some(&i) = live.iter().max_by(|&&a, &&b| {
            (exact[a] - out[a] as f64)
                .total_cmp(&(exact[b] - out[b] as f64))
                .then(b.cmp(&a))
        }) else {
            break;
        };
        out[i] += 1;
        sum += 1;
    }
    out
}

/// Sample points per zone: enough that a cell of a few positions holds hundreds of samples,
/// so the discrete centroids sit within a few centimetres of the continuous ones.
const SAMPLES: f64 = 2500.0;
/// Lloyd iterations at most; it converges in a few dozen on room shapes.
const LLOYD_ITERATIONS: usize = 100;
/// Passes over the points that break spacing or mirror rules.
const NUDGE_PASSES: usize = 6;

/// A sample point of a zone with its distances to the walls and the nearest speaker.
#[derive(Debug, Clone, Copy)]
struct Sample {
    p: P,
    wall: f64,
    speaker: f64,
}

/// Suggests `n` positions over the plan zones `zones` (indices into `plan.zones`), following
/// `preset`. The same plan, zones, `n`, preset and `seed` give the same suggestion.
///
/// `n` is split over the zones by weight (area when unset). In each zone the positions are
/// spread by a Lloyd relaxation over sample points of the zone; samples closer to a wall than
/// the wall distance (or to a speaker than the speaker distance) are rejected, so concave
/// zones need no inward offset. Points breaking spacing or mirrored-pair rules are then
/// nudged to the nearest sample that keeps them. Rules that still fail are relaxed in order,
/// spacing first, then wall distance, then speaker distance, and named in `relaxed`; a zone
/// whose ear height is under the floor distance is named too (its height is the operator's).
///
/// ```
/// use ac2_proto::model::{PlanSpec, ZoneSpec};
/// use ac2_room::pattern::{Preset, suggest};
///
/// let plan = PlanSpec {
///     name: "Hall".into(),
///     outline: vec![[0.0, 0.0], [20.0, 0.0], [20.0, 30.0], [0.0, 30.0]],
///     ceiling: Some(10.0),
///     zones: vec![ZoneSpec {
///         name: "Stalls".into(),
///         outline: vec![[0.0, 6.0], [20.0, 6.0], [20.0, 30.0], [0.0, 30.0]],
///         ear_height: 1.2,
///         weight: None,
///         seats: None,
///     }],
///     markers: vec![],
///     surfaces: vec![],
/// };
/// let s = suggest(&plan, &[0], 8, &Preset::tuning(), 1).unwrap();
/// assert_eq!(s.points.len(), 8);
/// assert!(s.relaxed.is_empty());
/// assert_eq!(s, suggest(&plan, &[0], 8, &Preset::tuning(), 1).unwrap());
/// ```
pub fn suggest(
    plan: &PlanSpec,
    zones: &[usize],
    n: usize,
    preset: &Preset,
    seed: u64,
) -> Result<Suggestion, SuggestError> {
    validate(plan)?;
    if zones.is_empty() {
        return Err(SuggestError::NoZones);
    }
    if n == 0 {
        return Err(SuggestError::NoPositions);
    }
    for (i, &z) in zones.iter().enumerate() {
        if z >= plan.zones.len() {
            return Err(SuggestError::ZoneIndex(z));
        }
        if zones[..i].contains(&z) {
            return Err(SuggestError::ZoneTwice(z));
        }
    }
    let weights: Vec<f64> = zones
        .iter()
        .map(|&z| {
            let zone = &plan.zones[z];
            zone.weight
                .unwrap_or_else(|| signed_area(&zone.outline).abs())
        })
        .collect();
    if weights.iter().all(|w| *w <= 0.0) {
        return Err(SuggestError::NoWeight);
    }
    let quota = quotas(&weights, n);
    let speakers: Vec<[f64; 3]> = plan
        .markers
        .iter()
        .filter(|m| m.kind == MarkerKind::Speaker)
        .map(|m| m.at)
        .collect();
    let axes = if preset.no_mirrored_pairs {
        mirror_axes(plan)
    } else {
        Vec::new()
    };
    let mut rng = SplitMix64::new(seed);
    let mut wall_relaxed = Vec::new();
    let mut floor_relaxed = Vec::new();
    let mut speaker_relaxed = Vec::new();
    // Per picked zone: its valid samples.
    let mut valid: Vec<Vec<P>> = Vec::with_capacity(zones.len());
    let mut points: Vec<Position> = Vec::with_capacity(n);
    for (slot, &z) in zones.iter().enumerate() {
        let zone = &plan.zones[z];
        let samples = sample_zone(plan, &zone.outline, zone.ear_height, &speakers, &mut rng);
        let (wall, speaker) = limits(&samples, preset);
        let name = &zone.name;
        if quota[slot] > 0 && wall < preset.wall_distance {
            wall_relaxed.push(Relaxed::WallDistance {
                wanted: preset.wall_distance,
                got: wall,
                zone: name.clone(),
            });
        }
        if let Some(wanted) = preset.floor_distance
            && quota[slot] > 0
            && zone.ear_height < wanted - geom::EPS
        {
            floor_relaxed.push(Relaxed::FloorDistance {
                wanted,
                got: zone.ear_height,
                zone: name.clone(),
            });
        }
        if let Some(wanted) = preset.speaker_distance
            && quota[slot] > 0
            && speaker < wanted
        {
            speaker_relaxed.push(Relaxed::SpeakerDistance {
                wanted,
                got: speaker,
                zone: name.clone(),
            });
        }
        let ok: Vec<P> = samples
            .iter()
            .filter(|s| s.wall >= wall && s.speaker >= speaker)
            .map(|s| s.p)
            .collect();
        let place = |p: P| {
            geom::inside(p, &zone.outline)
                && geom::dist_to_boundary(p, &plan.outline) >= wall
                && speaker_distance(&speakers, p, zone.ear_height) >= speaker
        };
        for p in lloyd(&ok, quota[slot], &place, &mut rng) {
            points.push(Position {
                at: [p[0], p[1], zone.ear_height],
                zone: slot,
            });
        }
        valid.push(ok);
    }
    nudge(&mut points, &valid, preset.spacing, &axes);
    for p in &mut points {
        p.zone = zones[p.zone];
    }

    let mut relaxed = Vec::new();
    let got = min_spacing(&points);
    if got < preset.spacing - geom::EPS {
        relaxed.push(Relaxed::Spacing {
            wanted: preset.spacing,
            got,
            n,
        });
    }
    relaxed.extend(wall_relaxed);
    relaxed.extend(floor_relaxed);
    relaxed.extend(speaker_relaxed);
    let pairs = mirrored_pairs(&points, &axes);
    if pairs > 0 {
        relaxed.push(Relaxed::MirroredPairs { pairs });
    }
    Ok(Suggestion { points, relaxed })
}

/// Least 3D distance between two positions; infinite for fewer than two.
pub fn min_spacing(points: &[Position]) -> f64 {
    let mut best = f64::INFINITY;
    for (i, a) in points.iter().enumerate() {
        for b in &points[i + 1..] {
            best = best.min(dist3(a.at, b.at));
        }
    }
    best
}

/// How many pairs of positions lie within [`MIRROR_PAIR`] of each other's mirror image across
/// one of `axes`.
pub fn mirrored_pairs(points: &[Position], axes: &[Axis]) -> usize {
    let mut count = 0;
    for (i, a) in points.iter().enumerate() {
        for b in &points[i + 1..] {
            if axes
                .iter()
                .any(|ax| mirrored(ax, plan_xy(a.at), plan_xy(b.at)))
            {
                count += 1;
            }
        }
    }
    count
}

fn plan_xy(p: [f64; 3]) -> P {
    [p[0], p[1]]
}

fn dist3(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn mirrored(axis: &Axis, a: P, b: P) -> bool {
    geom::dist(axis.reflect(a), b) < MIRROR_PAIR
}

/// Distance from plan point `p` at `ear_height` to the nearest speaker; infinite with none.
fn speaker_distance(speakers: &[[f64; 3]], p: P, ear_height: f64) -> f64 {
    speakers
        .iter()
        .map(|s| dist3(*s, [p[0], p[1], ear_height]))
        .fold(f64::INFINITY, f64::min)
}

/// A jittered grid over the zone, every sample inside it, with its wall and speaker distance.
fn sample_zone(
    plan: &PlanSpec,
    zone: &[P],
    ear_height: f64,
    speakers: &[[f64; 3]],
    rng: &mut SplitMix64,
) -> Vec<Sample> {
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for v in zone {
        for k in 0..2 {
            lo[k] = lo[k].min(v[k]);
            hi[k] = hi[k].max(v[k]);
        }
    }
    let h = (signed_area(zone).abs() / SAMPLES).sqrt().max(1e-3);
    let (ox, oy) = (rng.unit() * h, rng.unit() * h);
    let mut pts = Vec::new();
    let mut y = lo[1] + oy;
    while y <= hi[1] {
        let mut x = lo[0] + ox;
        while x <= hi[0] {
            if geom::inside([x, y], zone) {
                pts.push([x, y]);
            }
            x += h;
        }
        y += h;
    }
    if pts.is_empty() {
        // A sliver narrower than the grid: its vertices are still in it.
        pts.extend_from_slice(zone);
    }
    pts.into_iter()
        .map(|p| Sample {
            p,
            wall: geom::dist_to_boundary(p, &plan.outline),
            speaker: speaker_distance(speakers, p, ear_height),
        })
        .collect()
}

/// The wall and speaker distances a zone's samples are held to: the preset's, or, where no
/// sample keeps both, the wall distance relaxed first and then the speaker distance, each to
/// the largest any sample keeps.
fn limits(samples: &[Sample], preset: &Preset) -> (f64, f64) {
    let w = preset.wall_distance;
    let s = preset.speaker_distance.unwrap_or(0.0);
    let max_by = |f: &dyn Fn(&Sample) -> bool, g: fn(&Sample) -> f64| {
        samples
            .iter()
            .filter(|x| f(x))
            .map(g)
            .fold(f64::NEG_INFINITY, f64::max)
    };
    if samples.iter().any(|x| x.wall >= w && x.speaker >= s) {
        return (w, s);
    }
    let wall_at_s = max_by(&|x| x.speaker >= s, |x| x.wall);
    if wall_at_s.is_finite() {
        return (wall_at_s, s);
    }
    let wall = max_by(&|_| true, |x| x.wall).min(w);
    let speaker = max_by(&|x| x.wall >= wall, |x| x.speaker);
    (wall, speaker)
}

/// Lloyd relaxation of `k` points over the sample set: each point moves to the centroid of the
/// samples nearest it, or to the nearest sample of its cell when that centroid is not a valid
/// `place` (a concave zone's cell can have its centroid outside the zone or in a wall band).
fn lloyd(samples: &[P], k: usize, place: &dyn Fn(P) -> bool, rng: &mut SplitMix64) -> Vec<P> {
    if k == 0 || samples.is_empty() {
        return Vec::new();
    }
    let mut order: Vec<usize> = (0..samples.len()).collect();
    let mut centres = Vec::with_capacity(k);
    for i in 0..k {
        let m = order.len();
        if i < m {
            let j = i + rng.below(m - i);
            order.swap(i, j);
            centres.push(samples[order[i]]);
        } else {
            centres.push(samples[order[rng.below(m)]]);
        }
    }
    let mut owner = vec![0usize; samples.len()];
    for _ in 0..LLOYD_ITERATIONS {
        let mut sum = vec![[0.0f64; 2]; k];
        let mut count = vec![0usize; k];
        for (s, p) in samples.iter().enumerate() {
            let c = nearest(&centres, *p);
            owner[s] = c;
            sum[c][0] += p[0];
            sum[c][1] += p[1];
            count[c] += 1;
        }
        let mut moved = 0.0f64;
        for c in 0..k {
            if count[c] == 0 {
                continue;
            }
            let mean = [sum[c][0] / count[c] as f64, sum[c][1] / count[c] as f64];
            let next = if place(mean) {
                mean
            } else {
                samples
                    .iter()
                    .zip(&owner)
                    .filter(|(_, o)| **o == c)
                    .map(|(p, _)| *p)
                    .min_by(|a, b| geom::dist(*a, mean).total_cmp(&geom::dist(*b, mean)))
                    .unwrap_or(centres[c])
            };
            moved = moved.max(geom::dist(next, centres[c]));
            centres[c] = next;
        }
        if moved < 1e-4 {
            break;
        }
    }
    centres
}

fn nearest(centres: &[P], p: P) -> usize {
    let mut best = (0, f64::INFINITY);
    for (i, c) in centres.iter().enumerate() {
        let d = (c[0] - p[0]).powi(2) + (c[1] - p[1]).powi(2);
        if d < best.1 {
            best = (i, d);
        }
    }
    best.0
}

/// Moves each point that is closer than `spacing` to another or mirrored with another to the
/// valid sample of its zone that is mirror-free and keeps the most spacing (capped at
/// `spacing`), the nearest such sample when several tie; a point moves only to a better place.
fn nudge(points: &mut [Position], valid: &[Vec<P>], spacing: f64, axes: &[Axis]) {
    // Squared room to the nearest other point, capped at `spacing`; stops early once it
    // drops to `floor`, where the candidate can no longer win.
    let room2 = |points: &[Position], i: usize, p: [f64; 3], floor: f64| {
        let mut r = spacing * spacing;
        for (j, o) in points.iter().enumerate() {
            if j != i {
                let d =
                    (p[0] - o.at[0]).powi(2) + (p[1] - o.at[1]).powi(2) + (p[2] - o.at[2]).powi(2);
                if d < r {
                    r = d;
                    if r <= floor {
                        break;
                    }
                }
            }
        }
        r
    };
    let mirror_free = |points: &[Position], i: usize, p: P| {
        points
            .iter()
            .enumerate()
            .all(|(j, o)| j == i || axes.iter().all(|ax| !mirrored(ax, p, plan_xy(o.at))))
    };
    // Mirror-free beats mirrored; then more room; equal scores within a micrometre tie.
    let better =
        |a: (bool, f64), b: (bool, f64)| (a.0 && !b.0) || (a.0 == b.0 && a.1 > b.1 + 1e-12);
    for _ in 0..NUDGE_PASSES {
        let mut changed = false;
        for i in 0..points.len() {
            let here = points[i].at;
            let current = (
                mirror_free(points, i, plan_xy(here)),
                room2(points, i, here, -1.0),
            );
            if current.0 && current.1 >= spacing * spacing - geom::EPS {
                continue;
            }
            // The current place scores first at zero displacement, so a point moves only to
            // a strictly better place, and to the nearest of several equally good ones.
            let mut best = (current, 0.0f64, here);
            for s in &valid[points[i].zone] {
                let p = [s[0], s[1], here[2]];
                let moved = geom::dist(*s, plan_xy(here));
                // A mirror-free best can only be beaten on room, or tied nearer.
                let floor = if best.0.0 { best.0.1 - 1e-12 } else { -1.0 };
                let r = room2(points, i, p, floor);
                if best.0.0 && r < best.0.1 - 1e-12 {
                    continue;
                }
                let sc = (mirror_free(points, i, *s), r);
                let tie = !better(sc, best.0) && !better(best.0, sc);
                if better(sc, best.0) || (tie && moved < best.1) {
                    best = (sc, moved, p);
                }
            }
            if best.2 != here {
                points[i].at = best.2;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}
