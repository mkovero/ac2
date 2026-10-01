//! Shared drawing helpers for the builders: layer stack, pane frames, gap-aware polylines.

use crate::axis::{Axis, TickKind};
use crate::primitives::{
    Anchor, Color, FillRect, Grid, GridAxis, GridKind, GridLine, HAlign, Label, Layer, Polyline,
    Rect, Scene, Stroke, VAlign, Viewport,
};
use crate::theme::Theme;

/// Layer order: base (backgrounds, grids, axis labels) → data (traces) → overlay (cursor,
/// legend, readouts) → banners (cover everything).
#[derive(Debug, Default)]
pub(crate) struct Canvas {
    pub base: Layer,
    pub data: Layer,
    pub overlay: Layer,
    pub banners: Layer,
}

impl Canvas {
    pub fn new(size: Viewport, theme: &Theme) -> Self {
        let mut c = Self::default();
        c.base.rects.push(FillRect {
            rect: Rect::new(0.0, 0.0, size.width, size.height),
            color: theme.background,
            clip: None,
        });
        c
    }

    pub fn into_scene(self, size: Viewport) -> Scene {
        Scene {
            viewport: size,
            layers: vec![self.base, self.data, self.overlay, self.banners],
        }
    }
}

/// Margins around a plot rectangle: room for y labels on the left and x labels below.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Margins {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

pub(crate) const MARGINS: Margins = Margins {
    left: 48.0,
    right: 12.0,
    top: 8.0,
    bottom: 22.0,
};

/// Gap between stacked panes.
pub(crate) const PANE_GAP: f32 = 10.0;

pub(crate) fn label(
    text: impl Into<String>,
    pos: [f32; 2],
    anchor: Anchor,
    size: f32,
    color: Color,
) -> Label {
    Label {
        text: text.into(),
        pos,
        anchor,
        size,
        color,
        clip: None,
    }
}

pub(crate) const fn anchor(h: HAlign, v: VAlign) -> Anchor {
    Anchor { h, v }
}

/// Plot background, grid from both axes' ticks, y labels left of the plot, x labels below
/// it when `x_labels`, and the y-axis title inside the top-left corner.
pub(crate) fn pane_frame(
    c: &mut Canvas,
    plot: Rect,
    x: &Axis,
    y: &Axis,
    x_labels: bool,
    title: &str,
    theme: &Theme,
) {
    c.base.rects.push(FillRect {
        rect: plot,
        color: theme.plot_background,
        clip: None,
    });
    let mut lines = Vec::new();
    for (axis, ga) in [(x, GridAxis::X), (y, GridAxis::Y)] {
        for t in &axis.ticks {
            if !t.pos.is_finite() {
                continue;
            }
            lines.push(GridLine {
                axis: ga,
                pos: t.pos,
                kind: match t.kind {
                    TickKind::Major => GridKind::Major,
                    TickKind::Minor => GridKind::Minor,
                },
            });
        }
    }
    c.base.grids.push(Grid {
        rect: plot,
        lines,
        major: theme.grid_major,
        minor: theme.grid_minor,
    });
    for t in &y.ticks {
        if let Some(text) = &t.label {
            c.base.labels.push(label(
                text.clone(),
                [plot.x - 5.0, t.pos],
                anchor(HAlign::Right, VAlign::Center),
                theme.small_font_size,
                theme.axis_text,
            ));
        }
    }
    if x_labels {
        for t in &x.ticks {
            if let Some(text) = &t.label {
                c.base.labels.push(label(
                    text.clone(),
                    [t.pos, plot.bottom() + 4.0],
                    anchor(HAlign::Center, VAlign::Top),
                    theme.small_font_size,
                    theme.axis_text,
                ));
            }
        }
    }
    c.base.labels.push(label(
        title,
        [plot.x + 6.0, plot.y + 4.0],
        Anchor::TOP_LEFT,
        theme.small_font_size,
        theme.text_dim,
    ));
}

/// A horizontal reference line (0 dB, 0°) when `y` is inside the plot.
pub(crate) fn hline(c: &mut Canvas, plot: Rect, y: f32, stroke: Stroke) {
    if y.is_finite() && y >= plot.y && y <= plot.bottom() {
        c.base.polylines.push(Polyline {
            points: vec![[plot.x, y], [plot.right(), y]],
            alpha: vec![],
            stroke,
            clip: Some(plot),
        });
    }
}

/// A vertical line across `plot` at `x` when inside it.
pub(crate) fn vline(layer: &mut Layer, plot: Rect, x: f32, stroke: Stroke) {
    if x.is_finite() && x >= plot.x && x <= plot.right() {
        layer.polylines.push(Polyline {
            points: vec![[x, plot.y], [x, plot.bottom()]],
            alpha: vec![],
            stroke,
            clip: Some(plot),
        });
    }
}

/// Points with gaps: a non-finite coordinate becomes one NaN break point (runs collapse);
/// `split(prev, cur)` can request an extra break between two finite neighbours (wrapped
/// phase jumps). Leading and trailing breaks are dropped. `alpha` (if given) follows the
/// points one-to-one.
pub(crate) fn gapped(
    xs: &[f32],
    ys: &[f32],
    alpha: Option<&[f32]>,
    split: impl Fn(usize, usize) -> bool,
) -> (Vec<[f32; 2]>, Vec<f32>) {
    let mut pts: Vec<[f32; 2]> = Vec::with_capacity(xs.len());
    let mut al: Vec<f32> = Vec::new();
    let mut last: Option<usize> = None;
    let push_break = |pts: &mut Vec<[f32; 2]>, al: &mut Vec<f32>| {
        if pts.last().is_some_and(|p| p[0].is_finite()) {
            pts.push([f32::NAN, f32::NAN]);
            if alpha.is_some() {
                al.push(0.0);
            }
        }
    };
    for i in 0..xs.len().min(ys.len()) {
        let ok = xs[i].is_finite() && ys[i].is_finite();
        if !ok {
            push_break(&mut pts, &mut al);
            last = None;
            continue;
        }
        if let Some(p) = last
            && split(p, i)
        {
            push_break(&mut pts, &mut al);
        }
        pts.push([xs[i], ys[i]]);
        if let Some(a) = alpha {
            al.push(a.get(i).copied().unwrap_or(1.0));
        }
        last = Some(i);
    }
    if pts.last().is_some_and(|p| !p[0].is_finite()) {
        pts.pop();
        if alpha.is_some() {
            al.pop();
        }
    }
    (pts, al)
}

/// Index range of columns worth drawing for an x range: those inside plus one neighbour on
/// each side so lines run to the plot edge.
pub(crate) fn visible_columns(freqs: &[f64], lo: f64, hi: f64) -> std::ops::Range<usize> {
    let first = freqs.iter().position(|f| *f >= lo).unwrap_or(freqs.len());
    let last = freqs.iter().rposition(|f| *f <= hi).map_or(0, |i| i + 1);
    let a = first.saturating_sub(1);
    let b = (last + 1).min(freqs.len());
    if a < b { a..b } else { 0..0 }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Runs of finite points in a gapped polyline.
    pub fn segments(points: &[[f32; 2]]) -> Vec<Vec<[f32; 2]>> {
        let mut out = vec![];
        let mut cur = vec![];
        for p in points {
            if p[0].is_finite() && p[1].is_finite() {
                cur.push(*p);
            } else if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    }

    #[test]
    fn nan_runs_collapse_to_one_break() {
        let xs = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let ys = [f32::NAN, 1.0, f32::NAN, f32::NAN, 4.0, 5.0, f32::NAN];
        let al = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7];
        let (p, a) = gapped(&xs, &ys, Some(&al), |_, _| false);
        assert_eq!(p.len(), 4);
        assert_eq!(a.len(), 4);
        assert!(p[1][0].is_nan());
        assert_eq!(
            segments(&p),
            vec![vec![[1.0, 1.0]], vec![[4.0, 4.0], [5.0, 5.0]]]
        );
        assert_eq!(a, vec![0.2, 0.0, 0.5, 0.6]);
    }

    #[test]
    fn split_inserts_break() {
        let xs = [0.0, 1.0, 2.0];
        let ys = [0.0, 1.0, 2.0];
        let (p, a) = gapped(&xs, &ys, None, |a, _| a == 1);
        assert_eq!(segments(&p).len(), 2);
        assert!(a.is_empty());
    }

    #[test]
    fn visible_range() {
        let f = [10.0, 20.0, 40.0, 80.0, 160.0];
        assert_eq!(visible_columns(&f, 25.0, 100.0), 1..5);
        assert_eq!(visible_columns(&f, 20.0, 40.0), 0..4);
        // Sparse columns either side of the range still give the crossing segment.
        assert_eq!(visible_columns(&[10.0, 10_000.0], 100.0, 200.0), 0..2);
        assert_eq!(visible_columns(&[], 100.0, 200.0), 0..0);
    }
}
