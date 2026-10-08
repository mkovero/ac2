//! A plot's legend box: its rows on a plate in the plot's own colour, so they read over any
//! number of curves running behind them; placed where the operator dragged it (or snapped to
//! a corner), at most as large as they allowed, and scrolled when the rows do not all fit.
//! The cursor readout beside it sits on the same kind of plate.

use std::ops::RangeInclusive;

use crate::canvas::{anchor, cut_to, label, text_width};
use crate::primitives::{Color, FillRect, HAlign, Layer, Rect, VAlign};
use crate::theme::Theme;

/// Row pitch.
pub const ROW: f32 = 16.0;
/// Padding inside a plate.
pub const PAD_X: f32 = 6.0;
pub const PAD_Y: f32 = 4.0;
/// Distance a plate keeps from the plot's edges, so its border never merges with the frame.
pub const INSET: f32 = 4.0;
/// Width of the resize grip's square at the plate's bottom-right corner.
pub const GRIP: f32 = 12.0;
/// Bounds of the size limits, fractions of the plot: below the lower one a row would not
/// hold a name.
pub const SIZE_LIMITS: RangeInclusive<f32> = 0.15..=1.0;
/// Colour swatch of a row and the gap after it.
const SWATCH_W: f32 = 12.0;
const SWATCH_GAP: f32 = 6.0;
/// Opacity of a plate: text stays legible over a dense bundle of curves while the curves
/// behind still show that they continue under it.
const PLATE_ALPHA: f32 = 0.88;

/// Corners the legend snaps to from the command palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegendCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl LegendCorner {
    pub const ALL: [Self; 4] = [
        Self::TopLeft,
        Self::TopRight,
        Self::BottomLeft,
        Self::BottomRight,
    ];

    /// Its place as [`LegendView::x`], [`LegendView::y`].
    pub fn position(self) -> [f32; 2] {
        match self {
            Self::TopLeft => [0.0, 0.0],
            Self::TopRight => [1.0, 0.0],
            Self::BottomLeft => [0.0, 1.0],
            Self::BottomRight => [1.0, 1.0],
        }
    }
}

/// What the pointer is over: the plate moves it, the grip resizes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegendHover {
    Plate,
    Grip,
}

/// The operator's legend choices.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LegendView {
    pub hidden: bool,
    /// Where the plate sits in the room the plot leaves beside it, 0 at the left edge and 1
    /// at the right: a fraction, so a legend in a corner stays in it when the pane resizes.
    pub x: f32,
    /// As `x`, 0 at the top and 1 at the bottom.
    pub y: f32,
    /// Widest the plate may be, a fraction of the plot's width ([`SIZE_LIMITS`]); longer rows
    /// are cut with `…`.
    pub max_width: f32,
    /// Tallest the plate may be, a fraction of the plot's height; rows beyond it scroll.
    pub max_height: f32,
    /// First row shown when the rows scroll.
    pub first: usize,
    pub hover: Option<LegendHover>,
}

impl Default for LegendView {
    fn default() -> Self {
        Self {
            hidden: false,
            x: 0.0,
            y: 0.0,
            max_width: 0.6,
            max_height: 0.7,
            first: 0,
            hover: None,
        }
    }
}

impl LegendView {
    /// Shows it in corner `c`.
    pub fn snap(&mut self, c: LegendCorner) {
        [self.x, self.y] = c.position();
        self.hidden = false;
    }

    /// The corner it sits in, when it sits in one.
    pub fn corner(&self) -> Option<LegendCorner> {
        LegendCorner::ALL
            .into_iter()
            .find(|c| c.position() == [self.x, self.y])
    }

    /// Moves it to `x`, `y` ([`LegendBox::position_for`]), kept inside the plot.
    pub fn move_to(&mut self, x: f32, y: f32) {
        if x.is_finite() && y.is_finite() {
            self.x = x.clamp(0.0, 1.0);
            self.y = y.clamp(0.0, 1.0);
        }
    }

    /// Sets its size limits ([`LegendBox::limits_for`]), within [`SIZE_LIMITS`].
    pub fn resize(&mut self, max_width: f32, max_height: f32) {
        let (lo, hi) = (*SIZE_LIMITS.start(), *SIZE_LIMITS.end());
        if max_width.is_finite() && max_height.is_finite() {
            self.max_width = max_width.clamp(lo, hi);
            self.max_height = max_height.clamp(lo, hi);
        }
    }
}

/// Where the legend went and what it shows.
#[derive(Clone, Debug, PartialEq)]
pub struct LegendBox {
    /// What it moves within: the plot below its title.
    pub area: Rect,
    /// The plate.
    pub rect: Rect,
    /// The resize grip, inside the plate's bottom-right corner.
    pub grip: Rect,
    /// Rows `first .. first + shown` of `total` are shown.
    pub first: usize,
    pub shown: usize,
    pub total: usize,
    /// The shown rows' text as drawn (cut to the width) and their centre y.
    pub rows: Vec<(String, f32)>,
    /// `3 above · 9 below` when not every row fits.
    pub more: Option<String>,
    /// Lines after the rows (the reference's delay), as drawn.
    pub notes: Vec<String>,
}

impl LegendBox {
    /// [`LegendView::x`], [`LegendView::y`] that put the plate's top-left at `left`, `top`.
    pub fn position_for(&self, left: f32, top: f32) -> [f32; 2] {
        let (sx, sy) = self.slack();
        let f = |p: f32, lo: f32, slack: f32| {
            if slack > 0.0 {
                ((p - lo) / slack).clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        [
            f(left, self.area.x + INSET, sx),
            f(top, self.area.y + INSET, sy),
        ]
    }

    /// [`LegendView::max_width`], [`LegendView::max_height`] that put the plate's
    /// bottom-right corner at `right`, `bottom`.
    pub fn limits_for(&self, right: f32, bottom: f32) -> [f32; 2] {
        let (lo, hi) = (*SIZE_LIMITS.start(), *SIZE_LIMITS.end());
        [
            ((right - self.rect.x) / self.area.w.max(1.0)).clamp(lo, hi),
            ((bottom - self.rect.y) / self.area.h.max(1.0)).clamp(lo, hi),
        ]
    }

    /// [`LegendView::first`] after scrolling `rows` down (negative: up).
    pub fn scrolled(&self, rows: i32) -> usize {
        let last = self.total.saturating_sub(self.shown) as i64;
        (self.first as i64 + i64::from(rows)).clamp(0, last) as usize
    }

    /// Whether some rows are scrolled out.
    pub fn scrolls(&self) -> bool {
        self.shown < self.total
    }

    /// Centre y of row `i` when it is shown.
    pub fn row_y(&self, i: usize) -> Option<f32> {
        i.checked_sub(self.first)
            .and_then(|k| self.rows.get(k))
            .map(|r| r.1)
    }

    fn slack(&self) -> (f32, f32) {
        (
            (self.area.w - 2.0 * INSET - self.rect.w).max(0.0),
            (self.area.h - 2.0 * INSET - self.rect.h).max(0.0),
        )
    }
}

/// `3 above · 9 below`, `12 below`, `15 above`.
fn more_text(above: usize, below: usize) -> String {
    match (above, below) {
        (0, b) => format!("{b} below"),
        (a, 0) => format!("{a} above"),
        (a, b) => format!("{a} above · {b} below"),
    }
}

/// Lays the legend of `texts` (one per row) and `notes` out in `area` as `view` asks;
/// `None` when hidden or empty.
pub(crate) fn place(
    view: &LegendView,
    area: Rect,
    texts: &[&str],
    notes: &[String],
    font: f32,
) -> Option<LegendBox> {
    if view.hidden || (texts.is_empty() && notes.is_empty()) {
        return None;
    }
    let (lo, hi) = (*SIZE_LIMITS.start(), *SIZE_LIMITS.end());
    let room_w = (area.w - 2.0 * INSET).max(1.0);
    let room_h = (area.h - 2.0 * INSET).max(1.0);
    let max_w = (view.max_width.clamp(lo, hi) * area.w).min(room_w);
    let max_h = (view.max_height.clamp(lo, hi) * area.h).min(room_h);
    let lines_fit = (((max_h - 2.0 * PAD_Y) / ROW).floor() as usize).max(1);
    let total = texts.len();
    let fixed = notes.len();
    let shown = if total + fixed <= lines_fit {
        total
    } else {
        // One line for the note of what is scrolled out; at least one row stays.
        lines_fit.saturating_sub(fixed + 1).max(1).min(total)
    };
    let first = view.first.min(total - shown);
    let more = (shown < total).then(|| more_text(first, total - first - shown));
    let inner = (max_w - 2.0 * PAD_X).max(1.0);
    let row_room = (inner - SWATCH_W - SWATCH_GAP).max(1.0);
    // Every row sets the width, not only the shown ones: the plate keeps its size as it
    // scrolls.
    let cut: Vec<String> = texts.iter().map(|t| cut_to(t, row_room, font)).collect();
    let notes: Vec<String> = notes.iter().map(|n| cut_to(n, inner, font)).collect();
    let content = cut
        .iter()
        .map(|t| SWATCH_W + SWATCH_GAP + text_width(t, font))
        .chain(notes.iter().chain(&more).map(|t| text_width(t, font)))
        .fold(0.0f32, f32::max);
    let w = (content + 2.0 * PAD_X).min(max_w);
    let lines = shown + fixed + usize::from(more.is_some());
    let h = (lines as f32 * ROW + 2.0 * PAD_Y).min(room_h);
    let x = area.x + INSET + view.x.clamp(0.0, 1.0) * (room_w - w).max(0.0);
    let y = area.y + INSET + view.y.clamp(0.0, 1.0) * (room_h - h).max(0.0);
    let rect = Rect::new(x, y, w, h);
    let row_y = |k: usize| y + PAD_Y + ROW / 2.0 + k as f32 * ROW;
    let rows = cut[first..first + shown]
        .iter()
        .enumerate()
        .map(|(k, t)| (t.clone(), row_y(k)))
        .collect();
    Some(LegendBox {
        area,
        rect,
        grip: Rect::new(rect.right() - GRIP, rect.bottom() - GRIP, GRIP, GRIP),
        first,
        shown,
        total,
        rows,
        more,
        notes,
    })
}

/// A plate: the plot's background, nearly opaque (fully while the pointer is on it, so the
/// rows read at their best when the operator looks at them), with a thin border.
pub(crate) fn plate(layer: &mut Layer, rect: Rect, hovered: bool, clip: Rect, theme: &Theme) {
    let fill = theme
        .plot_background
        .with_alpha(if hovered { 1.0 } else { PLATE_ALPHA });
    let border = if hovered {
        theme.text_dim
    } else {
        theme.grid_major.color
    };
    let clip = Some(clip);
    layer.rects.push(FillRect {
        rect,
        color: fill,
        clip,
    });
    let (x, y, w, h) = (rect.x, rect.y, rect.w, rect.h);
    for r in [
        Rect::new(x, y, w, 1.0),
        Rect::new(x, y + h - 1.0, w, 1.0),
        Rect::new(x, y, 1.0, h),
        Rect::new(x + w - 1.0, y, 1.0, h),
    ] {
        layer.rects.push(FillRect {
            rect: r,
            color: border,
            clip,
        });
    }
}

/// How one row is drawn.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RowStyle {
    pub color: Color,
    pub dim: bool,
    pub selected: bool,
}

/// Draws `b` with `styles` (one per row of the whole legend, not only the shown ones).
pub(crate) fn draw(
    layer: &mut Layer,
    b: &LegendBox,
    styles: &[RowStyle],
    hover: Option<LegendHover>,
    clip: Rect,
    theme: &Theme,
) {
    plate(layer, b.rect, hover.is_some(), clip, theme);
    let font = theme.small_font_size;
    let x0 = b.rect.x + PAD_X;
    let text = |layer: &mut Layer, t: &str, x: f32, y: f32, color: Color| {
        let mut l = label(t, [x, y], anchor(HAlign::Left, VAlign::Center), font, color);
        l.clip = Some(clip);
        layer.labels.push(l);
    };
    for (k, (t, y)) in b.rows.iter().enumerate() {
        let Some(s) = styles.get(b.first + k) else {
            continue;
        };
        // The selected row's bar sits in the padding, before the swatch.
        crate::tf::legend_swatch(
            layer,
            x0 + 1.0,
            *y,
            SWATCH_W,
            s.color,
            s.selected,
            Some(clip),
            theme,
        );
        let color = if s.dim { theme.text_dim } else { theme.text };
        text(layer, t, x0 + SWATCH_W + SWATCH_GAP, *y, color);
    }
    let mut y = b.rect.y + PAD_Y + ROW / 2.0 + b.rows.len() as f32 * ROW;
    for line in b.more.iter().chain(&b.notes) {
        text(layer, line, x0, y, theme.text_dim);
        y += ROW;
    }
    // The grip: three dots along the corner's diagonal, brighter while the pointer is on it.
    let dot = if hover == Some(LegendHover::Grip) {
        theme.text
    } else {
        theme.text_dim
    };
    let (gx, gy) = (b.rect.right() - 3.0, b.rect.bottom() - 3.0);
    for (dx, dy) in [(0.0, 0.0), (-4.0, 0.0), (0.0, -4.0)] {
        layer.rects.push(FillRect {
            rect: Rect::new(gx + dx - 2.0, gy + dy - 2.0, 2.0, 2.0),
            color: dot,
            clip: Some(clip),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(50.0, 30.0, 800.0, 300.0);

    fn texts(n: usize) -> Vec<String> {
        (0..n)
            .map(|i| format!("xc-xone-tf-cap {i} · indep."))
            .collect()
    }

    fn placed(view: &LegendView, n: usize, notes: &[String]) -> LegendBox {
        let t = texts(n);
        let refs: Vec<&str> = t.iter().map(String::as_str).collect();
        place(view, AREA, &refs, notes, 10.5).expect("placed")
    }

    /// A few rows: all shown, top-left, sized to the text.
    #[test]
    fn few_rows_fit_in_the_top_left_corner() {
        let b = placed(&LegendView::default(), 3, &[]);
        assert_eq!((b.first, b.shown, b.total), (0, 3, 3));
        assert_eq!(b.more, None);
        assert_eq!(b.rect.x, AREA.x + INSET);
        assert_eq!(b.rect.y, AREA.y + INSET);
        assert_eq!(b.rect.h, 3.0 * ROW + 2.0 * PAD_Y);
        assert!(b.rect.w < 0.6 * AREA.w);
        assert_eq!(b.rows[0].0, "xc-xone-tf-cap 0 · indep.");
        assert_eq!(b.row_y(1), Some(b.rect.y + PAD_Y + ROW * 1.5));
    }

    /// Eighteen rows in half a 300 px plot: the plate stops at its limit, the rest scroll
    /// and a line says how many are out of view.
    #[test]
    fn many_rows_scroll_inside_the_height_limit() {
        let v = LegendView {
            max_height: 0.5,
            ..LegendView::default()
        };
        let b = placed(&v, 18, &["ref m1 12.34 ms".to_string()]);
        assert!(b.rect.h <= 0.5 * AREA.h);
        // 150 px: 8 lines of 16 px after the padding; the delay note and the more line.
        assert_eq!(b.shown, 6);
        assert_eq!(b.more.as_deref(), Some("12 below"));
        let down = LegendView {
            first: b.scrolled(3),
            ..v
        };
        let b = placed(&down, 18, &["ref m1 12.34 ms".to_string()]);
        assert_eq!(b.first, 3);
        assert_eq!(b.rows[0].0, "xc-xone-tf-cap 3 · indep.");
        assert_eq!(b.more.as_deref(), Some("3 above · 9 below"));
        assert_eq!(b.row_y(2), None);
        // Scrolling never runs past the last rows.
        assert_eq!(b.scrolled(100), 12);
        assert_eq!(b.scrolled(-100), 0);
    }

    /// Corners and a dragged place: fractions of the room beside the plate, inside the plot.
    #[test]
    fn corners_and_dragging() {
        let mut v = LegendView::default();
        v.snap(LegendCorner::BottomRight);
        let b = placed(&v, 3, &[]);
        assert_eq!(b.rect.right(), AREA.right() - INSET);
        assert_eq!(b.rect.bottom(), AREA.bottom() - INSET);
        assert_eq!(v.corner(), Some(LegendCorner::BottomRight));
        // Dragging its top-left to the middle of the room.
        let mid = b.position_for(
            AREA.x + INSET + (AREA.w - 2.0 * INSET - b.rect.w) / 2.0,
            AREA.y + INSET + (AREA.h - 2.0 * INSET - b.rect.h) / 2.0,
        );
        assert_eq!(mid, [0.5, 0.5]);
        v.move_to(mid[0], mid[1]);
        assert_eq!(v.corner(), None);
        // Past the edge stays inside.
        assert_eq!(b.position_for(-1000.0, 1e6), [0.0, 1.0]);
    }

    /// Dragging the grip sets the limits: a narrow plate cuts its rows with `…`.
    #[test]
    fn resizing_cuts_long_rows() {
        let mut v = LegendView::default();
        let b = placed(&v, 3, &[]);
        let [w, h] = b.limits_for(b.rect.x + 120.0, b.rect.y + 60.0);
        assert_eq!(w, 0.15);
        assert_eq!(h, 0.2);
        v.resize(w, h);
        let b = placed(&v, 3, &[]);
        assert!(b.rect.w <= 120.0 + 0.01);
        assert!(b.rows[0].0.ends_with('…'), "{}", b.rows[0].0);
        assert_eq!(b.shown, 3);
        // Limits stay within bounds.
        v.resize(5.0, -1.0);
        assert_eq!((v.max_width, v.max_height), (1.0, 0.15));
    }

    #[test]
    fn hidden_or_empty_draws_nothing() {
        let v = LegendView {
            hidden: true,
            ..LegendView::default()
        };
        assert_eq!(place(&v, AREA, &["a"], &[], 10.5), None);
        assert_eq!(place(&LegendView::default(), AREA, &[], &[], 10.5), None);
    }
}
