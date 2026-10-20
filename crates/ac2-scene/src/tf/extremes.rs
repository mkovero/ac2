//! Min / max tracking on the transfer magnitude: the highest peaks and deepest dips of one
//! curve, marked ▲ / ▼ where they are and listed on a plate in a corner of the magnitude
//! view. Dips explain a comb (a delayed copy summed in) at a glance; when four or more
//! are evenly spaced the list names the spacing and the delay it implies.

use super::*;
use crate::spectrum::PeakRow;
use crate::spectrum::peaks::{self, Extreme, Peak, find_extremes};

/// The tracked curve's peaks and dips as listed and marked.
#[derive(Clone, Debug, PartialEq)]
pub struct TfExtremes {
    pub key: TraceKey,
    pub name: String,
    /// Highest first.
    pub max: Vec<PeakRow>,
    /// Deepest first.
    pub min: Vec<PeakRow>,
    pub comb: Option<Comb>,
}

/// Evenly spaced dips ([`peaks::comb_spacing`]): their spacing and the delay between the
/// two arrivals that makes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Comb {
    pub spacing_hz: f64,
    pub delay_s: f64,
    /// `comb Δf 1.00 kHz · 1.00 ms · 0.34 m @ 20 °C` ([`crate::readout::comb_text`]).
    pub text: String,
}

/// Size of a mark's triangle, logical pixels: as wide as the spectrum's boxes.
const MARK: f32 = 4.5;

/// The curve min / max tracking follows: the selected one when shown, else the first of
/// the pane's own (not compared), else the first. One curve, so the marks and the list stay
/// one statement.
pub(super) fn tracked(traces: &[TfTrace<'_>]) -> Option<usize> {
    traces
        .iter()
        .position(|t| t.selected)
        .or_else(|| traces.iter().position(|t| !t.compared))
        .or_else(|| (!traces.is_empty()).then_some(0))
}

/// Finds, tracks and marks the extremes of the tracked curve's displayed magnitude (smoothed
/// as served, offset, coherence-blanked columns left out) in the magnitude view `plot`.
#[allow(clippy::too_many_arguments)]
pub(super) fn extremes(
    c: &mut Canvas,
    traces: &[TfTrace<'_>],
    shown: &[DisplayTrace],
    cache: &DisplayCache,
    plot: Rect,
    xm: &crate::axis::Mapping,
    ym: &crate::axis::Mapping,
    ranks: bool,
    temp_c: f64,
    theme: &Theme,
) -> Option<TfExtremes> {
    let d = shown.get(tracked(traces)?)?;
    // The displayed magnitude is NaN wherever the curve is not drawn (invalid or
    // coherence-blanked columns), so no mark lands on what is not shown.
    let max = find_extremes(&d.freqs, &d.magnitude_db, Extreme::Max, true);
    let min = find_extremes(&d.freqs, &d.magnitude_db, Extreme::Min, true);
    let comb = peaks::comb_spacing(&min).map(|spacing_hz| Comb {
        spacing_hz,
        delay_s: 1.0 / spacing_hz,
        text: crate::readout::comb_text(spacing_hz, temp_c),
    });
    let (max, min) = cache.track_extremes(d.key, &max, &min);
    let color = d
        .color
        .with_alpha(if d.is_stale() { theme.stale_alpha } else { 1.0 });
    let mut rows = |found: &[Peak], dir: Extreme| -> Vec<PeakRow> {
        found
            .iter()
            .enumerate()
            .map(|(k, p)| {
                let (x, y) = (xm.to_px(p.freq_hz), ym.to_px(p.level));
                let inside = x.is_finite()
                    && y.is_finite()
                    && x >= plot.x
                    && x <= plot.right()
                    && y >= plot.y
                    && y <= plot.bottom();
                let mark = inside.then(|| Rect::new(x - MARK, y - MARK, 2.0 * MARK, 2.0 * MARK));
                if let Some(m) = mark {
                    // ▲ on a peak, ▼ on a dip: the point of the triangle says which way the
                    // curve turns there.
                    let (tip, base) = match dir {
                        Extreme::Max => (m.y, m.bottom()),
                        Extreme::Min => (m.bottom(), m.y),
                    };
                    c.overlay.polylines.push(Polyline {
                        points: vec![[x, tip], [m.right(), base], [m.x, base], [x, tip]],
                        alpha: vec![],
                        stroke: Stroke::solid(color, 1.5),
                        clip: Some(plot),
                    });
                    if ranks {
                        let (ry, v) = match dir {
                            Extreme::Max => (m.y - 2.0, VAlign::Bottom),
                            Extreme::Min => (m.bottom() + 2.0, VAlign::Top),
                        };
                        c.overlay.labels.push(label(
                            (k + 1).to_string(),
                            [x, ry],
                            anchor(HAlign::Center, v),
                            theme.small_font_size,
                            theme.text,
                        ));
                    }
                }
                PeakRow {
                    freq_hz: p.freq_hz,
                    level: p.level,
                    text: format!(
                        "{}  {}  {}",
                        k + 1,
                        format::freq_readout(p.freq_hz),
                        format::db_readout(p.level)
                    ),
                    mark,
                }
            })
            .collect()
    };
    let max_rows = rows(&max, Extreme::Max);
    let min_rows = rows(&min, Extreme::Min);
    Some(TfExtremes {
        key: d.key,
        name: d.name.clone(),
        max: max_rows,
        min: min_rows,
        comb,
    })
}

/// The list as drawn: the curve's name when others are shown beside it, then `max` and
/// `min` with their rows, then the comb.
pub(super) fn list_text(e: &TfExtremes, named: bool) -> String {
    let mut lines = Vec::new();
    if named {
        lines.push(e.name.clone());
    }
    for (head, rows) in [("max", &e.max), ("min", &e.min)] {
        if !rows.is_empty() {
            lines.push(head.to_string());
            lines.extend(rows.iter().map(|r| r.text.clone()));
        }
    }
    if let Some(c) = &e.comb {
        lines.push(c.text.clone());
    }
    lines.join("\n")
}

/// Draws `text` on a plate in a bottom corner of `plot` clear of the legend and the cursor
/// values (`avoid`): bottom-right first, as the legend starts top-left and the cursor values
/// top-right. Returns the plate.
pub(super) fn draw_list(
    c: &mut Canvas,
    text: &str,
    plot: Rect,
    avoid: &[Rect],
    theme: &Theme,
) -> Rect {
    let font = theme.small_font_size;
    let lines: Vec<&str> = text.split('\n').collect();
    let w = lines
        .iter()
        .map(|l| text_width(l, font))
        .fold(0.0f32, f32::max)
        + 2.0 * PAD_X;
    let h = lines.len() as f32 * ROW + 2.0 * PAD_Y;
    let y = plot.bottom() - INSET - h;
    let right = Rect::new(plot.right() - INSET - w, y, w, h);
    let left = Rect::new(plot.x + INSET, y, w, h);
    let clear = |r: &Rect| {
        !avoid
            .iter()
            .any(|a| r.x < a.right() && a.x < r.right() && r.y < a.bottom() && a.y < r.bottom())
    };
    let rect = [right, left].into_iter().find(clear).unwrap_or(right);
    crate::legend::plate(&mut c.overlay, rect, false, plot, theme);
    for (k, l) in lines.iter().enumerate() {
        let mut lb = label(
            (*l).to_string(),
            [rect.x + PAD_X, rect.y + PAD_Y + ROW * (k as f32 + 0.5)],
            anchor(HAlign::Left, VAlign::Center),
            font,
            theme.text,
        );
        lb.clip = Some(plot);
        c.overlay.labels.push(lb);
    }
    rect
}
