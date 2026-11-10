//! 3D spectrograph: the spectrograph's history seen as a skyline, level as height
//! (`docs/design/spectrograph-3d.md`).
//!
//! Frequency runs left to right across a dark floor, time from the front edge (newest)
//! back into the distance, and on every cell of a decimated grid stands one thin spike
//! whose height is the cell's level within the pane's level range. Rows behind show through
//! the gaps; nothing hides anything, so there is no cursor and no readout here: which row a
//! pointer is over is ambiguous in such a picture, and the 2D view one G back reads values.
//!
//! This module is display truth only: the decimated grid ([`Skyline`]), the camera and its
//! clamps, the projection, the labels and the caption. `ac2-plot` places the pixels.

use std::sync::Arc;

use ac2_proto::model::layout::SpectrumCamera;

use crate::axis::{self, Range};
use crate::banner::Status;
use crate::canvas::{self, Canvas, MARGINS, anchor, label};
use crate::colormap;
use crate::primitives::{
    Color, Colormap, Dash, FillPolygon, FillRect, HAlign, HeatmapColumn, Label, Polyline, Rect,
    Scene, Skyline3d, SkylineGlow, SkylineId, SkylineLook, SkylineTier, Stroke, VAlign, Viewport,
};
use crate::spectrograph::SpectrographHistory;
use crate::spectrum::{SpectrumTrace, offset_note};
use crate::theme::Theme;
use crate::time::Freshness;
use crate::view::{PlotChrome, ViewState};

type V3 = [f64; 3];

/// Floor half-width (frequency, x in −W … W): wider than deep keeps ten octaves readable
/// from the default camera.
pub const FLOOR_HALF_W: f64 = 1.0;
/// Floor depth (age, z in 0 … D, newest at 0).
pub const FLOOR_DEPTH: f64 = 1.2;
/// Height of a spike at the top of the level range: tall enough to rise above the far rows
/// without leaving the frame.
pub const MAX_HEIGHT: f64 = 0.7;

/// The spike look. Set on synthetic room-like data (a look spike, `docs/design`); to be
/// retuned on a rig recording, here and nowhere else.
pub const LOOK: SkylineLook = SkylineLook {
    foot: 0.05,
    body_pow: 3.5,
    tip_px: 2.5,
    tip_gain: 1.5,
    level_floor: 0.3,
    age_fade: 2.0,
    eye_fade: 0.35,
    width_share: 0.35,
    width_px: [1.0, 3.0],
    // Rows stacked about a pixel apart at the default camera sum the dim bodies of a loud
    // low end past 1 in the 8-bit bloom target, a shapeless haze over the bass; light under
    // the threshold feeds no bloom, so only the tips glow.
    bloom_threshold: 0.25,
    bloom_gain: 0.12,
};

/// The bloom added over the pane: strong for ice (the holographic look), weaker for the
/// other colour maps so their hues stay legible under it.
pub const GLOW_ICE: f32 = 0.6;
pub const GLOW_MAP: f32 = 0.35;

/// The bloom's gain over the pane for colour map `c`.
pub fn glow_gain(c: Colormap) -> f32 {
    if c == Colormap::Ice {
        GLOW_ICE
    } else {
        GLOW_MAP
    }
}

/// Spike widths in the half-resolution target of the lighter tiers.
pub const LITE_WIDTH_PX: [f32; 2] = [1.0, 2.0];

/// The floor band's light along the front edge, as a share of the colour map's middle,
/// and the one-pixel "now" line's.
pub const FLOOR_BAND: f32 = 0.12;
pub const NOW_LINE: f32 = 0.35;
/// Floor band strips front to back: enough that the fade reads as a gradient.
const FLOOR_STRIPS: usize = 24;

/// The plot field, whatever the theme: light added to a white field would be invisible.
pub const FIELD: Color = Color::rgb(5.0 / 255.0, 8.0 / 255.0, 12.0 / 255.0);

/// Orbit target: the floor's centre, a little up.
pub const TARGET: V3 = [0.0, 0.15, FLOOR_DEPTH / 2.0];
pub const FOV_Y_DEG: f64 = 35.0;
pub const NEAR: f64 = 0.05;
pub const FAR: f64 = 20.0;
/// Margin left around the box's corners by the fitted distance, of the plot's half-width:
/// on a full-screen pane the height scale's numbers then fit outside its post.
const FIT_MARGIN: f64 = 0.07;
/// The eye stays at least this far outside the skyline's box.
pub const EYE_CLEARANCE: f64 = 0.15;
/// Zoom steps per doubling of the distance: about 9 % a step.
pub const ZOOM_STEPS_PER_OCTAVE: f64 = 8.0;

/// Key steps.
pub const YAW_KEY_DEG: i32 = 15;
pub const PITCH_KEY_DEG: i32 = 5;
pub const ZOOM_KEY_STEPS: i32 = 2;
/// Drag: degrees per logical pixel.
pub const DRAG_YAW_DEG_PER_PX: f64 = 0.3;
pub const DRAG_PITCH_DEG_PER_PX: f64 = 0.2;

const SKYLINE_ID: SkylineId = SkylineId(1);
/// Above the plot: the caption.
const CAPTION_H: f32 = 16.0;
/// Room right of the plot: the 2D view's, so the plot keeps its place across G.
const RIGHT_MARGIN: f32 = crate::spectrograph::RIGHT_MARGIN;

/// The grid the skyline is made of: columns, and about how many rows of history.
pub fn grid(tier: SkylineTier) -> (usize, usize) {
    match tier {
        SkylineTier::Full => (192, 96),
        SkylineTier::Lite | SkylineTier::Bare => (128, 48),
    }
}

/// History slots per row of the skyline: an integer, so rows are whole slots anchored on
/// absolute slot numbers and never shift as the history scrolls.
pub fn bucket_slots(slots: usize, tier: SkylineTier) -> usize {
    let rows = grid(tier).1;
    ((slots as f64 / rows as f64).round() as usize).max(1)
}

/// Rows the ring holds for `slots` slots of `k` each: those the history covers and the
/// partial one at the other end.
pub fn ring_rows(slots: usize, k: usize) -> usize {
    slots.div_ceil(k) + 1
}

// ---------------------------------------------------------------------------------------
// The skyline: the decimated grid.

#[derive(Clone, Debug, PartialEq)]
struct Key {
    generation: u64,
    span_s: u32,
    slots: usize,
    rows: usize,
    rows_lo: f64,
    rows_hi: f64,
    lo: f64,
    hi: f64,
    tier: SkylineTier,
}

/// The skyline of a spectrograph history: `C` log-spaced columns over the pane's frequency
/// span (within the history's rows), each the highest level of the rows it covers, and a
/// ring of rows of `k` whole slots each, each the highest level of its slots. Always
/// exactly a function of the history, the span and the tier: [`Skyline::update`] folds the
/// slots written since it last looked and recomputes the oldest row, and folding equals
/// rebuilding.
#[derive(Clone, Debug, Default)]
pub struct Skyline {
    key: Option<Key>,
    /// History rows `[first, end)` each column takes its maximum of.
    cols: Vec<(u32, u32)>,
    k: u64,
    ring: Vec<Option<HeatmapColumn>>,
    /// The row (absolute bucket number) each ring position holds.
    ids: Vec<Option<u64>>,
    /// The history's newest slot when last folded.
    newest: Option<u64>,
    slot_s: f64,
}

impl Skyline {
    /// Brings the skyline up to `h` over frequency span `freq` in `tier`'s grid: a fold
    /// of what is new, or a rebuild when the history was reset or the span, the grid or
    /// the tier changed.
    pub fn update(&mut self, h: &SpectrographHistory, freq: Range, tier: SkylineTier) {
        let (cols, _) = grid(tier);
        let key = h.rows().map(|r| {
            let lo = freq.lo.max(r.lo_hz());
            let hi = freq.hi.min(r.hi_hz());
            Key {
                generation: h.generation(),
                span_s: h.span_s(),
                slots: h.slots(),
                rows: r.rows(),
                rows_lo: r.lo_hz(),
                rows_hi: r.hi_hz(),
                lo,
                hi,
                tier,
            }
        });
        if key != self.key {
            let k = bucket_slots(h.slots(), tier);
            *self = Self {
                cols: key
                    .as_ref()
                    .filter(|k| k.lo.is_finite() && k.hi > k.lo)
                    .map_or_else(Vec::new, |k| column_rows(k, cols)),
                key,
                k: k as u64,
                ring: vec![None; ring_rows(h.slots(), k)],
                ids: vec![None; ring_rows(h.slots(), k)],
                newest: None,
                slot_s: h.slot_s(),
            };
        }
        self.fold(h);
    }

    fn fold(&mut self, h: &SpectrographHistory) {
        let Some(new) = h.newest_slot().filter(|_| !self.cols.is_empty()) else {
            self.ring.iter_mut().for_each(|r| *r = None);
            self.ids.iter_mut().for_each(|r| *r = None);
            self.newest = None;
            return;
        };
        let n = h.slots() as u64;
        let k = self.k;
        let first_slot = new.saturating_sub(n - 1);
        let (b_lo, b_hi) = (first_slot / k, new / k);
        // Slots written since the last fold are those after the newest then (and the newest
        // itself, which a second frame in its slot raises).
        let from = match self.newest {
            Some(old) if old <= new && new - old < n => (old / k).max(b_lo),
            _ => b_lo,
        };
        let len = self.ring.len() as u64;
        for b in std::iter::once(b_lo).chain(from..=b_hi) {
            let pos = (b % len) as usize;
            self.ring[pos] = self.bucket(h, b, first_slot, new);
            self.ids[pos] = Some(b);
        }
        for (pos, id) in self.ids.iter_mut().enumerate() {
            if id.is_some_and(|b| b < b_lo || b > b_hi) {
                *id = None;
                self.ring[pos] = None;
            }
        }
        self.newest = Some(new);
    }

    /// Row `b`: per column the highest level of its slots within `first … last`; `None`
    /// when none of them holds a frame.
    fn bucket(
        &self,
        h: &SpectrographHistory,
        b: u64,
        first: u64,
        last: u64,
    ) -> Option<HeatmapColumn> {
        let s0 = (b * self.k).max(first);
        let s1 = (b * self.k + self.k - 1).min(last);
        let mut out: Option<Vec<f32>> = None;
        for s in s0..=s1 {
            let Some(col) = h.at_slot(s) else {
                continue;
            };
            let o = out.get_or_insert_with(|| vec![f32::NAN; self.cols.len()]);
            for (v, &(a, e)) in o.iter_mut().zip(&self.cols) {
                for x in &col[a as usize..(e as usize).min(col.len())] {
                    // `max` keeps the number where one side has none (NaN).
                    *v = v.max(*x);
                }
            }
        }
        out.map(Arc::from)
    }

    /// Columns.
    pub fn cols(&self) -> usize {
        self.cols.len()
    }

    /// Ring rows (R + 1).
    pub fn rows(&self) -> usize {
        self.ring.len()
    }

    /// History slots per row.
    pub fn bucket_slots(&self) -> usize {
        self.k as usize
    }

    /// The ring by position; each row [`Self::cols`] levels, NaN where a column has none.
    pub fn ring(&self) -> &[Option<HeatmapColumn>] {
        &self.ring
    }

    /// Ring position of the newest row.
    pub fn newest(&self) -> usize {
        self.newest.map_or(0, |n| {
            ((n / self.k) % self.ring.len().max(1) as u64) as usize
        })
    }

    /// The newest row's fill fraction (slots filled ÷ slots per row), in (0, 1]: how far
    /// the rows have glided back. From the data, so a stalled stream stands still.
    pub fn phase(&self) -> f32 {
        self.newest
            .map_or(1.0, |n| ((n % self.k) + 1) as f32 / self.k as f32)
    }

    /// The frequency span the columns cover, Hz.
    pub fn span_hz(&self) -> Option<(f64, f64)> {
        self.key
            .as_ref()
            .filter(|_| !self.cols.is_empty())
            .map(|k| (k.lo, k.hi))
    }

    /// Length of one row, seconds.
    pub fn row_s(&self) -> f64 {
        self.k as f64 * self.slot_s
    }

    /// The row at `j` rows before the newest: `None` outside the ring or without data.
    pub fn row_back(&self, j: usize) -> Option<&HeatmapColumn> {
        let len = self.ring.len();
        if j >= len {
            return None;
        }
        self.ring[(self.newest() + len - j) % len].as_ref()
    }
}

/// History rows `[first, end)` per column for `cols` log-spaced columns over the key's
/// span: those whose centres fall in the column, or the one under its centre when the
/// column is narrower than a row.
fn column_rows(k: &Key, cols: usize) -> Vec<(u32, u32)> {
    let rn = k.rows as f64;
    let t = |hz: f64| (hz / k.rows_lo).ln() / (k.rows_hi / k.rows_lo).ln();
    let edge = |i: usize| k.lo * (k.hi / k.lo).powf(i as f64 / cols as f64);
    (0..cols)
        .map(|i| {
            let (a, b) = (edge(i), edge(i + 1));
            let first = (t(a) * rn - 0.5).ceil().clamp(0.0, rn) as u32;
            let end = (t(b) * rn - 0.5).ceil().clamp(0.0, rn) as u32;
            if end > first {
                return (first, end);
            }
            let c = t((a * b).sqrt()) * rn;
            if (0.0..rn).contains(&c) {
                (c as u32, c as u32 + 1)
            } else {
                (0, 0)
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// Camera.

/// The camera as drawn: continuous, so a key step can ease and a drag follow the pointer;
/// stored as a [`SpectrumCamera`] in whole degrees and steps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub yaw_deg: f64,
    pub pitch_deg: f64,
    pub zoom_step: f64,
}

impl From<SpectrumCamera> for Pose {
    fn from(c: SpectrumCamera) -> Self {
        Self {
            yaw_deg: f64::from(c.yaw_deg),
            pitch_deg: f64::from(c.pitch_deg),
            zoom_step: f64::from(c.zoom_step),
        }
    }
}

impl Pose {
    /// Within the camera's ranges.
    pub fn clamped(self) -> Self {
        let yaw = f64::from(SpectrumCamera::YAW_MAX_DEG);
        let p = SpectrumCamera::PITCH_DEG;
        let z = SpectrumCamera::ZOOM_STEPS;
        Self {
            yaw_deg: self.yaw_deg.clamp(-yaw, yaw),
            pitch_deg: self
                .pitch_deg
                .clamp(f64::from(*p.start()), f64::from(*p.end())),
            zoom_step: self
                .zoom_step
                .clamp(f64::from(*z.start()), f64::from(*z.end())),
        }
    }

    /// The nearest stored camera: whole degrees and steps, within the ranges.
    pub fn snapped(self) -> SpectrumCamera {
        let c = self.clamped();
        SpectrumCamera {
            yaw_deg: c.yaw_deg.round() as i8,
            pitch_deg: c.pitch_deg.round() as u8,
            zoom_step: c.zoom_step.round() as i8,
        }
    }

    /// Unit vector from the orbit target towards the eye.
    fn towards_eye(self) -> V3 {
        let (y, p) = (self.yaw_deg.to_radians(), self.pitch_deg.to_radians());
        [y.sin() * p.cos(), p.sin(), -y.cos() * p.cos()]
    }
}

/// `c` moved by key steps: `yaw` steps of [`YAW_KEY_DEG`] (positive: the eye moves right),
/// `pitch` of [`PITCH_KEY_DEG`] (positive: up), `zoom` steps (positive: closer); clamped.
pub fn step(c: SpectrumCamera, yaw: i32, pitch: i32, zoom: i32) -> SpectrumCamera {
    let yaw_max = i32::from(SpectrumCamera::YAW_MAX_DEG);
    let p = SpectrumCamera::PITCH_DEG;
    let z = SpectrumCamera::ZOOM_STEPS;
    SpectrumCamera {
        yaw_deg: (i32::from(c.yaw_deg) + yaw * YAW_KEY_DEG).clamp(-yaw_max, yaw_max) as i8,
        pitch_deg: (i32::from(c.pitch_deg) + pitch * PITCH_KEY_DEG)
            .clamp(i32::from(*p.start()), i32::from(*p.end())) as u8,
        zoom_step: (i32::from(c.zoom_step) + zoom).clamp(i32::from(*z.start()), i32::from(*z.end()))
            as i8,
    }
}

/// `c` moved by a drag of `dx`, `dy` logical pixels (right, down): the eye orbits with the
/// pointer, a drag up looks from higher. Continuous; snapped on release.
pub fn drag(c: Pose, dx: f64, dy: f64) -> Pose {
    Pose {
        yaw_deg: c.yaw_deg - dx * DRAG_YAW_DEG_PER_PX,
        pitch_deg: c.pitch_deg - dy * DRAG_PITCH_DEG_PER_PX,
        ..c
    }
    .clamped()
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(a: V3) -> V3 {
    scale(a, 1.0 / dot(a, a).sqrt())
}

/// The skyline's box: the floor and the full height above it.
const BOX_MIN: V3 = [-FLOOR_HALF_W, 0.0, 0.0];
const BOX_MAX: V3 = [FLOOR_HALF_W, MAX_HEIGHT, FLOOR_DEPTH];

fn box_corners() -> [V3; 8] {
    let mut out = [[0.0; 3]; 8];
    for (i, c) in out.iter_mut().enumerate() {
        for (a, v) in c.iter_mut().enumerate() {
            *v = if i & (1 << a) != 0 {
                BOX_MAX[a]
            } else {
                BOX_MIN[a]
            };
        }
    }
    out
}

/// Distance along `dir` (unit) from the target to where the ray leaves the box.
fn exit_distance(dir: V3) -> f64 {
    (0..3)
        .filter(|&a| dir[a] != 0.0)
        .map(|a| {
            let bound = if dir[a] > 0.0 { BOX_MAX[a] } else { BOX_MIN[a] };
            (bound - TARGET[a]) / dir[a]
        })
        .fold(f64::INFINITY, f64::min)
}

/// World to clip space for a plot of aspect `aspect` (width ÷ height).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewProj {
    /// Row-major: `clip[i] = Σ m[i][j] · p[j]`, `p = (x, y, z, 1)`.
    pub m: [[f64; 4]; 4],
    pub eye: V3,
}

impl ViewProj {
    pub fn clip(&self, p: V3) -> [f64; 4] {
        let v = [p[0], p[1], p[2], 1.0];
        let row = |i: usize| (0..4).map(|j| self.m[i][j] * v[j]).sum::<f64>();
        [row(0), row(1), row(2), row(3)]
    }

    /// Normalised device coordinates, `None` behind the near plane.
    pub fn ndc(&self, p: V3) -> Option<[f64; 2]> {
        let c = self.clip(p);
        (c[3] >= NEAR).then(|| [c[0] / c[3], c[1] / c[3]])
    }

    /// Column-major `f32` for the GPU.
    pub fn gpu(&self) -> [[f32; 4]; 4] {
        let mut out = [[0.0; 4]; 4];
        for (j, col) in out.iter_mut().enumerate() {
            for (i, v) in col.iter_mut().enumerate() {
                *v = self.m[i][j] as f32;
            }
        }
        out
    }
}

/// `shift` moves the picture up the plot by that much of its half-height (an off-axis
/// lens: the pose and its perspective stay, only the frame slides).
fn view_proj_at(pose: Pose, distance: f64, aspect: f64, shift: f64) -> ViewProj {
    let eye = add(TARGET, scale(pose.towards_eye(), distance));
    let f = normalize(sub(TARGET, eye));
    // Right-handed world seen with +x to the right of the screen when looking along +z:
    // right = up × forward.
    let r = normalize(cross([0.0, 1.0, 0.0], f));
    let u = cross(f, r);
    let fy = 1.0 / (FOV_Y_DEG.to_radians() / 2.0).tan();
    let fx = fy / aspect;
    let d = FAR / (FAR - NEAR);
    let row = |a: V3, k: f64, off: f64| [a[0] * k, a[1] * k, a[2] * k, -dot(a, eye) * k + off];
    ViewProj {
        m: [
            row(r, fx, 0.0),
            row(add(scale(u, fy), scale(f, shift)), 1.0, 0.0),
            row(f, d, -NEAR * d),
            row(f, 1.0, 0.0),
        ],
        eye,
    }
}

/// The box's lowest and highest corner in NDC y at `distance`, unshifted; `None` when a
/// corner is behind the near plane.
fn box_y_extent(pose: Pose, distance: f64, aspect: f64) -> Option<(f64, f64)> {
    let vp = view_proj_at(pose, distance, aspect, 0.0);
    box_corners()
        .iter()
        .try_fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
            let n = vp.ndc(*p)?;
            Some((lo.min(n[1]), hi.max(n[1])))
        })
}

/// The distance at zoom step 0 for `pose`'s angles in a plot of aspect `aspect`: every
/// corner of the box at full height fits with a 7 % margin, the box centred up and down
/// ([`lens_shift`]), so the front edge's tall spikes are never cut at the sides and a
/// steep or turned camera keeps the near floor corner in the picture. Fitted per angle, not
/// once at the default: from above and aside the box's projected outline is another shape.
/// The fit is continuous in the angles (each corner's distance to fit is), so a camera
/// easing through an orbit dollies smoothly with it.
pub fn fitted_distance(pose: Pose, aspect: f64) -> f64 {
    let pose = pose.clamped();
    let lim = 1.0 - FIT_MARGIN;
    let fits = |d: f64| {
        let vp = view_proj_at(pose, d, aspect, 0.0);
        box_corners()
            .iter()
            .all(|p| vp.ndc(*p).is_some_and(|n| n[0].abs() <= lim))
            && box_y_extent(pose, d, aspect).is_some_and(|(lo, hi)| {
                // The lens shift centres the box but moves it at most half the plot.
                let shift = (-(lo + hi) / 2.0).clamp(-SHIFT_MAX, SHIFT_MAX);
                lo + shift >= -lim && hi + shift <= lim
            })
    };
    let (mut lo, mut hi) = (0.1, 100.0);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        if fits(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

/// The lens shift's bound, in NDC: half the plot.
const SHIFT_MAX: f64 = 0.5;

/// The lens shift that centres the box up and down in the plot for `pose`'s angles: a low
/// camera sees the floor as a thin strip under tall spikes, and with the orbit target in
/// the middle of the floor the picture would sit high, a dead band under it. Taken at the
/// zoom-0 distance, so a dolly scales the picture about the same point.
pub fn lens_shift(pose: Pose, aspect: f64) -> f64 {
    let pose = pose.clamped();
    let d = eye_distance(
        Pose {
            zoom_step: 0.0,
            ..pose
        },
        aspect,
    );
    box_y_extent(pose, d, aspect).map_or(0.0, |(lo, hi)| {
        (-(lo + hi) / 2.0).clamp(-SHIFT_MAX, SHIFT_MAX)
    })
}

/// The eye's distance from the target: the dolly's, never inside the grown box. The stored
/// zoom step is left as set; only the distance drawn is held off.
pub fn eye_distance(pose: Pose, aspect: f64) -> f64 {
    let pose = pose.clamped();
    let dolly = fitted_distance(pose, aspect) * 2f64.powf(-pose.zoom_step / ZOOM_STEPS_PER_OCTAVE);
    dolly.max(exit_distance(pose.towards_eye()) + EYE_CLEARANCE)
}

/// The camera's view-projection for a plot of aspect `aspect`.
pub fn view_proj(pose: Pose, aspect: f64) -> ViewProj {
    let pose = pose.clamped();
    view_proj_at(
        pose,
        eye_distance(pose, aspect),
        aspect,
        lens_shift(pose, aspect),
    )
}

/// Whether `eye` is outside the skyline's box grown by `margin`.
pub fn outside_box(eye: V3, margin: f64) -> bool {
    (0..3).any(|a| eye[a] < BOX_MIN[a] - margin || eye[a] > BOX_MAX[a] + margin)
}

// ---------------------------------------------------------------------------------------
// Tier.

/// Settings › Display › 3D quality: this machine's, not the wire's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Quality {
    /// By the GPU ([`auto_tier`]).
    #[default]
    Auto,
    Full,
    Lite,
    Bare,
}

impl Quality {
    pub const ALL: [Quality; 4] = [Quality::Auto, Quality::Full, Quality::Lite, Quality::Bare];

    pub fn name(self) -> &'static str {
        match self {
            Quality::Auto => "auto",
            Quality::Full => "full",
            Quality::Lite => "lite",
            Quality::Bare => "bare",
        }
    }

    /// The tier drawn with `auto` the adapter's, and whether it was forced (the caption
    /// names a forced tier). `None` when the adapter has no tier: Settings cannot force a
    /// view the GPU cannot fill in time.
    pub fn tier(self, auto: Option<SkylineTier>) -> Option<(SkylineTier, bool)> {
        let auto = auto?;
        Some(match self {
            Quality::Auto => (auto, false),
            Quality::Full => (SkylineTier::Full, true),
            Quality::Lite => (SkylineTier::Lite, true),
            Quality::Bare => (SkylineTier::Bare, true),
        })
    }

    /// `d` places on in [`Self::ALL`], wrapping.
    pub fn step(self, d: i32) -> Self {
        let n = Self::ALL.len() as i32;
        let i = Self::ALL.iter().position(|q| *q == self).unwrap_or(0) as i32;
        Self::ALL[(i + d).rem_euclid(n) as usize]
    }
}

/// What the tier choice needs of the GPU adapter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AdapterFacts {
    /// A CPU rasterizer (llvmpipe, lavapipe, WARP).
    pub software: bool,
    pub name: String,
    pub max_texture_2d: u32,
    pub integrated: bool,
}

/// The tier for an adapter, by what it is, never by backend (a Pi runs GLES or Vulkan):
/// a Broadcom V3D → none, the 3D view is unavailable (its fill rate leaves even Bare 8–10×
/// over a frame); software → Bare; a 4096-texel limit or an integrated GPU other than
/// Apple's → Lite; otherwise Full.
pub fn auto_tier(a: &AdapterFacts) -> Option<SkylineTier> {
    let name = a.name.to_ascii_lowercase();
    if name.contains("v3d") {
        None
    } else if a.software {
        Some(SkylineTier::Bare)
    } else if a.max_texture_2d <= 4096 || (a.integrated && !name.contains("apple")) {
        Some(SkylineTier::Lite)
    } else {
        Some(SkylineTier::Full)
    }
}

// ---------------------------------------------------------------------------------------
// Axes on demand.

/// How long the axes stay after a touch, click, camera move or level-range change, s:
/// long enough to read a number off the height scale once the hand is off the screen.
pub const AXES_HOLD_S: f64 = 3.0;
/// Fade in, s: quick, so the scale answers the pointer at once.
pub const AXES_FADE_IN_S: f64 = 0.15;
/// Fade out, s: a little slower, so the axes leave without a flash.
pub const AXES_FADE_OUT_S: f64 = 0.25;

/// When the 3D view's axis decorations (labels, height scale, floor grid, ceiling line)
/// show. They cover the skyline, so they come on demand: while the pointer is over the
/// view, and for [`AXES_HOLD_S`] after an interaction (a kiosk's touch screen has no
/// hover). The UI reports the events with their times; [`AxesReveal::alpha`] says how much
/// they show at any later time.
///
/// The fade is a ramp at a fixed rate towards on or off, kept as the alpha and time of the
/// last event, so the alpha is a pure function of time between events: a second touch
/// while the axes fade in or out carries on from where they are, never jumps.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AxesReveal {
    hovered: bool,
    /// The last interaction's time, s.
    poke_s: Option<f64>,
    /// The alpha at the last event, and when that was.
    alpha0: f64,
    t0_s: f64,
}

impl AxesReveal {
    /// The pointer came over the view (`true`) or left it, at `now_s`.
    pub fn hover(&mut self, on: bool, now_s: f64) {
        if on != self.hovered {
            self.rebase(now_s);
            self.hovered = on;
        }
    }

    /// A touch, click, camera change or level-range change at `now_s`.
    pub fn poke(&mut self, now_s: f64) {
        self.rebase(now_s);
        self.poke_s = Some(now_s);
    }

    pub fn hovered(&self) -> bool {
        self.hovered
    }

    fn rebase(&mut self, now_s: f64) {
        self.alpha0 = self.ramp(now_s);
        self.t0_s = now_s;
    }

    /// When the axes turn off: never while hovered; else at the end of the hold.
    fn off_at(&self) -> f64 {
        if self.hovered {
            f64::INFINITY
        } else {
            self.poke_s
                .map_or(self.t0_s, |p| (p + AXES_HOLD_S).max(self.t0_s))
        }
    }

    /// The linear ramp: up from the last event until the off time, then down.
    fn ramp(&self, now_s: f64) -> f64 {
        let t = now_s.max(self.t0_s);
        let off = self.off_at();
        let up = (t.min(off) - self.t0_s) / AXES_FADE_IN_S;
        let peak = (self.alpha0 + up).min(1.0);
        if t <= off {
            peak
        } else {
            (peak - (t - off) / AXES_FADE_OUT_S).max(0.0)
        }
    }

    /// How much the axes show at `now_s`, 0 … 1, eased at both ends.
    pub fn alpha(&self, now_s: f64) -> f32 {
        let a = self.ramp(now_s);
        (a * a * (3.0 - 2.0 * a)) as f32
    }

    /// Whether the alpha still changes after `now_s`, and when: `Some(0.0)` while it fades,
    /// `Some(wait)` while it holds before fading out, `None` at rest.
    pub fn next_change(&self, now_s: f64) -> Option<f64> {
        let a = self.ramp(now_s);
        let off = self.off_at();
        if now_s < off {
            if a < 1.0 {
                Some(0.0)
            } else {
                off.is_finite().then_some(off - now_s)
            }
        } else {
            (a > 0.0).then_some(0.0)
        }
    }
}

// ---------------------------------------------------------------------------------------
// The scene.

/// What the 3D spectrograph shows: the history of the pane's measurement, its skyline and
/// how to see it.
#[derive(Clone, Debug)]
pub struct Spectrograph3dInput<'a> {
    pub history: &'a SpectrographHistory,
    /// Updated for this history, the pane's frequency span and `tier`.
    pub skyline: &'a Skyline,
    pub name: String,
    /// The level range the heights span (the pane's level axis for the history's scale).
    pub range: Range,
    /// The measurement's display offset, dB.
    pub offset_db: f64,
    pub freshness: Option<Freshness>,
    pub colours: Colormap,
    pub camera: Pose,
    pub tier: SkylineTier,
    /// The tier was set in Settings, not chosen by the adapter.
    pub tier_forced: bool,
    /// How much the axis decorations show, 0 … 1 ([`AxesReveal::alpha`]); what the tier
    /// in `view.chrome` would show, faded. An exported picture shows them whole (1).
    pub axes: f32,
}

/// Which axis a label belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelAxis {
    Frequency,
    Time,
    Level,
    /// The level unit at the top of the height scale.
    Unit,
}

/// A label placed by projecting its anchor through the camera.
#[derive(Clone, Debug, PartialEq)]
pub struct AxisLabel {
    pub axis: LabelAxis,
    pub text: String,
    pub pos: [f32; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spectrograph3dScene {
    pub scene: Scene,
    pub banners: Vec<crate::banner::BannerRow>,
    pub plot: Rect,
    pub caption: String,
    /// Shown in the plot instead of a picture.
    pub message: Option<String>,
    pub labels: Vec<AxisLabel>,
    /// The time axis's side: `true` right.
    pub time_right: bool,
    pub view_proj: ViewProj,
}

/// Floor x of frequency `hz` over `lo … hi`.
fn x_of(hz: f64, lo: f64, hi: f64) -> f64 {
    -FLOOR_HALF_W + 2.0 * FLOOR_HALF_W * (hz / lo).ln() / (hi / lo).ln()
}

/// Maps world points to the plot's pixels.
struct Projector {
    vp: ViewProj,
    plot: Rect,
}

impl Projector {
    fn px(&self, ndc: [f64; 2]) -> [f32; 2] {
        [
            self.plot.x + ((ndc[0] + 1.0) / 2.0) as f32 * self.plot.w,
            self.plot.y + ((1.0 - ndc[1]) / 2.0) as f32 * self.plot.h,
        ]
    }

    fn point(&self, p: V3) -> Option<[f32; 2]> {
        self.vp.ndc(p).map(|n| self.px(n))
    }

    fn inside(&self, p: [f32; 2]) -> bool {
        p[0] >= self.plot.x
            && p[0] <= self.plot.right()
            && p[1] >= self.plot.y
            && p[1] <= self.plot.bottom()
    }

    /// The segment `a`–`b` cut at the near plane; `None` when wholly behind it.
    fn segment(&self, a: V3, b: V3) -> Option<[[f32; 2]; 2]> {
        let (ca, cb) = (self.vp.clip(a), self.vp.clip(b));
        let (wa, wb) = (ca[3] - NEAR, cb[3] - NEAR);
        if wa < 0.0 && wb < 0.0 {
            return None;
        }
        let cut = |p: V3, q: V3, wp: f64, wq: f64| add(p, scale(sub(q, p), wp / (wp - wq)));
        let a = if wa < 0.0 { cut(a, b, wa, wb) } else { a };
        let b = if wb < 0.0 { cut(b, a, wb, wa) } else { b };
        Some([self.point(a)?, self.point(b)?])
    }

    /// The polygon cut at the near plane, in pixels.
    fn polygon(&self, pts: &[V3]) -> Vec<[f32; 2]> {
        let w = |p: V3| self.vp.clip(p)[3] - NEAR;
        let mut out = Vec::new();
        for (i, &p) in pts.iter().enumerate() {
            let q = pts[(i + 1) % pts.len()];
            let (wp, wq) = (w(p), w(q));
            if wp >= 0.0 {
                out.extend(self.point(p));
            }
            if (wp >= 0.0) != (wq >= 0.0) {
                out.extend(self.point(add(p, scale(sub(q, p), wp / (wp - wq)))));
            }
        }
        out
    }

    /// Projected length of `a`–`b`, pixels (0 when behind the eye).
    fn length(&self, a: V3, b: V3) -> f32 {
        self.segment(a, b).map_or(0.0, |[p, q]| {
            ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2)).sqrt()
        })
    }
}

fn mix(field: Color, light: Color, k: f32) -> Color {
    Color::rgb(
        (field.r + light.r * k).min(1.0),
        (field.g + light.g * k).min(1.0),
        (field.b + light.b * k).min(1.0),
    )
}

/// Generous box around a one-line label (no font metrics here): 0.62 em a character.
fn label_box(l: &Label) -> Rect {
    let w = canvas::text_width(&l.text, l.size);
    let h = l.size * 1.25;
    let x = match l.anchor.h {
        HAlign::Left => l.pos[0],
        HAlign::Center => l.pos[0] - w / 2.0,
        HAlign::Right => l.pos[0] - w,
    };
    let y = match l.anchor.v {
        VAlign::Top => l.pos[1],
        VAlign::Center => l.pos[1] - h / 2.0,
        _ => l.pos[1] - h,
    };
    Rect::new(x, y, w, h)
}

/// How much of the field a label's plate hides: enough to dim a spike behind the text, not
/// so much that the plate reads as a box.
pub const PLATE_ALPHA: f32 = 0.55;

/// A rounded plate around `b`, tightened to the text: [`label_box`] is generous across.
fn plate(b: Rect) -> Vec<[f32; 2]> {
    const R: f32 = 3.0;
    const SEGS: usize = 4;
    let inset = (b.w * 0.06).min(4.0);
    let (x0, x1) = (b.x + inset - 2.0, b.right() - inset + 2.0);
    let (y0, y1) = (b.y, b.bottom());
    let r = R.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0);
    let corners = [
        (x1 - r, y0 + r, -90.0f32),
        (x1 - r, y1 - r, 0.0),
        (x0 + r, y1 - r, 90.0),
        (x0 + r, y0 + r, 180.0),
    ];
    let mut out = Vec::with_capacity(4 * (SEGS + 1));
    for (cx, cy, a0) in corners {
        for i in 0..=SEGS {
            let a = (a0 + 90.0 * i as f32 / SEGS as f32).to_radians();
            out.push([cx + r * a.cos(), cy + r * a.sin()]);
        }
    }
    out
}

/// Labels kept where they do not overlap one kept before them, in the order given.
fn thin(candidates: Vec<(AxisLabel, Label)>, plot: Rect, out: &mut Vec<(AxisLabel, Label)>) {
    const GAP: f32 = 6.0;
    for c in candidates {
        let b = label_box(&c.1);
        // A label the field would cut is not drawn: half a number reads as another one.
        let whole = b.x >= plot.x
            && b.right() <= plot.right()
            && b.y >= plot.y
            && b.bottom() <= plot.bottom();
        let clear = whole
            && out.iter().all(|(_, k)| {
                let o = label_box(k);
                b.right() + GAP <= o.x
                    || o.right() + GAP <= b.x
                    || b.bottom() + 1.0 <= o.y
                    || o.bottom() + 1.0 <= b.y
            });
        if clear {
            out.push(c);
        }
    }
}

/// The spectrum pane's 3D spectrograph: the banners, a caption, and the skyline of `sg`'s
/// history on a dark field with its labels, height scale and ceiling line as `view.chrome`
/// says. `None` (no spectrum or RTA measurement) draws an empty field that says so.
pub fn spectrograph_3d_scene(
    traces: &[SpectrumTrace<'_>],
    status: &Status,
    sg: Option<&Spectrograph3dInput<'_>>,
    view: &ViewState,
    theme: &Theme,
    size: Viewport,
) -> Spectrograph3dScene {
    // The axes come on demand: at rest the view is the skyline alone, as T's traces-only
    // tier draws it; while shown they fade as a whole, each part what the tier would show.
    let axes = sg.map_or(1.0, |s| s.axes.clamp(0.0, 1.0));
    let chrome = if axes > 0.0 {
        view.chrome
    } else {
        PlotChrome::Bare
    };
    let right = crate::freq_column::right(RIGHT_MARGIN, view);
    let mut c0 = Canvas::new(size, theme);
    let plot_w = (size.width - MARGINS.left - right).max(1.0);
    let strip = canvas::banner_strip(&mut c0, status, MARGINS.left, plot_w, size, theme);
    let top = strip.rect.bottom() + MARGINS.top;
    let mut layers = c0.into_scene(size).layers;
    let plot = Rect::new(
        MARGINS.left,
        top + CAPTION_H,
        plot_w,
        (size.height - top - CAPTION_H - MARGINS.bottom).max(1.0),
    );
    let aspect = f64::from(plot.w / plot.h);
    let pose = sg.map_or(Pose::from(SpectrumCamera::DEFAULT), |s| s.camera);
    let vp = view_proj(pose, aspect);
    let pj = Projector { vp, plot };
    // The field and the labels in it keep the dark theme's colours in every theme.
    let dark = Theme::dark();
    let mut c = Canvas::default();
    c.base.rects.push(FillRect {
        rect: plot,
        color: FIELD,
        clip: None,
    });
    let colours = sg.map_or_else(Colormap::default, |s| s.colours);
    let mid = Color::from_rgba8(colormap::lut(colours)[colormap::LUT_SIZE / 2]);
    let depth_fade = |z: f64| (-f64::from(LOOK.age_fade) * z / FLOOR_DEPTH).exp() as f32;
    // Floor band: strips front to back, each lit by the depth fade at its front, under
    // the spikes (which are max-composited over it, so the brighter of the two shows).
    for i in 0..FLOOR_STRIPS {
        let z0 = FLOOR_DEPTH * i as f64 / FLOOR_STRIPS as f64;
        let z1 = FLOOR_DEPTH * (i + 1) as f64 / FLOOR_STRIPS as f64;
        let w = FLOOR_HALF_W;
        let points = pj.polygon(&[[-w, 0.0, z0], [w, 0.0, z0], [w, 0.0, z1], [-w, 0.0, z1]]);
        if points.len() >= 3 {
            c.base.polygons.push(FillPolygon {
                points,
                color: mix(FIELD, mid, FLOOR_BAND * depth_fade(z0)),
                clip: Some(plot),
            });
        }
    }
    if let Some(s) = pj.segment([-FLOOR_HALF_W, 0.0, 0.0], [FLOOR_HALF_W, 0.0, 0.0]) {
        c.base.polylines.push(Polyline {
            points: s.to_vec(),
            alpha: vec![],
            stroke: Stroke::solid(mix(FIELD, mid, NOW_LINE), 1.0),
            clip: Some(plot),
        });
    }

    let skyline = sg.map(|s| s.skyline);
    let span_hz = skyline.and_then(Skyline::span_hz);
    let span_s = sg.map_or(ac2_proto::model::SpectrumDisplay::DEFAULT_SPAN_S, |s| {
        s.history.span_s()
    });
    let range = sg.map_or(view.spectrum.level, |s| s.range);
    let time_right = pose.yaw_deg >= 0.0;
    let side_x = if time_right {
        FLOOR_HALF_W
    } else {
        -FLOOR_HALF_W
    };
    // Age (seconds) to floor depth, as the renderer places rows: a row of `k` slots
    // stands at its oldest edge, `R + 1` rows over the floor's depth.
    let z_of_age = |t: f64| {
        skyline
            .filter(|s| s.rows() > 0)
            .map_or(t / f64::from(span_s) * FLOOR_DEPTH, |s| {
                FLOOR_DEPTH * t / (s.row_s() * s.rows() as f64)
            })
    };

    let grid_stroke = Stroke::solid(dark.grid_major.color.with_alpha(0.5 * axes), 1.0);
    let mut cands = Vec::new();
    let small = theme.small_font_size;
    if let Some((lo, hi)) = span_hz {
        let front = pj.length([-FLOOR_HALF_W, 0.0, 0.0], [FLOOR_HALF_W, 0.0, 0.0]);
        let fax = axis::freq_axis(Range::new(lo, hi), 0.0, front.max(1.0));
        let decade = |v: f64| (v.log10() - v.log10().round()).abs() < 1e-9;
        let mut ticks: Vec<_> = fax
            .ticks
            .iter()
            .filter(|t| t.label.is_some() && t.value >= lo && t.value <= hi)
            .collect();
        // Decades first: thinning keeps them over the 2 and 5 between.
        ticks.sort_by_key(|t| !decade(t.value));
        if chrome.grid() {
            for t in fax.ticks.iter().filter(|t| decade(t.value)) {
                let x = x_of(t.value, lo, hi);
                if let Some(s) = pj.segment([x, 0.0, 0.0], [x, 0.0, FLOOR_DEPTH]) {
                    c.base.polylines.push(Polyline {
                        points: s.to_vec(),
                        alpha: vec![],
                        stroke: grid_stroke,
                        clip: Some(plot),
                    });
                }
            }
        }
        for t in ticks {
            let Some(p) = pj.point([x_of(t.value, lo, hi), 0.0, 0.0]) else {
                continue;
            };
            let text = t.label.clone().unwrap_or_default();
            let l = label(
                text.clone(),
                [p[0], p[1] + 4.0],
                anchor(HAlign::Center, VAlign::Top),
                small,
                dark.axis_text,
            );
            if pj.inside(p) {
                cands.push((
                    AxisLabel {
                        axis: LabelAxis::Frequency,
                        text,
                        pos: l.pos,
                    },
                    l,
                ));
            }
        }
    }
    // Time along the side nearer the eye.
    let side = pj.length([side_x, 0.0, 0.0], [side_x, 0.0, FLOOR_DEPTH]);
    let tax = axis::linear_axis(Range::new(0.0, f64::from(span_s)), 0.0, side.max(1.0), "s");
    let mut times: Vec<(f64, String)> = vec![(0.0, "now".to_string())];
    times.extend(
        tax.ticks
            .iter()
            .filter(|t| t.label.is_some() && t.value > 0.0 && t.value <= f64::from(span_s))
            .map(|t| {
                (
                    t.value,
                    format!("−{} s", t.label.clone().unwrap_or_default()),
                )
            }),
    );
    for (t, text) in times {
        let z = z_of_age(t);
        if z > FLOOR_DEPTH + 1e-9 {
            continue;
        }
        if chrome.grid()
            && t > 0.0
            && let Some(s) = pj.segment([-FLOOR_HALF_W, 0.0, z], [FLOOR_HALF_W, 0.0, z])
        {
            c.base.polylines.push(Polyline {
                points: s.to_vec(),
                alpha: vec![],
                stroke: grid_stroke,
                clip: Some(plot),
            });
        }
        let Some(p) = pj.point([side_x, 0.0, z]) else {
            continue;
        };
        let (h, dx) = if time_right {
            (HAlign::Left, 6.0)
        } else {
            (HAlign::Right, -6.0)
        };
        let l = label(
            text.clone(),
            [p[0] + dx, p[1]],
            anchor(h, VAlign::Center),
            small,
            dark.axis_text,
        );
        if pj.inside(p) {
            cands.push((
                AxisLabel {
                    axis: LabelAxis::Time,
                    text,
                    pos: l.pos,
                },
                l,
            ));
        }
    }
    // Time before frequency: "now" shares the front corner with the top decade's label, and
    // the corner says where the newest row is.
    cands.sort_by_key(|(a, _)| a.axis != LabelAxis::Time);
    // The height scale: a post at the front corner nearer the eye, floor to range top.
    let ceiling_stroke = Stroke {
        color: dark.text_dim.with_alpha(0.45 * axes),
        width: 1.0,
        dash: Some(Dash {
            on: 4.0,
            off: 4.0,
            offset: 0.0,
        }),
    };
    let post_stroke = Stroke::solid(dark.axis_text.with_alpha(0.7 * axes), 1.0);
    let unit = sg
        .and_then(|s| s.history.unit())
        .map_or(String::new(), str::to_string);
    if chrome.labels() && range.is_valid() {
        if let Some(s) = pj.segment([side_x, 0.0, 0.0], [side_x, MAX_HEIGHT, 0.0]) {
            c.data.polylines.push(Polyline {
                points: s.to_vec(),
                alpha: vec![],
                stroke: post_stroke,
                clip: Some(plot),
            });
        }
        if let Some(s) = pj.segment(
            [-FLOOR_HALF_W, MAX_HEIGHT, FLOOR_DEPTH],
            [FLOOR_HALF_W, MAX_HEIGHT, FLOOR_DEPTH],
        ) {
            c.data.polylines.push(Polyline {
                points: s.to_vec(),
                alpha: vec![],
                stroke: ceiling_stroke,
                clip: Some(plot),
            });
        }
        let post = pj.length([side_x, 0.0, 0.0], [side_x, MAX_HEIGHT, 0.0]);
        let lax = axis::linear_axis(range, 0.0, post.max(1.0), &unit);
        let mut ticks = Vec::new();
        for t in lax.ticks.iter().filter(|t| t.label.is_some()) {
            let y = MAX_HEIGHT * (t.value - range.lo) / range.span();
            // A tick on the floor would sit on the time axis's "now"; the floor reads the
            // range's bottom without it.
            if !(MAX_HEIGHT * 0.05..=MAX_HEIGHT + 1e-9).contains(&y) {
                continue;
            }
            let (Some(p), Some(q)) = (
                pj.point([side_x, y, 0.0]),
                pj.point([side_x * 1.03, y, 0.0]),
            ) else {
                continue;
            };
            c.data.polylines.push(Polyline {
                points: vec![p, q],
                alpha: vec![],
                stroke: post_stroke,
                clip: Some(plot),
            });
            ticks.push((p, q, t.label.clone().unwrap_or_default()));
        }
        // The numbers stand outside the post, past its ticks; where the field's edge would
        // cut one, all of them stand inside it instead, so the scale reads on one side.
        let place = |inward: bool, p: [f32; 2], q: [f32; 2], text: &str| {
            let (h, x) = match (time_right, inward) {
                (true, false) => (HAlign::Left, q[0] + 5.0),
                (false, false) => (HAlign::Right, q[0] - 5.0),
                (true, true) => (HAlign::Right, p[0] - 5.0),
                (false, true) => (HAlign::Left, p[0] + 5.0),
            };
            label(
                text.to_string(),
                [x, q[1]],
                anchor(h, VAlign::Center),
                small,
                dark.axis_text,
            )
        };
        let inward = ticks.iter().any(|(p, q, text)| {
            let b = label_box(&place(false, *p, *q, text));
            b.x < plot.x || b.right() > plot.right()
        });
        let mut level = Vec::new();
        for (p, q, text) in ticks {
            let l = place(inward, p, q, &text);
            if pj.inside(p) {
                level.push((
                    AxisLabel {
                        axis: LabelAxis::Level,
                        text,
                        pos: l.pos,
                    },
                    l,
                ));
            }
        }
        if let Some(p) = pj.point([side_x, MAX_HEIGHT, 0.0])
            && pj.inside(p)
            && !unit.is_empty()
        {
            // High enough that the top tick's number, centred on the post's top, clears it.
            let l = label(
                unit.clone(),
                [p[0], p[1] - 9.0],
                anchor(HAlign::Center, VAlign::Bottom),
                small,
                dark.text_dim,
            );
            level.insert(
                0,
                (
                    AxisLabel {
                        axis: LabelAxis::Unit,
                        text: unit.clone(),
                        pos: l.pos,
                    },
                    l,
                ),
            );
        }
        // The level scale's own labels first: its unit and ticks read the heights.
        let mut kept = Vec::new();
        thin(level, plot, &mut kept);
        thin(cands, plot, &mut kept);
        cands = kept;
    } else if !chrome.labels() {
        cands.clear();
    } else {
        let mut kept = Vec::new();
        thin(cands, plot, &mut kept);
        cands = kept;
    }

    let mut message = None;
    let mut caption = String::new();
    match sg {
        None => message = Some("no spectrum or RTA measurement".to_string()),
        Some(s) => {
            let name = if s.offset_db != 0.0 {
                offset_note(&s.name, s.offset_db)
            } else {
                s.name.clone()
            };
            caption = format!("{name} · last {span_s} s");
            if let Some(unit) = s.history.unit_noted() {
                caption.push_str(&format!(" · {unit}"));
            }
            if let Some(tag) = s.freshness.and_then(|f| f.tag()) {
                caption.push_str(" · ");
                caption.push_str(&tag);
            }
            let sk = s.skyline;
            if s.history.is_empty() || sk.span_hz().is_none() || !range.is_valid() {
                message = Some(format!("{}: no frames yet", s.name));
            } else {
                let off = s.offset_db;
                c.data.skylines.push(Skyline3d {
                    id: SKYLINE_ID,
                    rect: plot,
                    clip: Some(plot),
                    cols: sk.cols() as u32,
                    rows: sk.rows() as u32,
                    newest: sk.newest() as u32,
                    phase: sk.phase(),
                    view_proj: vp.gpu(),
                    eye: vp.eye.map(|v| v as f32),
                    world: [FLOOR_HALF_W as f32, FLOOR_DEPTH as f32, MAX_HEIGHT as f32],
                    // Heights are of the displayed (offset) level.
                    range: [(range.lo - off) as f32, (range.hi - off) as f32],
                    colormap: s.colours,
                    tier: s.tier,
                    look: SkylineLook {
                        width_px: match s.tier {
                            SkylineTier::Full => LOOK.width_px,
                            SkylineTier::Lite | SkylineTier::Bare => LITE_WIDTH_PX,
                        },
                        ..LOOK
                    },
                    data: sk.ring().to_vec(),
                });
                if s.tier.bloom_taps().is_some() {
                    c.data.glows.push(SkylineGlow {
                        id: SKYLINE_ID,
                        clip: Some(plot),
                        gain: glow_gain(s.colours),
                    });
                }
            }
        }
    }
    let cell = sg.and_then(|s| cell_text(s.skyline));
    let tier = sg
        .filter(|s| s.tier_forced)
        .map(|s| tier_name(s.tier).to_string());
    let tail: Vec<String> = cell.into_iter().chain(tier).collect();
    let with_tail = |base: &str| {
        std::iter::once(base.to_string())
            .chain(tail.iter().cloned())
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    };
    let mut forms = Vec::new();
    if !caption.is_empty()
        && let Some(cal) = traces.first().map(|t| t.caption.as_str())
        && let cal = cal.strip_suffix(" · stopped").unwrap_or(cal)
        && !cal.is_empty()
    {
        for f in crate::spectrum::caption_forms(cal) {
            if !f.is_empty() {
                forms.push(with_tail(&format!("{caption} · {f}")));
            }
        }
    }
    let own = with_tail(&caption);
    forms.extend([own.clone(), caption.clone(), String::new()]);
    let shown = forms
        .into_iter()
        .find(|c| canvas::text_width(c, small) <= plot.w)
        .unwrap_or_default();
    let caption = if caption.is_empty() {
        caption
    } else {
        shown.clone()
    };
    c.overlay.labels.push(label(
        shown,
        [plot.x, plot.y - 3.0],
        anchor(HAlign::Left, VAlign::Bottom),
        small,
        theme.text_dim,
    ));
    if let Some(m) = &message {
        c.overlay.labels.push(label(
            m.clone(),
            [plot.x + plot.w / 2.0, plot.y + plot.h / 2.0],
            anchor(HAlign::Center, VAlign::Center),
            small,
            dark.text_dim,
        ));
    }
    let labels = if chrome == PlotChrome::Bare {
        Vec::new()
    } else {
        cands
    };
    let axis_labels = labels.iter().map(|(a, _)| a.clone()).collect();
    // Over the spikes and their bloom (the overlay draws after both): each label on a
    // faint dark plate, so a number over a bright spike still reads.
    for (_, l) in &labels {
        c.overlay.polygons.push(FillPolygon {
            points: plate(label_box(l)),
            color: FIELD.with_alpha(PLATE_ALPHA * axes),
            clip: Some(plot),
        });
    }
    c.overlay
        .labels
        .extend(labels.into_iter().map(|(_, mut l)| {
            l.clip = Some(plot);
            l.color = l.color.with_alpha(axes);
            l
        }));
    layers.extend(c.into_scene(size).layers.into_iter().take(3));
    Spectrograph3dScene {
        scene: Scene {
            viewport: size,
            layers,
        },
        banners: strip.rows,
        plot,
        caption,
        message,
        labels: axis_labels,
        time_right,
        view_proj: vp,
    }
}

/// `1/19 oct · 0.30 s`: the cell the picture is made of.
pub fn cell_text(s: &Skyline) -> Option<String> {
    let (lo, hi) = s.span_hz()?;
    let per_oct = s.cols() as f64 / (hi / lo).log2();
    let freq = if per_oct >= 1.5 {
        format!("1/{} oct", per_oct.round())
    } else {
        format!("{:.1} oct", 1.0 / per_oct)
    };
    Some(format!("{freq} · {:.2} s", s.row_s()))
}

/// What the caption calls a forced tier.
pub fn tier_name(t: SkylineTier) -> &'static str {
    match t {
        SkylineTier::Full => "full",
        SkylineTier::Lite => "lite",
        SkylineTier::Bare => "bare",
    }
}

#[cfg(test)]
#[path = "spectrograph_3d_tests.rs"]
mod tests;
