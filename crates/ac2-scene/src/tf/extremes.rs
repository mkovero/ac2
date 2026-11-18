//! Peak tracking on the transfer magnitude: the highest peaks and deepest dips of one
//! curve, marked ▲ / ▼ where they are and listed on the peak list's plate. Dips explain a
//! comb (a delayed copy summed in) at a glance; when four or more are evenly spaced the
//! list names the spacing and the delay it implies.

use super::*;
use crate::plate::{Metrics, PlateBox, PlateHover, PlateKind, PlatePlace};
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

/// The curve peak tracking, the cursor's mark and jumps follow: the selected one when
/// shown, else the first of the pane's own (not compared), else the first. One curve, so
/// the marks and the list stay one statement.
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
    marks: bool,
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
                // At traces only the peaks are still tracked (their order holds), not marked.
                let mark = crate::peak_marks::mark_rect(
                    xm.to_px(p.freq_hz),
                    ym.to_px(p.level),
                    crate::peak_marks::TRIANGLE_HALF,
                    plot,
                    marks,
                );
                if let Some(m) = mark {
                    crate::peak_marks::triangle(c, m, k + 1, dir, color, plot, theme);
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
    crate::peak_marks::list_text(
        named.then_some(e.name.as_str()),
        &e.max,
        &e.min,
        e.comb.as_ref().map(|c| c.text.as_str()),
    )
}

/// A `w` × `h` plate's place in a bottom corner of `plot` clear of the legend and the
/// cursor values (`avoid`): bottom-right first, as the legend starts top-left and the
/// cursor values top-right.
pub(super) fn corner(plot: Rect, avoid: &[Rect], w: f32, h: f32) -> Rect {
    let y = plot.bottom() - INSET - h;
    let right = Rect::new(plot.right() - INSET - w, y, w, h);
    let left = Rect::new(plot.x + INSET, y, w, h);
    let clear = |r: &Rect| {
        !avoid
            .iter()
            .any(|a| r.x < a.right() && a.x < r.right() && r.y < a.bottom() && a.y < r.bottom())
    };
    [right, left].into_iter().find(clear).unwrap_or(right)
}

/// Draws the cursor mark's `text` on its plate, one the operator moves and scales
/// (`place`, with the pointer on it or not): where it was dropped, else in a bottom
/// [`corner`] of `plot`. Returns the plate.
pub(super) fn draw_mark(
    c: &mut Canvas,
    text: &str,
    plot: Rect,
    avoid: &[Rect],
    place: &PlatePlace,
    hover: Option<PlateHover>,
    theme: &Theme,
) -> PlateBox {
    let set = place.scale();
    let lines: Vec<(String, Color)> = text
        .split('\n')
        .map(|l| (l.to_string(), theme.text))
        .collect();
    let texts: Vec<&str> = lines.iter().map(|l| l.0.as_str()).collect();
    let (w, h) = Metrics::new(theme, set).size(&texts);
    let s = crate::plate::fitted(set, plot, w, h);
    let m = Metrics::new(theme, s);
    let (w, h) = m.size(&texts);
    let rect = match place.at {
        Some(at) => PlatePlace::rect_at(at, plot, w, h),
        None => corner(plot, avoid, w, h),
    };
    crate::plate::draw_lines(
        &mut c.overlay,
        rect,
        &lines,
        HAlign::Left,
        m,
        true,
        hover,
        plot,
        theme,
    );
    PlateBox::new(PlateKind::Mark, plot, rect, s)
}
