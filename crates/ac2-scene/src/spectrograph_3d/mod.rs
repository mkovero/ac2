//! 3D spectrograph: the spectrograph's history seen as a skyline, level as height
//! (`docs/design/spectrograph-3d.md`).
//!
//! Frequency runs left to right across a dark floor, time from the front edge (newest)
//! back into the distance, and on every cell of a decimated grid stands one thin spike
//! whose height is the cell's level within the pane's level range. Rows behind show through
//! the gaps; nothing hides anything, so a pointer over the picture names no cell by itself:
//! a click picks the spike nearest it, the nearer the eye of two as near ([`pick()`]), and
//! the cursor is that cell, read on the cursor values' plate as on every view.
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
use crate::trace::TraceKey;
use crate::view::{PlotChrome, ViewState};

type V3 = [f64; 3];

mod camera;
mod cursor;
mod pick;
mod points;
mod scene;
pub use camera::*;
pub use pick::{PICK_PX, PickFrame, Picked, Pointer3d, cell_at, col_hz, pick, row_age};
pub use points::PeakPoint3d;
pub use scene::*;

/// Floor half-width (frequency, x in −W … W): wider than deep keeps ten octaves readable
/// from the default camera.
pub const FLOOR_HALF_W: f64 = 1.0;
/// Floor depth (age, z in 0 … D, newest at 0).
pub const FLOOR_DEPTH: f64 = 1.2;
/// Height of a spike at the top of the level range: tall enough to rise above the far rows
/// without leaving the frame.
pub const MAX_HEIGHT: f64 = 0.7;
/// The tallest spike, in [`MAX_HEIGHT`]s: a level two spans above the range top. Above the
/// top a spike keeps the same dB per unit height (the camera can stop and look at it), but
/// a broken frame (a wild level, a mis-set range) must not stand a spike off past every
/// camera limit and swamp the picture; two spans above is more than any real level above
/// a range the operator chose.
pub const HEIGHT_CAP: f64 = 3.0;

/// A level `v` as the height of its spike, in [`MAX_HEIGHT`]s, over the level range
/// `lo..hi`: 0 at or under the range bottom (no spike), 1 at its top, in proportion past
/// it up to [`HEIGHT_CAP`]. The renderer's `skyline.wgsl` stands spikes by the same rule.
pub fn spike_height(v: f64, lo: f64, hi: f64) -> f64 {
    ((v - lo) / (hi - lo)).clamp(0.0, HEIGHT_CAP)
}

/// How far a spike stands off its cell's centre at most, in cell pitches along each axis.
/// On a regular lattice every lattice line whose vertical plane holds the eye projects all
/// its spikes onto one screen line; max blending shows that stack as one spike, so the line
/// reads as a dark stripe that wanders with the camera. Scattered within the cell the
/// spikes leave every such plane; under half a pitch keeps each strictly inside its cell.
pub const SCATTER: f32 = 0.35;

/// A full-avalanche 32-bit mix (Wellons' lowbias32): neighbouring cells get unrelated
/// offsets, so the scatter is white within each cell (a stratified, blue-ish pattern) and
/// lays no new lattice. `skyline.wgsl` has the same function.
pub const fn mix32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

/// The spike of column `col` on row `row_id` (the row's absolute bucket number, wrapped to
/// 32 bits: a row keeps its offsets as it moves back): its offset from the cell's centre
/// along x and z, cell pitches, each within ±[`SCATTER`]. Display only: the spike still
/// reads its column's frequency and its row's age. `skyline.wgsl` computes the same at the
/// look's `scatter` ([`LOOK`]), every step exact in f32 but the last multiply, which IEEE
/// rounds alike everywhere.
pub fn scatter(col: u32, row_id: u32) -> [f32; 2] {
    let h = mix32(col ^ mix32(row_id));
    let u = |v: u32| ((v >> 8) as f32 * (1.0 / 16_777_216.0) * 2.0 - 1.0) * SCATTER;
    [u(h), u(mix32(h))]
}

/// Where the spike of column `col`, `j` rows back, stands on the floor, in a skyline of
/// `cols` columns and `rows` ring rows whose newest row stands `phase` rows back and is
/// row `newest_id` ([`DrawnRows`]): its cell's centre moved by [`scatter`]. The renderer,
/// the pick, the cursor and the peak marks all stand spikes here.
pub fn spike_foot(
    cols: usize,
    rows: usize,
    (col, j): (usize, usize),
    phase: f64,
    newest_id: u32,
) -> V3 {
    let [dx, dz] = scatter(col as u32, newest_id.wrapping_sub(j as u32));
    let pitch = 2.0 * FLOOR_HALF_W / cols.max(1) as f64;
    [
        -FLOOR_HALF_W + (col as f64 + 0.5 + f64::from(dx)) * pitch,
        0.0,
        (j as f64 + phase + f64::from(dz)) * FLOOR_DEPTH / rows.max(1) as f64,
    ]
}

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
    scatter: SCATTER,
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
    /// The history's revision when last folded: with nothing new, nothing is folded, so a
    /// picture redrawn between frames (a glide) neither recomputes rows nor hands the
    /// renderer new ones to upload.
    revision: Option<u64>,
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
                revision: None,
                slot_s: h.slot_s(),
            };
        }
        if self.revision != Some(h.revision()) {
            self.fold(h);
            self.revision = Some(h.revision());
        }
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

    /// The newest row's absolute bucket number, wrapped to 32 bits: row `j` back is
    /// `newest_id − j` ([`spike_foot`]).
    pub fn newest_id(&self) -> u32 {
        self.newest.map_or(0, |n| (n / self.k.max(1)) as u32)
    }

    /// Ring position of the newest row.
    pub fn newest(&self) -> usize {
        self.newest.map_or(0, |n| {
            ((n / self.k) % self.ring.len().max(1) as u64) as usize
        })
    }

    /// The newest row's fill fraction (slots filled ÷ slots per row), in (0, 1]: how far
    /// back the rows stand when the time axis is at the end of the newest slot. From the
    /// data, so a stalled stream stands still.
    pub fn phase(&self) -> f32 {
        self.newest
            .map_or(1.0, |n| ((n % self.k) + 1) as f32 / self.k as f32)
    }

    /// Where the rows are drawn with the time axis `lead_slots` history slots past the end
    /// of the newest slot ([`SpectrographHistory::lead_slots`]; 0: at their places,
    /// [`Self::phase`]). The axis runs on the display's clock
    /// ([`crate::scroll_clock`]), so the rows move back the same distance every display
    /// frame however unevenly frames bring the audio; drawn at their places they would
    /// jump back by whole slots at each frame, which the eye reads as a stutter.
    ///
    /// Inside the newest slot the newest row stands nearer the front edge, at most to it;
    /// past the slot, while the next frame is due, further back than one row, the strip in
    /// front of it the time the next frame will fill.
    pub fn drawn(&self, lead_slots: f64) -> DrawnRows {
        let lead = if lead_slots.is_finite() {
            lead_slots
        } else {
            0.0
        };
        let p = f64::from(self.phase()) + lead / self.k.max(1) as f64;
        DrawnRows {
            newest: self.newest(),
            newest_id: self.newest_id(),
            phase: p.clamp(0.0, MAX_PHASE) as f32,
        }
    }

    /// The frequency span the columns cover, Hz.
    pub fn span_hz(&self) -> Option<(f64, f64)> {
        self.key
            .as_ref()
            .filter(|_| !self.cols.is_empty())
            .map(|k| (k.lo, k.hi))
    }

    /// Audio time at the end of the newest slot when last folded, seconds
    /// ([`SpectrographHistory::newest_end_s`]).
    pub fn newest_end_s(&self) -> Option<f64> {
        self.newest.map(|n| (n + 1) as f64 * self.slot_s)
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

/// Farthest back the front row is drawn, rows ([`Skyline::drawn`]): a row and the
/// [`crate::spectrograph::LEAD_SLOTS`] a stalled stream waits at, at one slot per row.
pub const MAX_PHASE: f64 = 1.0 + crate::spectrograph::LEAD_SLOTS.1;

/// The rows as drawn at one moment of the scroll ([`Skyline::drawn`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawnRows {
    /// Ring position of the front (newest) row.
    pub newest: usize,
    /// Its absolute bucket number, wrapped ([`Skyline::newest_id`]).
    pub newest_id: u32,
    /// How far the front row stands back, in rows, in [0, [`MAX_PHASE`]]: the renderer's
    /// phase.
    pub phase: f32,
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

#[cfg(test)]
mod tests;
