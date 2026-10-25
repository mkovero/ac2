//! A plot's legend box: its rows on a plate in the plot's own colour, so they read over any
//! number of curves running behind them; placed where the operator dragged it (or snapped to
//! a corner) at the text size they chose ([`crate::plate`]), and scrolled when the rows do
//! not all fit. The cursor readout beside it sits on the same kind of plate.

use crate::canvas::{anchor, cut_to, label, text_width};
use crate::plate::{Metrics, PlateBox, PlateHover, PlateKind, PlatePlace};
use crate::primitives::{Color, FillRect, HAlign, Layer, Rect, VAlign};
use crate::theme::Theme;
use crate::trace::TraceKey;

/// Row pitch.
pub const ROW: f32 = 16.0;
/// Padding inside a plate.
pub const PAD_X: f32 = 6.0;
pub const PAD_Y: f32 = 4.0;
/// Distance a plate keeps from the plot's edges, so its border never merges with the frame.
pub const INSET: f32 = 4.0;
/// Width of the scale grip's square at a plate's bottom-right corner.
pub const GRIP: f32 = 12.0;
/// Widest the legend may be, a fraction of the plot's width: longer rows are cut with `…`
/// so the curves keep most of the plot.
pub const MAX_WIDTH: f32 = 0.6;
/// Tallest the legend may be, a fraction of the plot's height; rows beyond it scroll.
pub const MAX_HEIGHT: f32 = 0.7;
/// Colour swatch of a row and the gap after it.
const SWATCH_W: f32 = 12.0;
const SWATCH_GAP: f32 = 6.0;

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

    /// Its place as [`PlatePlace::at`].
    pub fn position(self) -> [f32; 2] {
        match self {
            Self::TopLeft => [0.0, 0.0],
            Self::TopRight => [1.0, 0.0],
            Self::BottomLeft => [0.0, 1.0],
            Self::BottomRight => [1.0, 1.0],
        }
    }
}

/// The operator's legend choices besides its place and size ([`crate::plate::Plates`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LegendView {
    pub hidden: bool,
    /// First row shown when the rows scroll.
    pub first: usize,
}

/// Where a legend row is drawn and whose curve it names: a click there selects that curve,
/// as a click on its row in the measurement list does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LegendHit {
    pub key: TraceKey,
    pub rect: Rect,
}

/// The curve whose legend row is at `x`, `y`, if any.
pub fn hit_at(hits: &[LegendHit], x: f32, y: f32) -> Option<TraceKey> {
    hits.iter()
        .find(|h| {
            (h.rect.x..=h.rect.right()).contains(&x) && (h.rect.y..=h.rect.bottom()).contains(&y)
        })
        .map(|h| h.key)
}

/// Where the legend went and what it shows.
#[derive(Clone, Debug, PartialEq)]
pub struct LegendBox {
    /// The plate, what it moves within (the plot below its title) and its grip.
    pub plate: PlateBox,
    /// The plate (as `plate.rect`).
    pub rect: Rect,
    /// Row pitch at its text scale.
    pub row: f32,
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

    /// The shown rows across the plate, `keys` naming every row of the whole legend (not
    /// only the shown ones).
    pub fn hits(&self, keys: &[TraceKey]) -> Vec<LegendHit> {
        self.rows
            .iter()
            .enumerate()
            .filter_map(|(k, &(_, y))| {
                let key = *keys.get(self.first + k)?;
                let rect = Rect::new(self.rect.x, y - self.row / 2.0, self.rect.w, self.row);
                Some(LegendHit { key, rect })
            })
            .collect()
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

/// Lays the legend of `texts` (one per row) and `notes` out in `area` as `view` asks, at
/// `place` (the top-left corner while the scene places it); `None` when hidden or empty.
pub(crate) fn place(
    view: &LegendView,
    place: &PlatePlace,
    area: Rect,
    texts: &[&str],
    notes: &[String],
    m: Metrics,
) -> Option<LegendBox> {
    if view.hidden || (texts.is_empty() && notes.is_empty()) {
        return None;
    }
    let room_w = (area.w - 2.0 * INSET).max(1.0);
    let room_h = (area.h - 2.0 * INSET).max(1.0);
    let max_w = (MAX_WIDTH * area.w).min(room_w);
    let max_h = (MAX_HEIGHT * area.h).min(room_h);
    // Rows are cut to the width and scrolled within the height, but the notes, one row and
    // the line counting the rest must fit: a text too large for that shrinks to fit.
    let least = notes.len() + texts.len().min(1) + usize::from(texts.len() > 1);
    let s = {
        let set = place.scale();
        let need = least as f32 * m.row + 2.0 * m.pad_y;
        let room = Rect::new(area.x, area.y, area.w, max_h + 2.0 * INSET);
        crate::plate::fitted(set, room, 1.0, need)
    };
    let m = m.scaled(s / place.scale());
    let (font, row, pad_x, pad_y) = (m.font, m.row, m.pad_x, m.pad_y);
    let lines_fit = (((max_h - 2.0 * pad_y) / row).floor() as usize).max(1);
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
    let inner = (max_w - 2.0 * pad_x).max(1.0);
    let row_room = (inner - (SWATCH_W + SWATCH_GAP) * s).max(1.0);
    // Every row sets the width, not only the shown ones: the plate keeps its size as it
    // scrolls.
    let cut: Vec<String> = texts.iter().map(|t| cut_to(t, row_room, font)).collect();
    let notes: Vec<String> = notes.iter().map(|n| cut_to(n, inner, font)).collect();
    let content = cut
        .iter()
        .map(|t| (SWATCH_W + SWATCH_GAP) * s + text_width(t, font))
        .chain(notes.iter().chain(&more).map(|t| text_width(t, font)))
        .fold(0.0f32, f32::max);
    let w = (content + 2.0 * pad_x).min(max_w);
    let lines = shown + fixed + usize::from(more.is_some());
    let h = (lines as f32 * row + 2.0 * pad_y).min(room_h);
    let rect = PlatePlace::rect_at(place.at.unwrap_or([0.0, 0.0]), area, w, h);
    let y = rect.y;
    let row_y = |k: usize| y + pad_y + row / 2.0 + k as f32 * row;
    let rows = cut[first..first + shown]
        .iter()
        .enumerate()
        .map(|(k, t)| (t.clone(), row_y(k)))
        .collect();
    Some(LegendBox {
        plate: PlateBox::new(PlateKind::Legend, area, rect, s),
        rect,
        row,
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
        .with_alpha(if hovered { 1.0 } else { theme.plate_alpha });
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

/// Draws `b` with `styles` (one per row of the whole legend, not only the shown ones);
/// `hover`: the pointer is on it (or its grip).
pub(crate) fn draw(
    layer: &mut Layer,
    b: &LegendBox,
    styles: &[RowStyle],
    hover: Option<PlateHover>,
    clip: Rect,
    theme: &Theme,
) {
    plate(layer, b.rect, hover.is_some(), clip, theme);
    let s = b.plate.scale;
    let m = Metrics::new(theme, s);
    let font = m.font;
    let x0 = b.rect.x + m.pad_x;
    let text = |layer: &mut Layer, t: &str, x: f32, y: f32, color: Color| {
        let mut l = label(t, [x, y], anchor(HAlign::Left, VAlign::Center), font, color);
        l.clip = Some(clip);
        layer.labels.push(l);
    };
    for (k, (t, y)) in b.rows.iter().enumerate() {
        let Some(st) = styles.get(b.first + k) else {
            continue;
        };
        // The selected row's bar sits in the padding, before the swatch.
        crate::tf::legend_swatch(
            layer,
            x0 + 1.0,
            *y,
            SWATCH_W * s,
            st.color,
            st.selected,
            Some(clip),
            theme,
        );
        let color = if st.dim { theme.text_dim } else { theme.text };
        text(layer, t, x0 + (SWATCH_W + SWATCH_GAP) * s, *y, color);
    }
    let mut y = b.rect.y + m.pad_y + m.row / 2.0 + b.rows.len() as f32 * m.row;
    for line in b.more.iter().chain(&b.notes) {
        text(layer, line, x0, y, theme.text_dim);
        y += m.row;
    }
    crate::plate::grip(layer, b.rect, hover.is_some_and(|h| h.grip), clip, theme);
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

    fn placed_at(
        view: &LegendView,
        at: &PlatePlace,
        area: Rect,
        n: usize,
        notes: &[String],
    ) -> LegendBox {
        let t = texts(n);
        let refs: Vec<&str> = t.iter().map(String::as_str).collect();
        place(view, at, area, &refs, notes, Metrics::of(10.5, at.scale())).expect("placed")
    }

    fn placed(view: &LegendView, n: usize, notes: &[String]) -> LegendBox {
        placed_at(view, &PlatePlace::default(), AREA, n, notes)
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
        assert!(b.rect.w < MAX_WIDTH * AREA.w);
        assert_eq!(b.rows[0].0, "xc-xone-tf-cap 0 · indep.");
        assert_eq!(b.row_y(1), Some(b.rect.y + PAD_Y + ROW * 1.5));
    }

    /// Eighteen rows in a 214 px plot: the plate stops at its limit (150 px), the rest
    /// scroll and a line says how many are out of view.
    #[test]
    fn many_rows_scroll_inside_the_height_limit() {
        let area = Rect::new(50.0, 30.0, 800.0, 150.0 / MAX_HEIGHT);
        let v = LegendView::default();
        let at = PlatePlace::default();
        let notes = ["ref m1 12.34 ms".to_string()];
        let b = placed_at(&v, &at, area, 18, &notes);
        assert!(b.rect.h <= 150.0 + 0.01);
        // 150 px: 8 lines of 16 px after the padding; the delay note and the more line.
        assert_eq!(b.shown, 6);
        assert_eq!(b.more.as_deref(), Some("12 below"));
        let down = LegendView {
            first: b.scrolled(3),
            ..v
        };
        let b = placed_at(&down, &at, area, 18, &notes);
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
        let v = LegendView::default();
        let mut at = PlatePlace {
            at: Some(LegendCorner::BottomRight.position()),
            ..PlatePlace::default()
        };
        let b = placed_at(&v, &at, AREA, 3, &[]);
        assert_eq!(b.rect.right(), AREA.right() - INSET);
        assert_eq!(b.rect.bottom(), AREA.bottom() - INSET);
        // Dragging its top-left to the middle of the room.
        let mid = b.plate.position_for(
            AREA.x + INSET + (AREA.w - 2.0 * INSET - b.rect.w) / 2.0,
            AREA.y + INSET + (AREA.h - 2.0 * INSET - b.rect.h) / 2.0,
        );
        assert_eq!(mid, [0.5, 0.5]);
        at.move_to(mid[0], mid[1]);
        let b = placed_at(&v, &at, AREA, 3, &[]);
        assert_eq!(b.plate.position_for(b.rect.x, b.rect.y), [0.5, 0.5]);
        // Past the edge stays inside.
        assert_eq!(b.plate.position_for(-1000.0, 1e6), [0.0, 1.0]);
    }

    /// A larger text scale: wider, taller rows, the hit rows following them.
    #[test]
    fn scaled_text_grows_the_plate() {
        let v = LegendView::default();
        let one = placed(&v, 3, &[]);
        let two = placed_at(
            &v,
            &PlatePlace {
                at: None,
                scale: 2.0,
            },
            AREA,
            3,
            &[],
        );
        assert_eq!(two.rect.h, 2.0 * one.rect.h);
        assert!(two.rect.w > 1.9 * one.rect.w);
        assert_eq!(two.row, 2.0 * ROW);
        assert_eq!(two.plate.scale, 2.0);
        let keys: Vec<TraceKey> = (0..3)
            .map(|i| TraceKey::Live(ac2_proto::units::MeasId(i)))
            .collect();
        assert_eq!(two.hits(&keys)[1].rect.h, 2.0 * ROW);
    }

    #[test]
    fn hidden_or_empty_draws_nothing() {
        let v = LegendView {
            hidden: true,
            ..LegendView::default()
        };
        let at = PlatePlace::default();
        let m = Metrics::of(10.5, 1.0);
        assert_eq!(place(&v, &at, AREA, &["a"], &[], m), None);
        assert_eq!(place(&LegendView::default(), &at, AREA, &[], &[], m), None);
    }
}
