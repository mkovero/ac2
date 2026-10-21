//! The alignment view of a transfer pane: the phase difference of the two systems as
//! measured and as aligned (top), how fully they add as measured and as aligned (bottom),
//! the band they were aligned in as a gentle hint in both, and the readout above.

use ac2_proto::model::{AlignBand, AlignCurves, AlignState};

use super::{Readout, readout};
use crate::axis::{self, Axis, Range};
use crate::banner::{BannerRow, Status};
use crate::canvas::{self, Canvas, MARGINS, PANE_GAP, anchor, cut_to, gapped, label};
use crate::primitives::{
    Anchor, Color, FillRect, HAlign, Polyline, Rect, Scene, Stroke, VAlign, Viewport,
};
use crate::theme::Theme;
use crate::view::{Style, ViewState};

/// The summation axis, dB: 0 is full addition, a −30 dB dip is as deep as a cancellation
/// needs to read as one.
const SUM_RANGE: Range = Range::new(-30.0, 3.0);

/// One alignment channel's newest frame, its systems named.
#[derive(Clone, Copy, Debug)]
pub struct Shown<'a> {
    /// The systems' names, A and B.
    pub a: &'a str,
    pub b: &'a str,
    pub band: AlignBand,
    pub state: &'a AlignState,
    pub curves: &'a AlignCurves,
    pub freqs: &'a [f64],
    /// The channel's colour (the aligned curves); the as-measured ones are dim.
    pub color: Color,
}

/// What the view draws.
#[derive(Clone, Copy, Debug)]
pub enum Content<'a> {
    Shown(Shown<'a>),
    /// Nothing to draw: why (`No alignment in Sub's group — Alt+M aligns two transfer
    /// curves`).
    Empty(&'a str),
}

/// The two plots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignPlot {
    Phase,
    Summation,
}

/// The view, as data plus the scene.
#[derive(Clone, Debug, PartialEq)]
pub struct AlignScene {
    pub scene: Scene,
    pub x_axis: Axis,
    pub plots: Vec<(AlignPlot, Rect)>,
    /// The readout as drawn; `None` while empty.
    pub readout: Option<Readout>,
    /// Plot titles as drawn.
    pub titles: Vec<String>,
    /// The empty view's text.
    pub empty: Option<String>,
    pub banners: Vec<BannerRow>,
}

fn band_hint(c: &mut Canvas, plot: Rect, x: &Axis, band: AlignBand, theme: &Theme) {
    let (x0, x1) = (x.mapping.to_px(band.lo.0), x.mapping.to_px(band.hi.0));
    let (l, r) = (x0.max(plot.x), x1.min(plot.right()));
    if r > l {
        c.base.rects.push(FillRect {
            rect: Rect::new(l, plot.y, r - l, plot.h),
            color: theme.unresolved_shade,
            clip: Some(plot),
        });
    }
    for e in [x0, x1] {
        if e > plot.x + 0.5 && e < plot.right() - 0.5 {
            c.base.polylines.push(Polyline {
                points: vec![[e, plot.y], [e, plot.bottom()]],
                alpha: vec![],
                stroke: theme.unresolved_edge,
                clip: Some(plot),
            });
        }
    }
}

fn curve(c: &mut Canvas, plot: Rect, xs: &[f32], y: &Axis, v: &[f32], stroke: Stroke, wraps: bool) {
    let ys: Vec<f32> = v
        .iter()
        .map(|&d| {
            if d.is_finite() {
                y.mapping.to_px(f64::from(d))
            } else {
                f32::NAN
            }
        })
        .collect();
    // A wrapped phase jumping by more than half a turn is the wrap, not a slope.
    let (points, _) = gapped(xs, &ys, None, |p, i| wraps && (v[i] - v[p]).abs() > 180.0);
    if points.len() > 1 {
        c.data.polylines.push(Polyline {
            points,
            alpha: vec![],
            stroke,
            clip: Some(plot),
        });
    }
}

/// The two swatches at a plot's top right: dim `as measured`, coloured `aligned`.
fn key(c: &mut Canvas, plot: Rect, color: Color, theme: &Theme) {
    let font = theme.small_font_size;
    let mut x = plot.right() - 8.0;
    let y = plot.y + 4.0 + font / 2.0;
    for (text, col) in [("aligned", color), ("as measured", theme.text_dim)] {
        let w = canvas::text_width(text, font);
        x -= w;
        c.overlay.labels.push(label(
            text,
            [x, y],
            anchor(HAlign::Left, VAlign::Center),
            font,
            theme.text,
        ));
        x -= 22.0;
        crate::tf::legend_swatch(&mut c.overlay, x, y, 16.0, col, false, Some(plot), theme);
        x -= 10.0;
    }
}

/// The alignment view of `content` at `size`.
pub fn alignment_scene(
    content: &Content<'_>,
    status: &Status,
    view: &ViewState,
    theme: &Theme,
    size: Viewport,
) -> AlignScene {
    let style = Style {
        chrome: view.chrome,
        theme,
    };
    let mut c = Canvas::new(size, theme);
    let plot_x = MARGINS.left;
    let plot_w = (size.width - MARGINS.left - MARGINS.right).max(1.0);
    let strip = canvas::banner_strip(&mut c, status, plot_x, plot_w, size, theme);
    let x_axis = axis::freq_axis(view.freq.range(), plot_x, plot_x + plot_w);
    let mut top = strip.rect.bottom() + MARGINS.top;
    let (shown, empty) = match content {
        Content::Shown(s) => (Some(s), None),
        Content::Empty(why) => (None, Some((*why).to_string())),
    };
    let ro = shown.map(|s| readout(s.state, s.a, s.b, s.band, view.temperature_c));
    if let Some(r) = &ro {
        let big = theme.font_size;
        let small = theme.small_font_size;
        c.overlay.labels.push(label(
            cut_to(&r.headline, plot_w, big),
            [plot_x, top],
            Anchor::TOP_LEFT,
            big,
            if r.fit { theme.text } else { theme.text_dim },
        ));
        top += big * 1.45;
        for line in &r.lines {
            c.overlay.labels.push(label(
                cut_to(line, plot_w, small),
                [plot_x, top],
                Anchor::TOP_LEFT,
                small,
                theme.text_dim,
            ));
            top += small * 1.4;
        }
        top += 4.0;
    }
    let bottom = size.height - MARGINS.bottom;
    let h = ((bottom - top - PANE_GAP) / 2.0).max(1.0);
    let phase_plot = Rect::new(plot_x, top, plot_w, h);
    let sum_plot = Rect::new(plot_x, top + h + PANE_GAP, plot_w, h);
    let names = shown.map_or(("A".to_string(), "B".to_string()), |s| {
        (s.a.to_string(), s.b.to_string())
    });
    let titles = vec![
        format!("phase difference {} − {} (°)", names.0, names.1),
        "summation (dB, 0 = they add fully)".to_string(),
    ];
    let y_phase = axis::phase_axis(Range::new(-180.0, 180.0), phase_plot.bottom(), phase_plot.y);
    let y_sum = axis::linear_axis(SUM_RANGE, sum_plot.bottom(), sum_plot.y, "dB");
    canvas::pane_frame(
        &mut c, phase_plot, &x_axis, &y_phase, false, &titles[0], style,
    );
    canvas::pane_frame(&mut c, sum_plot, &x_axis, &y_sum, true, &titles[1], style);
    let zero = Stroke::solid(theme.text_dim, 1.0);
    canvas::hline(&mut c, phase_plot, y_phase.mapping.to_px(0.0), zero);
    canvas::hline(&mut c, sum_plot, y_sum.mapping.to_px(0.0), zero);
    if let Some(s) = shown {
        band_hint(&mut c, phase_plot, &x_axis, s.band, theme);
        band_hint(&mut c, sum_plot, &x_axis, s.band, theme);
        let xs: Vec<f32> = s.freqs.iter().map(|f| x_axis.mapping.to_px(*f)).collect();
        let dim = Stroke::solid(theme.text_dim, theme.trace_width);
        let bright = Stroke::solid(s.color, theme.trace_width * 1.5);
        let cv = s.curves;
        curve(
            &mut c,
            phase_plot,
            &xs,
            &y_phase,
            &cv.phase_before,
            dim,
            true,
        );
        curve(
            &mut c,
            phase_plot,
            &xs,
            &y_phase,
            &cv.phase_after,
            bright,
            true,
        );
        curve(&mut c, sum_plot, &xs, &y_sum, &cv.sum_before, dim, false);
        curve(&mut c, sum_plot, &xs, &y_sum, &cv.sum_after, bright, false);
        if view.chrome.labels() {
            key(&mut c, phase_plot, s.color, theme);
        }
    }
    if let Some(why) = &empty {
        let font = theme.font_size;
        c.overlay.labels.push(label(
            cut_to(why, plot_w - 12.0, font),
            [plot_x + plot_w / 2.0, phase_plot.bottom() + PANE_GAP / 2.0],
            anchor(HAlign::Center, VAlign::Center),
            font,
            theme.text_dim,
        ));
    }
    AlignScene {
        scene: c.into_scene(size),
        x_axis,
        plots: vec![
            (AlignPlot::Phase, phase_plot),
            (AlignPlot::Summation, sum_plot),
        ],
        readout: ro,
        titles,
        empty,
        banners: strip.rows,
    }
}
