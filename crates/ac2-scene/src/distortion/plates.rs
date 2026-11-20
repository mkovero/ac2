//! The sweep view's legend and cursor values on the plates every frequency view uses
//! ([`crate::legend`], [`crate::plate`]): dragged, scaled, scrolled and hidden alike.

use super::*;
use crate::legend::{LegendBox, LegendHit, RowStyle};
use crate::plate::{Metrics, PlateBox, PlateKind, PlatePlace};
use crate::primitives::{FillRect, Layer};

/// Above the legend's plate placed by the scene: the axis title's line at the plot's top.
pub(super) const LEGEND_TOP: f32 = 18.0;

/// The sweep view's legend as drawn ([`legend`]).
pub(crate) struct SweepLegend {
    /// The shown rows' texts as drawn (cut to the plate's width).
    pub shown: Vec<String>,
    /// The plate; `None` when hidden or empty.
    pub b: Option<LegendBox>,
    /// Where each shown row is, for a click on it.
    pub hits: Vec<LegendHit>,
}

/// The legend plate of `entries` ([`crate::legend::legend_plate`]) in `area`, clipped to
/// `clip`. A row that names a line style rather than a curve (the
/// floor's dashes, the reference's dots, the noise's shade) has a sample of that style
/// where the other rows have their swatch.
pub(crate) fn legend(
    layer: &mut Layer,
    entries: &[LegendEntry],
    view: &ViewState,
    area: Rect,
    clip: Rect,
    theme: &Theme,
) -> SweepLegend {
    let rows: Vec<crate::tf::LegendEntry> = entries
        .iter()
        .map(|e| crate::tf::LegendEntry {
            key: e.key,
            name: e.name.clone(),
            tags: Vec::new(),
            text: e.name.clone(),
            stale: false,
            selected: e.selected,
        })
        .collect();
    let styles: Vec<RowStyle> = entries
        .iter()
        .map(|e| {
            let line = e.mark == LegendMark::Line;
            RowStyle {
                // A sample takes the swatch's place.
                color: e.color.with_alpha(if line { 1.0 } else { 0.0 }),
                dim: !line,
                selected: e.selected,
            }
        })
        .collect();
    let (b, hits) =
        crate::legend::legend_plate(layer, &rows, &styles, &[], view, area, clip, theme);
    if let Some(b) = &b {
        for k in 0..b.rows.len() {
            let (Some(e), Some(r)) = (entries.get(b.first + k), b.swatch(k)) else {
                continue;
            };
            let mid = r.y + r.h / 2.0;
            let line = |stroke| Polyline::clipped(vec![[r.x, mid], [r.right(), mid]], stroke, clip);
            match e.mark {
                LegendMark::Line => {}
                LegendMark::Dashed => layer.polylines.push(line(floor_stroke(e.color, theme))),
                LegendMark::Dotted => layer.polylines.push(line(reference_stroke(e.color, theme))),
                LegendMark::Shade => {
                    layer
                        .rects
                        .push(FillRect::clipped(r, noise_color(theme), clip));
                }
            }
        }
    }
    SweepLegend {
        shown: b
            .as_ref()
            .map_or_else(Vec::new, |b| b.rows.iter().map(|r| r.0.clone()).collect()),
        b,
        hits,
    }
}

/// Draws the cursor values `lines` (text, colour) on their plate. Placed by the scene, it
/// hangs from the plot's right edge with its text's top at `top`; dragged, it sits where it
/// was dropped. A plate too large for the plot shrinks its text to stay whole inside it.
pub(crate) fn readout(
    layer: &mut Layer,
    lines: Vec<(String, Color)>,
    plot: Rect,
    top: f32,
    view: &ViewState,
    theme: &Theme,
) -> PlateBox {
    let place = &view.plates.readout;
    let texts: Vec<&str> = lines.iter().map(|l| l.0.as_str()).collect();
    let set = place.scale();
    let (w, h) = Metrics::new(theme, set).size(&texts);
    let s = crate::plate::fitted(set, plot, w, h);
    let m = Metrics::new(theme, s);
    let (w, h) = m.size(&texts);
    let rect = match place.at {
        Some(at) => PlatePlace::rect_at(at, plot, w, h),
        None => Rect::new(plot.right() - crate::legend::INSET - w, top - m.pad_y, w, h),
    };
    let hover = view.plate_hover.filter(|h| h.kind == PlateKind::Readout);
    crate::plate::draw_lines(
        layer,
        rect,
        &lines,
        HAlign::Right,
        m,
        true,
        hover,
        plot,
        theme,
    );
    PlateBox::new(PlateKind::Readout, plot, rect, s)
}
