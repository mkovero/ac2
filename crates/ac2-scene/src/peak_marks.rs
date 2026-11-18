//! What peak tracking draws the same way on every plot that tracks: the dip's (and the
//! transfer peak's) triangle with its rank, and the list of `max` and `min` rows on the
//! peak list's plate.

use crate::canvas::{Canvas, anchor, label};
use crate::plate::{Metrics, PlateBox, PlateKind, PlatePlace};
use crate::primitives::{Color, HAlign, Polyline, Rect, Stroke, VAlign};
use crate::spectrum::PeakRow;
use crate::spectrum::peaks::Extreme;
use crate::theme::Theme;
use crate::view::ViewState;

/// Half the width of a triangle, logical pixels: as wide as the spectrum's boxes with
/// their stroke.
pub(crate) const TRIANGLE_HALF: f32 = 4.5;

/// The square of half-side `half` around the point (`x`, `y`) of a marked extreme, when
/// the point is on `plot` and `marks` (T) draws peak marks at all.
pub(crate) fn mark_rect(x: f32, y: f32, half: f32, plot: Rect, marks: bool) -> Option<Rect> {
    let inside = marks
        && x.is_finite()
        && y.is_finite()
        && x >= plot.x
        && x <= plot.right()
        && y >= plot.y
        && y <= plot.bottom();
    inside.then(|| Rect::new(x - half, y - half, 2.0 * half, 2.0 * half))
}

/// A triangle in `m` with its rank beyond its base: ▲ on a peak, ▼ on a dip. The point of
/// the triangle says which way the curve turns there, and the rank ties the mark to its
/// row in the list.
pub(crate) fn triangle(
    c: &mut Canvas,
    m: Rect,
    rank: usize,
    dir: Extreme,
    color: Color,
    plot: Rect,
    theme: &Theme,
) {
    let x = m.x + m.w / 2.0;
    let (tip, base) = match dir {
        Extreme::Max => (m.y, m.bottom()),
        Extreme::Min => (m.bottom(), m.y),
    };
    c.overlay.polylines.push(Polyline::clipped(
        vec![[x, tip], [m.right(), base], [m.x, base], [x, tip]],
        Stroke::solid(color, 1.5),
        plot,
    ));
    let (ry, v) = match dir {
        Extreme::Max => (m.y - 2.0, VAlign::Bottom),
        Extreme::Min => (m.bottom() + 2.0, VAlign::Top),
    };
    c.overlay.labels.push(label(
        rank.to_string(),
        [x, ry],
        anchor(HAlign::Center, v),
        theme.small_font_size,
        theme.text,
    ));
}

/// The list as drawn: the curve's `name` when the rows could be read against another
/// curve's, then `max` and `min` with their rows (a group without rows is left out), then
/// `tail` (a transfer's comb).
pub(crate) fn list_text(
    name: Option<&str>,
    max: &[PeakRow],
    min: &[PeakRow],
    tail: Option<&str>,
) -> String {
    let mut lines: Vec<&str> = Vec::new();
    lines.extend(name);
    for (head, rows) in [("max", max), ("min", min)] {
        if !rows.is_empty() {
            lines.push(head);
            lines.extend(rows.iter().map(|r| r.text.as_str()));
        }
    }
    lines.extend(tail);
    lines.join("\n")
}

/// Draws the peak list on its plate ([`PlateKind::Peaks`], the pane's to move and size):
/// where the operator put it, else at `home(width, height, metrics)`.
pub(crate) fn list_plate(
    c: &mut Canvas,
    list: &str,
    plot: Rect,
    align: HAlign,
    home: impl FnOnce(f32, f32, &Metrics) -> Rect,
    view: &ViewState,
    theme: &Theme,
) -> PlateBox {
    let place = &view.plates.peaks;
    let lines: Vec<(String, Color)> = list
        .split('\n')
        .map(|l| (l.to_string(), theme.text))
        .collect();
    let texts: Vec<&str> = lines.iter().map(|l| l.0.as_str()).collect();
    let (w, h) = Metrics::new(theme, place.scale()).size(&texts);
    let s = crate::plate::fitted(place.scale(), plot, w, h);
    let m = Metrics::new(theme, s);
    let (w, h) = m.size(&texts);
    let rect = match place.at {
        Some(at) => PlatePlace::rect_at(at, plot, w, h),
        None => home(w, h, &m),
    };
    let hover = view.plate_hover.filter(|h| h.kind == PlateKind::Peaks);
    crate::plate::draw_lines(
        &mut c.overlay,
        rect,
        &lines,
        align,
        m,
        true,
        hover,
        plot,
        theme,
    );
    PlateBox::new(PlateKind::Peaks, plot, rect, s)
}
