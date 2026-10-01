//! Plain-data scene primitives consumed by the renderer (`ac2-plot`).
//!
//! Everything here is already mapped to logical pixels by the scene builders (origin
//! top-left, y down); the renderer only scales by the target's pixels-per-point, snaps where
//! the primitive asks for it, and rasterizes. No GPU types, no serde.

/// Colour as display-encoded (sRGB transfer, like egui and CSS) components in `0..=1` with
/// straight (not premultiplied) alpha. Blending happens on these encoded values when the
/// target format is not sRGB, which is what egui does too.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Color = Color::rgba(0.0, 0.0, 0.0, 0.0);
    pub const BLACK: Color = Color::rgb(0.0, 0.0, 0.0);
    pub const WHITE: Color = Color::rgb(1.0, 1.0, 1.0);

    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self::rgba(r, g, b, 1.0)
    }

    pub fn from_rgba8(c: [u8; 4]) -> Self {
        let f = |v: u8| f32::from(v) / 255.0;
        Self::rgba(f(c[0]), f(c[1]), f(c[2]), f(c[3]))
    }

    /// Same colour with alpha multiplied by `k`.
    pub fn with_alpha(self, k: f32) -> Self {
        Self {
            a: self.a * k,
            ..self
        }
    }

    pub fn to_rgba8(self) -> [u8; 4] {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        [q(self.r), q(self.g), q(self.b), q(self.a)]
    }
}

/// Axis-aligned rectangle in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// Size of the scene in logical pixels; the scene's coordinate system spans
/// `(0,0)..(width,height)` and nothing is drawn outside it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub width: f32,
    pub height: f32,
}

/// Dash pattern in logical pixels along the stroke, butt-ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dash {
    pub on: f32,
    pub off: f32,
    /// Arc length at which the pattern starts (shifts dashes along the line).
    pub offset: f32,
}

/// How a line is stroked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stroke {
    pub color: Color,
    /// Logical pixels. Strokes thinner than one physical pixel are drawn one pixel wide with
    /// alpha scaled by the width, so they fade instead of breaking up.
    pub width: f32,
    pub dash: Option<Dash>,
}

impl Stroke {
    pub const fn solid(color: Color, width: f32) -> Self {
        Self {
            color,
            width,
            dash: None,
        }
    }
}

/// Polyline with round joins and caps.
///
/// A non-finite point breaks the line: the segments touching it are not drawn and the
/// neighbours get round caps, so the scene can encode gaps without splitting the line.
#[derive(Clone, Debug, PartialEq)]
pub struct Polyline {
    /// Logical pixels.
    pub points: Vec<[f32; 2]>,
    /// Per-vertex opacity multiplier in `0..=1` (e.g. coherence blanking), interpolated
    /// along each segment. Empty means fully opaque; otherwise one per point.
    pub alpha: Vec<f32>,
    pub stroke: Stroke,
    /// Nothing outside this rectangle is drawn (typically the plot rect).
    pub clip: Option<Rect>,
}

/// One sample of a filled band: the two envelope edges at `x`; order of `y0`/`y1` does not
/// matter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandPoint {
    pub x: f32,
    pub y0: f32,
    pub y1: f32,
}

/// Filled area between two edges sampled at increasing `x` (a min/max envelope).
/// Pairs of samples whose `x` does not increase, or that contain a non-finite value, leave
/// a gap.
#[derive(Clone, Debug, PartialEq)]
pub struct Band {
    pub points: Vec<BandPoint>,
    pub color: Color,
    pub clip: Option<Rect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridAxis {
    /// Vertical line at a given x.
    X,
    /// Horizontal line at a given y.
    Y,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridKind {
    Major,
    Minor,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridLine {
    pub axis: GridAxis,
    /// Logical x (for [`GridAxis::X`]) or y (for [`GridAxis::Y`]).
    pub pos: f32,
    pub kind: GridKind,
}

/// Grid lines spanning `rect`. Positions are snapped to the physical pixel grid so a
/// whole-pixel-wide line is crisp; lines are clipped to `rect`.
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub rect: Rect,
    pub lines: Vec<GridLine>,
    pub major: Stroke,
    pub minor: Stroke,
}

/// Filled rectangle (backgrounds, banners).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FillRect {
    pub rect: Rect,
    pub color: Color,
    pub clip: Option<Rect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HAlign {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VAlign {
    /// `pos` is the top of the line box.
    Top,
    /// `pos` is the middle of the line box.
    Center,
    /// `pos` is on the baseline of the first line.
    Baseline,
    /// `pos` is the bottom of the line box.
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub h: HAlign,
    pub v: VAlign,
}

impl Anchor {
    pub const TOP_LEFT: Anchor = Anchor {
        h: HAlign::Left,
        v: VAlign::Top,
    };
    pub const CENTER: Anchor = Anchor {
        h: HAlign::Center,
        v: VAlign::Center,
    };
}

/// Text in the bundled font, single line per `\n`-separated row.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub text: String,
    /// Logical pixels; which point of the text box sits here is set by `anchor`.
    pub pos: [f32; 2],
    pub anchor: Anchor,
    /// Font size (em height) in logical pixels.
    pub size: f32,
    pub color: Color,
    pub clip: Option<Rect>,
}

/// Identity of a heatmap's GPU texture across frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HeatmapId(pub u64);

/// Colour lookup for heatmap values; all are perceptually uniform and colorblind-safe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Colormap {
    Viridis,
}

/// Values written into heatmap columns `first..first + n`, where
/// `n = values.len() / rows`. `values` holds whole columns one after another, each from
/// row 0 (bottom of the rect) upwards. A range that runs past the last column wraps to
/// column 0. NaN marks a cell without data (drawn transparent).
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnUpload {
    pub first: u32,
    pub values: Vec<f32>,
}

/// Scrolling value texture (spectrograph): a ring of `columns` columns of `rows` values,
/// drawn across `rect` with columns evenly spaced left to right and rows evenly spaced
/// bottom to top; the scene layer has already resampled onto that even grid.
///
/// The renderer keeps the ring between frames under `id`: `uploads` carries only the
/// columns that changed since the previous frame. A new `id` (or new dimensions) starts
/// with all cells empty; an id absent from a scene is released.
#[derive(Clone, Debug, PartialEq)]
pub struct Heatmap {
    pub id: HeatmapId,
    pub rect: Rect,
    pub clip: Option<Rect>,
    pub columns: u32,
    pub rows: u32,
    /// Ring column drawn at the left edge of `rect`; scrolling advances this.
    pub scroll: u32,
    /// Values mapped to the first and last colormap entry; values outside clamp.
    pub range: [f32; 2],
    pub colormap: Colormap,
    /// Overall opacity.
    pub opacity: f32,
    pub uploads: Vec<ColumnUpload>,
}

/// Primitives drawn in a fixed order: rects, heatmaps, bands, grids, polylines, labels.
/// Anything that must cover text (a banner over the plot) goes into a later layer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layer {
    pub rects: Vec<FillRect>,
    pub heatmaps: Vec<Heatmap>,
    pub bands: Vec<Band>,
    pub grids: Vec<Grid>,
    pub polylines: Vec<Polyline>,
    pub labels: Vec<Label>,
}

/// A frame's worth of drawing, in logical pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub viewport: Viewport,
    pub layers: Vec<Layer>,
}
