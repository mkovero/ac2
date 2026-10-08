//! Transfer-function view: magnitude, phase and coherence panes sharing one log-frequency
//! axis, with legend, comparison cursor, delay readout and the banner strip above them.
//! Coherence has its own pane by default or is overlaid on the magnitude pane
//! ([`CoherencePlacement`], [`CoherenceOverlay`]).

use crate::axis::{self, Axis, Range, Steps};
use crate::banner::{BannerRow, Status};
use crate::canvas::{
    self, Canvas, MARGINS, PANE_GAP, anchor, gapped, label, text_width, visible_columns,
};
use crate::format;
use crate::legend::{INSET, LegendBox, PAD_X, PAD_Y, ROW, RowStyle};
use crate::primitives::{
    Color, Dash, FillRect, HAlign, Layer, Polyline, Rect, Scene, Stroke, VAlign, Viewport,
};
use crate::readout::{self, CursorReadout};
use crate::theme::Theme;
use crate::trace::{
    DisplayCache, DisplayTrace, PhaseReference, PhaseRelation, TfTrace, TraceKey, display_traces,
    group_delay_span,
};
use crate::view::{CoherencePlacement, PhaseView, TfView, ViewState};

/// Relative heights of the panes.
const WEIGHT_MAGNITUDE: f32 = 3.0;
const WEIGHT_PHASE: f32 = 2.0;
const WEIGHT_COHERENCE: f32 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TfPaneKind {
    Magnitude,
    Phase,
    Coherence,
}

/// One placed pane.
#[derive(Clone, Debug, PartialEq)]
pub struct TfPane {
    pub kind: TfPaneKind,
    pub plot: Rect,
    pub y_axis: Axis,
    pub title: String,
}

/// Legend line of one trace.
#[derive(Clone, Debug, PartialEq)]
pub struct LegendEntry {
    pub key: TraceKey,
    pub name: String,
    /// Short flags: `ref`, `indep.`, `inv`, `+3.0 dB`, `Δt +1.50 ms`, `+0.25 ms from arrival`,
    /// `STALE 3.2 s`.
    pub tags: Vec<String>,
    /// `name · tag · tag`, as drawn.
    pub text: String,
    pub stale: bool,
    /// The selected stored trace: marked in the legend (a bar before a thicker swatch) and
    /// drawn with a thicker line.
    pub selected: bool,
}

/// Line width of the selected trace, times the theme's trace width.
pub(crate) const SELECTED_WIDTH: f32 = 2.0;

/// Draws one legend entry's swatch (a thicker one, and a bar in the text colour before it,
/// for the selected trace) with its swatch's left edge at `x`, centred on `y`. The bar is
/// drawn, not a glyph: the plot font has no arrow-like marks.
#[allow(clippy::too_many_arguments)]
pub(crate) fn legend_swatch(
    layer: &mut Layer,
    x: f32,
    y: f32,
    w: f32,
    color: crate::primitives::Color,
    selected: bool,
    clip: Option<Rect>,
    theme: &Theme,
) {
    let h = if selected { 3.0 * SELECTED_WIDTH } else { 3.0 };
    layer.rects.push(FillRect {
        rect: Rect::new(x, y - h / 2.0, w, h),
        color,
        clip,
    });
    if selected {
        layer.rects.push(FillRect {
            rect: Rect::new(x - 5.0, y - 5.0, 2.0, 10.0),
            color: theme.text,
            clip,
        });
    }
}

/// Everything the transfer view shows, as data plus the scene.
#[derive(Clone, Debug, PartialEq)]
pub struct TfScene {
    pub scene: Scene,
    pub x_axis: Axis,
    pub panes: Vec<TfPane>,
    pub reference: Option<PhaseReference>,
    pub traces: Vec<DisplayTrace>,
    pub legend: Vec<LegendEntry>,
    /// Where the legend went (`None`: hidden or nothing to list): the UI moves, resizes and
    /// scrolls it by this.
    pub legend_box: Option<LegendBox>,
    pub cursor: Option<CursorReadout>,
    /// The cursor values' plate.
    pub readout_box: Option<Rect>,
    /// Reference trace's delay: `ref m1 12.34 ms · 4.24 m @ 20 °C`.
    pub delay: Option<String>,
    /// Present when coherence is drawn over the magnitude pane.
    pub coherence_overlay: Option<CoherenceOverlay>,
    /// Banner strip above the panes; zero height when no banner is up.
    pub strip: Rect,
    pub banners: Vec<BannerRow>,
}

/// `nudge_s`: a stored trace's delay from its arrival (its display nudge); `stepped_s`: a
/// live curve's delay from its measured arrival, already in its columns and not part of
/// `Δ`. Either is tagged the same way: `+0.10 ms from arrival`.
fn legend_entry(t: &DisplayTrace, nudge_s: f64, stepped_s: f64, selected: bool) -> LegendEntry {
    let mut tags: Vec<String> = t.note.iter().cloned().collect();
    match t.relation {
        PhaseRelation::Reference => tags.push("ref".to_string()),
        PhaseRelation::Independent => tags.push("indep.".to_string()),
        PhaseRelation::Relative => {
            // Arrival relative to the reference = Δ + nudge (Δ already removed the nudge).
            tags.push(format!("Δt {}", signed_ms(t.shift_s + nudge_s)));
        }
    }
    tags.extend(format::from_arrival(nudge_s + stepped_s));
    if t.inverted {
        tags.push("inv".to_string());
    }
    if t.offset_db != 0.0 {
        tags.push(format::db_readout(t.offset_db));
    }
    if t.smoothing.is_some() {
        tags.push(format::smoothing(t.smoothing));
    }
    if let Some(tag) = t.freshness.and_then(|f| f.tag()) {
        tags.push(tag);
    }
    let mut text = t.name.clone();
    for tag in &tags {
        text.push_str(" · ");
        text.push_str(tag);
    }
    LegendEntry {
        key: t.key,
        name: t.name.clone(),
        tags,
        text,
        stale: t.is_stale(),
        selected,
    }
}

/// What the transfer pane's title says about smoothing of the trace its keys act on:
/// `smoothing 1/6 oct`, `smoothing off`.
pub fn smoothing_caption(s: Option<ac2_proto::model::Smoothing>) -> String {
    format!("smoothing {}", format::smoothing(s))
}

/// The span the shown traces' group delay is fitted over: `1/12 oct`, or `1/12–1/6 oct`
/// when their smoothing gives them different spans.
fn group_delay_span_text(shown: &[DisplayTrace]) -> String {
    let spans = shown.iter().map(|t| group_delay_span(t.smoothing));
    let narrowest = spans.clone().max().unwrap_or(group_delay_span(None));
    let widest = spans.min().unwrap_or(narrowest);
    if narrowest == widest {
        format!("1/{widest} oct")
    } else {
        format!("1/{narrowest}–1/{widest} oct")
    }
}

fn signed_ms(s: f64) -> String {
    format!("{} ms", format::signed(s * 1000.0, 2))
}

fn trace_stroke(t: &DisplayTrace, selected: bool, theme: &Theme) -> Stroke {
    let a = if t.is_stale() { theme.stale_alpha } else { 1.0 };
    let w = if selected { SELECTED_WIDTH } else { 1.0 };
    Stroke::solid(t.color.with_alpha(a), theme.trace_width * w)
}

/// Coherence overlaid on the magnitude pane: γ² 0…1 maps linearly onto `band`, the top
/// [`OVERLAY_FRACTION`] of the pane, so it stays clear of the 0 dB region where most
/// magnitude traces sit. Its axis is drawn in the right margin, outside the plot, where the
/// legend and cursor values cannot collide with it.
#[derive(Clone, Debug, PartialEq)]
pub struct CoherenceOverlay {
    pub band: Rect,
    pub axis: Axis,
}

/// Share of the magnitude pane height given to the overlaid coherence.
pub const OVERLAY_FRACTION: f32 = 0.3;
/// Gap between the pane's top border and γ² = 1, so a fully coherent trace does not merge
/// into the border line.
pub const OVERLAY_INSET: f32 = 4.0;
/// Right margin when the overlay axis needs room for `0.5`.
const OVERLAY_MARGIN_RIGHT: f32 = 30.0;
/// Opacity of the overlaid coherence trace, so it does not read as a magnitude trace of the
/// same colour.
pub const OVERLAY_ALPHA: f32 = 0.6;

fn overlaid(view: &ViewState) -> bool {
    view.tf.show_coherence
        && view.tf.show_magnitude
        && view.tf.coherence_placement == CoherencePlacement::OverlayOnMagnitude
}

/// Pane rectangles for the enabled panes, top to bottom, below `top`.
fn layout(view: &ViewState, size: Viewport, top: f32, right: f32) -> Vec<(TfPaneKind, Rect)> {
    let mut kinds = Vec::new();
    if view.tf.show_magnitude {
        kinds.push((TfPaneKind::Magnitude, WEIGHT_MAGNITUDE));
    }
    if view.tf.show_phase {
        kinds.push((TfPaneKind::Phase, WEIGHT_PHASE));
    }
    if view.tf.show_coherence && !overlaid(view) {
        kinds.push((TfPaneKind::Coherence, WEIGHT_COHERENCE));
    }
    let x = MARGINS.left;
    let w = (size.width - MARGINS.left - right).max(1.0);
    let gaps = PANE_GAP * kinds.len().saturating_sub(1) as f32;
    let h = (size.height - top - MARGINS.top - MARGINS.bottom - gaps).max(1.0);
    let total: f32 = kinds.iter().map(|k| k.1).sum();
    let mut y = top + MARGINS.top;
    kinds
        .into_iter()
        .map(|(k, wgt)| {
            let ph = h * wgt / total;
            let r = Rect::new(x, y, w, ph);
            y += ph + PANE_GAP;
            (k, r)
        })
        .collect()
}

/// One trace's values on one mapping, as a gapped polyline; `None` when nothing is visible.
fn trace_line(
    xs: &[f32],
    values: &[f64],
    to_px: impl Fn(f64) -> f32,
    alpha: Option<&[f32]>,
    wrapped: bool,
    stroke: Stroke,
    clip: Rect,
) -> Option<Polyline> {
    let ys: Vec<f32> = values.iter().map(|v| to_px(*v)).collect();
    let (points, alpha) = gapped(xs, &ys, alpha, |a, b| {
        wrapped && (values[b] - values[a]).abs() > 180.0
    });
    (!points.is_empty()).then_some(Polyline {
        points,
        alpha,
        stroke,
        clip: Some(clip),
    })
}

/// Right-margin axis of the overlaid coherence and a dashed line at γ² = 0.
fn overlay_frame(c: &mut Canvas, plot: Rect, o: &CoherenceOverlay, theme: &Theme) {
    let floor = Stroke {
        dash: Some(Dash {
            on: 3.0,
            off: 3.0,
            offset: 0.0,
        }),
        ..theme.grid_major
    };
    canvas::hline(c, plot, o.band.bottom(), floor);
    for t in &o.axis.ticks {
        if let Some(text) = &t.label {
            c.base.labels.push(label(
                text.clone(),
                [plot.right() + 4.0, t.pos],
                anchor(HAlign::Left, VAlign::Center),
                theme.small_font_size,
                theme.axis_text,
            ));
        }
    }
    c.base.labels.push(label(
        o.axis.title.clone(),
        [plot.right() + 4.0, o.band.bottom() + 8.0],
        anchor(HAlign::Left, VAlign::Top),
        theme.small_font_size,
        theme.text_dim,
    ));
}

/// The cursor values on a plate of their own: each on its trace's legend row, on the side of
/// the plot away from the legend, the frequency above them (below when the pane title or the
/// plot's top leaves no room). When that plate would run into the legend (a narrow pane,
/// long names) the values go under the legend, or over it when it sits low, in the same
/// order. With the legend hidden each value names its trace. Returns the plate.
#[allow(clippy::too_many_arguments)]
fn cursor_values(
    c: &mut Canvas,
    cr: &CursorReadout,
    shown: &[DisplayTrace],
    legend: Option<&LegendBox>,
    area: Rect,
    block: f32,
    clip: Rect,
    theme: &Theme,
) -> Option<Rect> {
    let font = theme.small_font_size;
    let values = |r: &readout::CursorRow| format!("{}  {}  {}", r.magnitude, r.phase, r.coherence);
    let lines = cr.rows.iter().filter_map(|r| {
        let i = shown.iter().position(|t| t.key == r.key)?;
        let text = match legend {
            Some(_) => values(r),
            None => format!("{}  {}", shown[i].name, values(r)),
        };
        Some((text, trace_stroke(&shown[i], false, theme).color, i))
    });
    let first_y = area.y + INSET + PAD_Y + ROW * 1.5;
    let (right_side, mut rows): (bool, Vec<(String, Color, f32)>) = match legend {
        Some(b) => (
            b.rect.x + b.rect.w / 2.0 <= area.x + area.w / 2.0,
            lines
                .filter_map(|(t, col, i)| b.row_y(i).map(|y| (t, col, y)))
                .collect(),
        ),
        None => (
            true,
            lines
                .enumerate()
                .map(|(k, (t, col, _))| (t, col, first_y + k as f32 * ROW))
                .take_while(|l| l.2 + ROW / 2.0 + PAD_Y <= area.bottom() - INSET)
                .collect(),
        ),
    };
    let w = rows
        .iter()
        .map(|l| text_width(&l.0, font))
        .chain([text_width(&cr.freq, font)])
        .fold(0.0f32, f32::max)
        + 2.0 * PAD_X;
    let x = if right_side {
        area.right() - INSET - w
    } else {
        area.x + INSET
    };
    if let Some(b) = legend
        && !rows.is_empty()
        && x < b.rect.right()
        && x + w > b.rect.x
    {
        let need = (rows.len() + 1) as f32 * ROW + 2.0 * PAD_Y;
        let top = if b.rect.bottom() + 2.0 + need <= area.bottom() {
            b.rect.bottom() + 2.0
        } else {
            b.rect.y - 2.0 - need
        };
        for (k, l) in rows.iter_mut().enumerate() {
            l.2 = top + PAD_Y + ROW * (k as f32 + 1.5);
        }
    }
    let lo = rows.iter().map(|l| l.2).fold(f32::INFINITY, f32::min);
    let hi = rows.iter().map(|l| l.2).fold(f32::NEG_INFINITY, f32::max);
    // The pane title is at the left: on the right the frequency may share its line.
    let ceiling = if right_side { block } else { area.y };
    let freq_y = if rows.is_empty() {
        area.y + INSET + PAD_Y + ROW / 2.0
    } else if lo - ROW * 1.5 - PAD_Y >= ceiling {
        lo - ROW
    } else {
        hi + ROW
    };
    let (top, bottom) = (lo.min(freq_y), hi.max(freq_y));
    let rect = Rect::new(
        x,
        top - ROW / 2.0 - PAD_Y,
        w,
        bottom - top + ROW + 2.0 * PAD_Y,
    );
    crate::legend::plate(&mut c.overlay, rect, false, clip, theme);
    let (tx, h) = if right_side {
        (rect.right() - PAD_X, HAlign::Right)
    } else {
        (rect.x + PAD_X, HAlign::Left)
    };
    for (text, color, y) in std::iter::once((cr.freq.clone(), theme.text, freq_y)).chain(rows) {
        let mut l = label(text, [tx, y], anchor(h, VAlign::Center), font, color);
        l.clip = Some(clip);
        c.overlay.labels.push(l);
    }
    Some(rect)
}

/// Builds the transfer view.
pub fn transfer_scene(
    traces: &[TfTrace<'_>],
    cache: &DisplayCache,
    status: &Status,
    view: &ViewState,
    theme: &Theme,
    size: Viewport,
) -> TfScene {
    let mut c = Canvas::new(size, theme);
    let (reference, shown) =
        display_traces(traces, cache, view.tf.phase_reference, &view.tf.coherence);
    let selected = traces.iter().find(|t| t.selected).map(|t| t.key);
    let overlay = overlaid(view);
    let right = if overlay {
        OVERLAY_MARGIN_RIGHT
    } else {
        MARGINS.right
    };
    let plot_x = MARGINS.left;
    let plot_w = (size.width - MARGINS.left - right).max(1.0);
    let strip = canvas::banner_strip(&mut c, status, plot_x, plot_w, size, theme);
    let panes_at = layout(view, size, strip.rect.bottom(), right);
    // Tick steps follow the pane heights of the default (coherence-pane) layout in both
    // placements, so toggling the overlay changes no axis labels.
    let pane_view = ViewState {
        tf: TfView {
            coherence_placement: CoherencePlacement::Pane,
            ..view.tf
        },
        ..*view
    };
    let density_of: Vec<(TfPaneKind, f32)> =
        layout(&pane_view, size, strip.rect.bottom(), MARGINS.right)
            .into_iter()
            .map(|(k, r)| (k, r.h))
            .collect();
    let x_axis = axis::freq_axis(view.freq.range(), plot_x, plot_x + plot_w);
    let xm = x_axis.mapping;
    let last = panes_at.len().saturating_sub(1);

    let mut panes = Vec::new();
    let mut coherence_overlay = None;
    for (pi, (kind, plot)) in panes_at.iter().copied().enumerate() {
        let density = density_of
            .iter()
            .find(|(k, _)| *k == kind)
            .map_or(plot.h, |(_, h)| *h);
        let y = |range: Range, title: &str, steps: Steps| {
            axis::axis_with_density(range, plot.bottom(), plot.y, title, steps, density)
        };
        let (y_axis, title) = match kind {
            TfPaneKind::Magnitude => (
                y(view.tf.magnitude_db, "dB", Steps::Decimal),
                "Magnitude dB".to_string(),
            ),
            TfPaneKind::Phase => match view.tf.phase {
                PhaseView::Wrapped => (
                    y(Range::new(-180.0, 180.0), "°", Steps::Degrees),
                    "Phase °".to_string(),
                ),
                PhaseView::Unwrapped { range } => (
                    y(range, "°", Steps::Degrees),
                    "Phase ° (unwrapped)".to_string(),
                ),
                PhaseView::GroupDelay { range_ms } => (
                    y(range_ms, "ms", Steps::Decimal),
                    format!("Group delay ms · {}", group_delay_span_text(&shown)),
                ),
            },
            TfPaneKind::Coherence => (
                axis::linear_axis(Range::new(0.0, 1.0), plot.bottom(), plot.y, "γ²"),
                "Coherence γ²".to_string(),
            ),
        };
        let band = (overlay && kind == TfPaneKind::Magnitude).then(|| {
            let band = Rect::new(
                plot.x,
                plot.y + OVERLAY_INSET,
                plot.w,
                (plot.h * OVERLAY_FRACTION - OVERLAY_INSET).max(1.0),
            );
            CoherenceOverlay {
                band,
                axis: axis::linear_axis(Range::new(0.0, 1.0), band.bottom(), band.y, "γ²"),
            }
        });
        // The title (and, below, the legend) sits under the overlay band so the coherence
        // curve never runs through text.
        let title_at = [
            plot.x + 6.0,
            band.as_ref().map_or(plot.y, |o| o.band.bottom()) + 4.0,
        ];
        canvas::pane_frame_at(
            &mut c,
            plot,
            &x_axis,
            &y_axis,
            pi == last,
            &title,
            title_at,
            theme,
        );
        let ym = y_axis.mapping;
        if kind != TfPaneKind::Coherence {
            canvas::hline(&mut c, plot, ym.to_px(0.0), theme.zero_line);
        }
        if let Some(o) = &band {
            overlay_frame(&mut c, plot, o, theme);
        }

        for t in &shown {
            let cols = visible_columns(&t.freqs, xm.range.lo, xm.range.hi);
            let xs: Vec<f32> = t.freqs[cols.clone()].iter().map(|f| xm.to_px(*f)).collect();
            let values: &[f64] = match kind {
                TfPaneKind::Magnitude => &t.magnitude_db,
                TfPaneKind::Phase => match view.tf.phase {
                    PhaseView::Wrapped => &t.phase_wrapped_deg,
                    PhaseView::Unwrapped { .. } => &t.phase_unwrapped_deg,
                    PhaseView::GroupDelay { .. } => &t.group_delay_s,
                },
                TfPaneKind::Coherence => &t.coherence,
            };
            let scale = match (kind, view.tf.phase) {
                (TfPaneKind::Phase, PhaseView::GroupDelay { .. }) => 1000.0,
                _ => 1.0,
            };
            // The coherence trace itself is never faded by coherence.
            let alpha = (kind != TfPaneKind::Coherence && view.tf.coherence.alpha)
                .then(|| &t.alpha[cols.clone()]);
            let wrapped = kind == TfPaneKind::Phase && view.tf.phase == PhaseView::Wrapped;
            let stroke = trace_stroke(t, selected == Some(t.key), theme);
            c.data.polylines.extend(trace_line(
                &xs,
                &values[cols.clone()],
                |v| ym.to_px(v * scale),
                alpha,
                wrapped,
                stroke,
                plot,
            ));
            if let Some(o) = &band {
                let om = o.axis.mapping;
                let stroke = Stroke {
                    color: stroke.color.with_alpha(OVERLAY_ALPHA),
                    ..stroke
                };
                c.data.polylines.extend(trace_line(
                    &xs,
                    &t.coherence[cols.clone()],
                    |v| om.to_px(v),
                    None,
                    false,
                    stroke,
                    plot,
                ));
            }
        }
        if band.is_some() {
            coherence_overlay = band;
        }
        panes.push(TfPane {
            kind,
            plot,
            y_axis,
            title,
        });
    }

    // Legend and cursor values, top-left / top-right of the first pane.
    let nudges: Vec<(f64, f64)> = traces
        .iter()
        .map(|t| (t.nudge.0, t.delay_nudge.0))
        .collect();
    let legend: Vec<LegendEntry> = shown
        .iter()
        .zip(&nudges)
        .map(|(t, (n, s))| legend_entry(t, *n, *s, selected == Some(t.key)))
        .collect();
    let cursor = view
        .cursor_hz
        .and_then(|hz| readout::cursor_readout(&shown, hz, view.tf.phase));
    let delay = reference.as_ref().map(|r| {
        let name = shown
            .iter()
            .find(|t| t.key == r.key)
            .map_or(String::new(), |t| t.name.clone());
        format!(
            "ref {name} {}",
            readout::delay_readout(r.delay.0, view.temperature_c)
        )
    });
    let mut legend_box = None;
    let mut readout_box = None;
    if let Some(&(_, top)) = panes_at.first() {
        // The text starts under the pane title, which in overlay mode sits under the
        // coherence band.
        let block = coherence_overlay
            .as_ref()
            .map_or(top.y, |o: &CoherenceOverlay| o.band.bottom());
        let area_top = block + 16.0;
        let area = Rect::new(top.x, area_top, top.w, (top.bottom() - area_top).max(1.0));
        let texts: Vec<&str> = legend.iter().map(|e| e.text.as_str()).collect();
        let notes: Vec<String> = delay.iter().cloned().collect();
        let placed =
            crate::legend::place(&view.tf.legend, area, &texts, &notes, theme.small_font_size);
        if let Some(b) = &placed {
            let styles: Vec<RowStyle> = legend
                .iter()
                .zip(&shown)
                .map(|(e, t)| RowStyle {
                    color: trace_stroke(t, e.selected, theme).color,
                    dim: e.stale,
                    selected: e.selected,
                })
                .collect();
            crate::legend::draw(&mut c.overlay, b, &styles, view.tf.legend.hover, top, theme);
        }
        if let Some(cr) = &cursor {
            readout_box =
                cursor_values(&mut c, cr, &shown, placed.as_ref(), area, block, top, theme);
            // Under the plates: the line runs behind the legend, never through its text.
            for (_, plot) in &panes_at {
                canvas::vline(&mut c.data, *plot, xm.to_px(cr.freq_hz), theme.cursor);
            }
        }
        legend_box = placed;
    }

    TfScene {
        scene: c.into_scene(size),
        x_axis,
        panes,
        reference,
        traces: shown,
        legend,
        legend_box,
        cursor,
        readout_box,
        delay,
        coherence_overlay,
        strip: strip.rect,
        banners: strip.rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::tests::segments;
    use crate::time::Freshness;
    use crate::trace::TimeBase;
    use ac2_proto::frame::ValidityMask;
    use ac2_proto::model::{Polarity, Smoothing, SmoothingFraction, SmoothingMode};
    use ac2_proto::units::{MeasId, Seconds, SessionEpoch, TraceId};

    const SIZE: Viewport = Viewport {
        width: 960.0,
        height: 600.0,
    };

    struct Cols {
        freqs: Vec<f64>,
        mag: Vec<f32>,
        phase: Vec<f32>,
        coh: Vec<f32>,
        validity: Vec<ValidityMask>,
    }

    fn cols(n: usize) -> Cols {
        // 1/12 octave around 1 kHz, with a column at exactly 1 kHz.
        let freqs: Vec<f64> = (0..n)
            .map(|i| 1000.0 * 2f64.powf((i as f64 - (n / 2) as f64) / 12.0))
            .collect();
        Cols {
            mag: vec![0.0; n],
            phase: vec![0.0; n],
            coh: vec![1.0; n],
            validity: vec![ValidityMask::NONE; n],
            freqs,
        }
    }

    fn trace(c: &Cols, key: TraceKey, delay: f64) -> TfTrace<'_> {
        TfTrace {
            key,
            name: match key {
                TraceKey::Live(m) => format!("m{}", m.0),
                TraceKey::Stored(t) => format!("t{}", t.0),
            },
            color: Theme::dark().trace_color(0),
            freqs: &c.freqs,
            mag_db: &c.mag,
            phase_deg: Some(&c.phase),
            coherence: Some(&c.coh),
            validity: Some(&c.validity),
            offset_db: 0.0,
            polarity: Polarity::Normal,
            nudge: Seconds(0.0),
            delay_nudge: Seconds(0.0),
            time_base: TimeBase::Shared {
                epoch: SessionEpoch(1),
                delay: Seconds(delay),
            },
            freshness: Some(Freshness::from_age(0.1)),
            smoothing: None,
            note: None,
            stored: None,
            selected: false,
        }
    }

    #[test]
    fn three_panes_share_the_x_axis() {
        let c = cols(100);
        let s = transfer_scene(
            &[trace(&c, TraceKey::Live(MeasId(1)), 0.0)],
            &DisplayCache::default(),
            &Status::default(),
            &ViewState::default(),
            &Theme::dark(),
            SIZE,
        );
        let kinds: Vec<_> = s.panes.iter().map(|p| p.kind).collect();
        assert_eq!(
            kinds,
            [
                TfPaneKind::Magnitude,
                TfPaneKind::Phase,
                TfPaneKind::Coherence
            ]
        );
        // Heights 3:2:1, no overlap.
        let h: Vec<f32> = s.panes.iter().map(|p| p.plot.h).collect();
        assert!((h[0] / h[2] - 3.0).abs() < 1e-4 && (h[1] / h[2] - 2.0).abs() < 1e-4);
        assert!(s.panes[1].plot.y >= s.panes[0].plot.bottom());
        assert_eq!(
            s.x_axis.labels(),
            [
                "20", "50", "100", "200", "500", "1k", "2k", "5k", "10k", "20k"
            ]
        );
        assert_eq!(
            s.panes[1].y_axis.labels(),
            ["−180", "−90", "0", "90", "180"]
        );
        assert_eq!(s.panes[2].title, "Coherence γ²");
        // One polyline per pane for the one trace; x labels only under the bottom pane.
        assert_eq!(s.scene.layers[1].polylines.len(), 3);
        let x_labels = s.scene.layers[0]
            .labels
            .iter()
            .filter(|l| l.text == "1k")
            .count();
        assert_eq!(x_labels, 1);
        assert!(s.banners.is_empty());
    }

    #[test]
    fn invalid_columns_split_the_line() {
        let mut c = cols(100);
        c.validity[40] = ValidityMask::THINNED;
        c.validity[41] = ValidityMask::THINNED;
        c.mag[70] = f32::NAN;
        let s = transfer_scene(
            &[trace(&c, TraceKey::Live(MeasId(1)), 0.0)],
            &DisplayCache::default(),
            &Status::default(),
            &ViewState::default(),
            &Theme::dark(),
            SIZE,
        );
        let mag = &s.scene.layers[1].polylines[0];
        let segs = segments(&mag.points);
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].len(), 40);
        assert_eq!(segs[1].len(), 28);
        assert_eq!(segs[2].len(), 29);
        assert_eq!(mag.alpha.len(), mag.points.len());
        // Phase and coherence panes break at the same columns: a column without a valid
        // magnitude is invalid as a whole.
        assert_eq!(segments(&s.scene.layers[1].polylines[1].points).len(), 3);
        let coh = &s.scene.layers[1].polylines[2];
        assert_eq!(segments(&coh.points).len(), 3);
    }

    #[test]
    fn wrapped_phase_breaks_at_the_wrap() {
        let mut c = cols(200);
        for (i, f) in c.freqs.iter().enumerate() {
            c.phase[i] = crate::trace::wrap_deg(-360.0 * f * 0.000_25) as f32;
        }
        let s = transfer_scene(
            &[trace(&c, TraceKey::Live(MeasId(1)), 0.0)],
            &DisplayCache::default(),
            &Status::default(),
            &ViewState::default(),
            &Theme::dark(),
            SIZE,
        );
        let phase = &s.scene.layers[1].polylines[1];
        let segs = segments(&phase.points);
        // 0.25 ms up to 17 kHz wraps four times; no segment jumps across the pane.
        assert!(segs.len() >= 4, "{}", segs.len());
        let plot = s.panes[1].plot;
        for seg in &segs {
            for w in seg.windows(2) {
                assert!((w[1][1] - w[0][1]).abs() < plot.h / 2.0);
            }
        }
    }

    #[test]
    fn coherence_alpha_reaches_the_polyline() {
        let mut c = cols(50);
        c.coh[10] = 0.0;
        let s = transfer_scene(
            &[trace(&c, TraceKey::Live(MeasId(1)), 0.0)],
            &DisplayCache::default(),
            &Status::default(),
            &ViewState::default(),
            &Theme::dark(),
            SIZE,
        );
        let mag = &s.scene.layers[1].polylines[0];
        assert_eq!(mag.alpha[10], 0.15);
        assert_eq!(mag.alpha[11], 1.0);
        // The coherence trace itself is never faded.
        assert!(s.scene.layers[1].polylines[2].alpha.is_empty());
    }

    #[test]
    fn smoothing_captions() {
        let s = |fraction, mode| Some(Smoothing { fraction, mode });
        assert_eq!(smoothing_caption(None), "smoothing off");
        assert_eq!(
            smoothing_caption(s(SmoothingFraction::Third, SmoothingMode::MagnitudePhase)),
            "smoothing 1/3 oct"
        );
        assert_eq!(
            smoothing_caption(s(
                SmoothingFraction::FortyEighth,
                SmoothingMode::MagnitudePhase
            )),
            "smoothing 1/48 oct"
        );
        assert_eq!(
            smoothing_caption(s(SmoothingFraction::Twelfth, SmoothingMode::Magnitude)),
            "smoothing 1/12 oct mag only"
        );
    }

    /// The selected stored trace is marked: a bar before its legend swatch, a thicker
    /// swatch, and a line twice as wide in every pane; the others stay as they are.
    #[test]
    fn the_selected_trace_is_marked_in_legend_and_plot() {
        let a = cols(97);
        let ta = trace(&a, TraceKey::Live(MeasId(1)), 0.010);
        let mut ts = trace(&a, TraceKey::Stored(TraceId(7)), 0.010);
        ts.selected = true;
        let theme = Theme::dark();
        let s = transfer_scene(
            &[ta, ts],
            &DisplayCache::default(),
            &Status::default(),
            &ViewState::default(),
            &theme,
            SIZE,
        );
        let sel: Vec<bool> = s.legend.iter().map(|e| e.selected).collect();
        assert_eq!(sel, [false, true]);
        assert_eq!(s.legend[1].text, "t7 · Δt 0.00 ms");
        let widths: Vec<f32> = s.scene.layers[1]
            .polylines
            .iter()
            .map(|p| p.stroke.width)
            .collect();
        // Magnitude, phase and coherence: the live line then the selected one, each pane.
        assert!(widths.len() >= 6, "{widths:?}");
        for pair in widths.chunks(2) {
            assert_eq!(
                pair,
                [theme.trace_width, theme.trace_width * SELECTED_WIDTH]
            );
        }
        let row = s.scene.layers[2]
            .labels
            .iter()
            .find(|l| l.text == "t7 · Δt 0.00 ms")
            .expect("legend row");
        // The two swatches (12 wide), the selected one thicker, and the bar on its row.
        let rects: Vec<Rect> = s.scene.layers[2].rects.iter().map(|r| r.rect).collect();
        let swatches: Vec<&Rect> = rects.iter().filter(|r| r.w == 12.0).collect();
        assert_eq!(swatches.len(), 2, "{rects:?}");
        assert_eq!(swatches[0].h, 3.0);
        assert_eq!(swatches[1].h, 3.0 * SELECTED_WIDTH);
        let bar = rects
            .iter()
            .find(|r| r.w == 2.0 && r.h == 10.0)
            .expect("bar");
        assert_eq!(bar.y + bar.h / 2.0, row.pos[1], "on the selected row");
        assert!(bar.right() < swatches[1].x);
        // All of it on the plate.
        let plate = s.legend_box.as_ref().expect("legend").rect;
        assert!(rects.contains(&plate));
        for r in [*swatches[0], *swatches[1], *bar] {
            assert!(
                r.x >= plate.x && r.right() <= plate.right(),
                "{r:?} in {plate:?}"
            );
        }
    }

    #[test]
    fn legend_cursor_delay_and_stale() {
        let a = cols(97);
        let b = cols(97);
        let mut ta = trace(&a, TraceKey::Live(MeasId(1)), 0.010);
        ta.name = "Main L".into();
        ta.smoothing = Some(Smoothing {
            fraction: SmoothingFraction::Sixth,
            mode: SmoothingMode::MagnitudePhase,
        });
        let mut tb = trace(&b, TraceKey::Live(MeasId(2)), 0.0115);
        tb.name = "Delay tower".into();
        tb.polarity = Polarity::Inverted;
        tb.offset_db = 3.0;
        tb.freshness = Some(Freshness::from_age(3.24));
        let mut ti = trace(&b, TraceKey::Stored(TraceId(7)), 0.0);
        ti.name = "imported".into();
        ti.time_base = TimeBase::Independent;
        ti.freshness = None;
        ti.nudge = Seconds(0.00025);
        ti.smoothing = Some(Smoothing {
            fraction: SmoothingFraction::TwentyFourth,
            mode: SmoothingMode::Magnitude,
        });
        let view = ViewState {
            cursor_hz: Some(1000.0),
            ..ViewState::default()
        };
        let status = Status {
            frame_age_s: Some(3.24),
            ..Status::default()
        };
        let s = transfer_scene(
            &[ta, tb, ti],
            &DisplayCache::default(),
            &status,
            &view,
            &Theme::dark(),
            SIZE,
        );
        let texts: Vec<&str> = s.legend.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "Main L · ref · 1/6 oct",
                "Delay tower · Δt +1.50 ms · inv · +3.0 dB · STALE 3.2 s",
                "imported · indep. · +0.25 ms from arrival · 1/24 oct mag only"
            ]
        );
        assert_eq!(
            s.delay.as_deref(),
            Some("ref Main L 10.00 ms · 3.43 m @ 20 °C")
        );
        let cur = s.cursor.as_ref().expect("cursor");
        assert_eq!(cur.freq, "1.00 kHz");
        assert_eq!(cur.rows[0].magnitude, "0.0 dB");
        assert_eq!(cur.rows[1].magnitude, "+3.0 dB");
        // Inverted and 1.5 ms later: 180° − 540° ≡ 0° at exactly 1 kHz.
        assert_eq!(cur.rows[1].phase, "0°");
        // Stale trace dims.
        let stroke = &s.scene.layers[1].polylines[1].stroke;
        assert!((stroke.color.a - Theme::dark().stale_alpha).abs() < 1e-6);
        assert_eq!(s.banners[0].text, "STALE · 3.2 s");
        // Cursor line in every pane, with the curves: under the legend's plate.
        let cursor_lines = s.scene.layers[1]
            .polylines
            .iter()
            .filter(|p| p.stroke == Theme::dark().cursor)
            .count();
        assert_eq!(cursor_lines, 3);
        assert!(s.scene.layers[2].polylines.is_empty());
        // The drawn strings include the legend and the cursor values.
        let labels: Vec<&str> = s.scene.layers[2]
            .labels
            .iter()
            .map(|l| l.text.as_str())
            .collect();
        assert!(labels.contains(&"Main L · ref · 1/6 oct"));
        assert!(labels.contains(&"1.00 kHz"));
        assert!(labels.contains(&"+3.0 dB  0°  1.00"));
    }

    /// A stopped measurement's curve is its final result: tagged, drawn at full strength.
    #[test]
    fn stopped_trace_is_tagged_not_dimmed() {
        let c = cols(100);
        let mut t = trace(&c, TraceKey::Live(MeasId(1)), 0.0);
        t.name = "Main L".into();
        t.freshness = Some(Freshness::Stopped { age_s: 23.0 });
        let s = transfer_scene(
            &[t],
            &DisplayCache::default(),
            &Status::default(),
            &ViewState::default(),
            &Theme::dark(),
            SIZE,
        );
        assert_eq!(s.legend[0].text, "Main L · ref · stopped");
        assert!(!s.legend[0].stale);
        let stroke = &s.scene.layers[1].polylines[0].stroke;
        assert_eq!(stroke.color.a, 1.0);
    }

    #[test]
    fn group_delay_pane() {
        let mut c = cols(100);
        for (i, f) in c.freqs.iter().enumerate() {
            c.phase[i] = crate::trace::wrap_deg(-360.0 * f * 0.0002) as f32;
        }
        let view = ViewState {
            tf: crate::view::TfView {
                phase: PhaseView::GroupDelay {
                    range_ms: Range::new(-2.0, 2.0),
                },
                show_coherence: false,
                ..Default::default()
            },
            cursor_hz: Some(1000.0),
            ..ViewState::default()
        };
        let s = transfer_scene(
            &[trace(&c, TraceKey::Live(MeasId(1)), 0.0)],
            &DisplayCache::default(),
            &Status::default(),
            &view,
            &Theme::dark(),
            SIZE,
        );
        assert_eq!(s.panes.len(), 2);
        assert_eq!(s.panes[1].title, "Group delay ms · 1/12 oct");
        assert_eq!(s.cursor.as_ref().expect("cursor").rows[0].phase, "0.20 ms");
        // Every phase-pane point sits at the 0.2 ms line.
        let y = s.panes[1].y_axis.mapping.to_px(0.2);
        let line = &s.scene.layers[1].polylines[1];
        assert!(line.points.iter().all(|p| (p[1] - y).abs() < 0.01));
    }

    fn overlay_view() -> ViewState {
        ViewState {
            tf: crate::view::TfView {
                coherence_placement: CoherencePlacement::OverlayOnMagnitude,
                ..Default::default()
            },
            ..ViewState::default()
        }
    }

    /// Two traces with a legend, delay line, cursor readout and every banner up.
    fn busy(view: &ViewState, status: &Status) -> TfScene {
        let a = cols(97);
        let mut ta = trace(&a, TraceKey::Live(MeasId(1)), 0.010);
        ta.name = "Main L".into();
        ta.smoothing = Some(Smoothing {
            fraction: SmoothingFraction::Sixth,
            mode: SmoothingMode::MagnitudePhase,
        });
        let mut tb = trace(&a, TraceKey::Live(MeasId(2)), 0.0115);
        tb.name = "Delay tower".into();
        tb.offset_db = 3.0;
        let view = ViewState {
            cursor_hz: Some(1000.0),
            ..*view
        };
        transfer_scene(
            &[ta, tb],
            &DisplayCache::default(),
            status,
            &view,
            &Theme::dark(),
            SIZE,
        )
    }

    #[test]
    fn banners_live_in_a_strip_above_the_panes() {
        use crate::banner::{BANNER_GAP, BANNER_HEIGHT, BANNER_PAD, MAX_BANNERS};
        use crate::canvas::tests::assert_banners_clear;
        for view in [ViewState::default(), overlay_view()] {
            let calm = busy(&view, &Status::default());
            assert!(calm.banners.is_empty());
            assert_eq!(calm.strip.h, 0.0);
            assert_eq!(calm.panes[0].plot.y, MARGINS.top);
            assert!(calm.scene.layers[3].rects.is_empty());

            let s = busy(&view, &crate::banner::tests::everything());
            assert_eq!(s.banners.len(), MAX_BANNERS);
            assert_eq!(s.banners[2].text, "+7 more");
            let strip_h = 2.0 * BANNER_PAD + 3.0 * BANNER_HEIGHT + 2.0 * BANNER_GAP;
            assert_eq!(s.strip, Rect::new(0.0, 0.0, SIZE.width, strip_h));
            // Panes start below the strip and shrink by its height, keeping their ratios.
            assert_eq!(s.panes[0].plot.y, strip_h + MARGINS.top);
            let total = |s: &TfScene| s.panes.iter().map(|p| p.plot.h).sum::<f32>();
            assert!((total(&calm) - total(&s) - strip_h).abs() < 1e-3);
            for (a, b) in calm.panes.iter().zip(&s.panes) {
                assert!((b.plot.h / a.plot.h - total(&s) / total(&calm)).abs() < 1e-4);
            }
            // Nothing drawn below the strip is covered: panes, the overlay band, legend,
            // delay line, cursor readout, axis labels and titles.
            assert!(!s.legend.is_empty() && s.cursor.is_some() && s.delay.is_some());
            let mut areas: Vec<Rect> = s.panes.iter().map(|p| p.plot).collect();
            areas.extend(s.coherence_overlay.as_ref().map(|o| o.band));
            assert_banners_clear(&s.scene, &s.banners, &areas);
            for r in &s.banners {
                assert!(r.rect.bottom() <= s.strip.bottom());
            }
        }
    }

    #[test]
    fn coherence_overlay_layout() {
        let c = cols(100);
        let t = [trace(&c, TraceKey::Live(MeasId(1)), 0.0)];
        let pane = transfer_scene(
            &t,
            &DisplayCache::default(),
            &Status::default(),
            &ViewState::default(),
            &Theme::dark(),
            SIZE,
        );
        assert!(pane.coherence_overlay.is_none());
        let s = transfer_scene(
            &t,
            &DisplayCache::default(),
            &Status::default(),
            &overlay_view(),
            &Theme::dark(),
            SIZE,
        );
        let kinds: Vec<_> = s.panes.iter().map(|p| p.kind).collect();
        assert_eq!(kinds, [TfPaneKind::Magnitude, TfPaneKind::Phase]);
        let (mag, phase) = (s.panes[0].plot, s.panes[1].plot);
        // Magnitude : phase = 3 : 2, filling the height the coherence pane had.
        assert!((mag.h / phase.h - 1.5).abs() < 1e-4);
        assert!((phase.bottom() - pane.panes[2].plot.bottom()).abs() < 1e-3);
        // Narrower plots: the right margin holds the overlay axis.
        assert_eq!(mag.right(), SIZE.width - OVERLAY_MARGIN_RIGHT);
        assert_eq!(s.x_axis.mapping.to_px(20_000.0), mag.right());
        // γ² 0…1 fills the top 30 % of the magnitude pane, inset from its top border so
        // γ² = 1 does not sit on the border line.
        let o = s.coherence_overlay.as_ref().expect("overlay");
        let top = mag.y + OVERLAY_INSET;
        let bottom = mag.y + mag.h * 0.3;
        assert_eq!(o.band, Rect::new(mag.x, top, mag.w, bottom - top));
        assert_eq!(o.axis.mapping.to_px(1.0), top);
        assert!((o.axis.mapping.to_px(0.0) - bottom).abs() < 1e-3);
        assert_eq!(o.axis.title, "γ²");
        assert_eq!(o.axis.labels(), ["0.0", "0.5", "1.0"]);
        // Axes as before: same ranges and the same steps although both panes grew; the
        // overlay labels sit in the right margin, outside the plot.
        for i in 0..2 {
            assert_eq!(
                s.panes[i].y_axis.mapping.range,
                pane.panes[i].y_axis.mapping.range
            );
            assert_eq!(s.panes[i].y_axis.labels(), pane.panes[i].y_axis.labels());
        }
        assert_eq!(
            s.panes[1].y_axis.labels(),
            ["−180", "−90", "0", "90", "180"]
        );
        assert_eq!(s.panes[0].title, "Magnitude dB");
        let right: Vec<&str> = s.scene.layers[0]
            .labels
            .iter()
            .filter(|l| l.pos[0] > mag.right())
            .map(|l| l.text.as_str())
            .collect();
        assert_eq!(right, ["0.0", "0.5", "1.0", "γ²"]);
        // Polylines: magnitude, overlaid coherence, phase.
        let lines = &s.scene.layers[1].polylines;
        assert_eq!(lines.len(), 3);
        let coh = &lines[1];
        assert!(coh.points.iter().all(|p| (p[1] - top).abs() < 1e-3));
        assert_eq!(coh.clip, Some(mag));
        assert!(coh.alpha.is_empty());
        let a = Theme::dark().trace_color(0).a * OVERLAY_ALPHA;
        assert!((coh.stroke.color.a - a).abs() < 1e-6);
    }

    #[test]
    fn overlay_keeps_alpha_and_blanking() {
        let mut c = cols(60);
        for i in 10..14 {
            c.coh[i] = 0.2;
        }
        c.coh[30] = 0.7;
        let blank = |placement| ViewState {
            tf: crate::view::TfView {
                coherence: crate::view::CoherenceStyle {
                    blank_below: Some(0.5),
                    ..Default::default()
                },
                coherence_placement: placement,
                ..Default::default()
            },
            ..ViewState::default()
        };
        let t = [trace(&c, TraceKey::Live(MeasId(1)), 0.0)];
        let build = |v: &ViewState| {
            transfer_scene(
                &t,
                &DisplayCache::default(),
                &Status::default(),
                v,
                &Theme::dark(),
                SIZE,
            )
            .scene
            .layers[1]
                .polylines
                .clone()
        };
        let pane = build(&blank(CoherencePlacement::Pane));
        let over = build(&blank(CoherencePlacement::OverlayOnMagnitude));
        // Magnitude: same gap and the same per-point opacity in both layouts.
        assert_eq!(segments(&pane[0].points).len(), 2);
        assert_eq!(pane[0].alpha, over[0].alpha);
        assert!(pane[0].alpha.iter().any(|a| *a < 1.0));
        // Coherence is never blanked or faded, in its pane or overlaid.
        for coh in [&pane[2], &over[1]] {
            assert_eq!(segments(&coh.points).len(), 1);
            assert_eq!(coh.points.len(), 60);
            assert!(coh.alpha.is_empty());
        }
        // Phase blanks like magnitude.
        assert_eq!(segments(&over[2].points).len(), 2);
    }

    #[test]
    fn overlay_without_magnitude_keeps_the_pane() {
        let c = cols(20);
        let mut view = overlay_view();
        view.tf.show_magnitude = false;
        let s = transfer_scene(
            &[trace(&c, TraceKey::Live(MeasId(1)), 0.0)],
            &DisplayCache::default(),
            &Status::default(),
            &view,
            &Theme::dark(),
            SIZE,
        );
        let kinds: Vec<_> = s.panes.iter().map(|p| p.kind).collect();
        assert_eq!(kinds, [TfPaneKind::Phase, TfPaneKind::Coherence]);
        assert!(s.coherence_overlay.is_none());
        // Hidden coherence is hidden in both placements.
        let mut view = overlay_view();
        view.tf.show_coherence = false;
        let s = transfer_scene(
            &[trace(&c, TraceKey::Live(MeasId(1)), 0.0)],
            &DisplayCache::default(),
            &Status::default(),
            &view,
            &Theme::dark(),
            SIZE,
        );
        assert!(s.coherence_overlay.is_none());
        assert_eq!(s.scene.layers[1].polylines.len(), 2);
    }

    #[test]
    fn overlay_text_sits_below_the_band() {
        use crate::canvas::tests::{intersects, label_box};
        let s = busy(&overlay_view(), &Status::default());
        let o = s.coherence_overlay.as_ref().expect("overlay");
        // Title, legend, delay line and cursor values all start below the band.
        let texts: Vec<&str> = s
            .legend
            .iter()
            .map(|e| e.text.as_str())
            .chain(s.delay.as_deref())
            .chain(["Magnitude dB", "1.00 kHz"])
            .collect();
        let labels: Vec<&crate::primitives::Label> = s.scene.layers[..3]
            .iter()
            .flat_map(|l| &l.labels)
            .filter(|l| texts.contains(&l.text.as_str()) || l.text.contains("dB  "))
            .collect();
        assert!(labels.len() >= texts.len() + 2, "{labels:?}");
        for l in labels {
            assert!(
                !intersects(label_box(l), o.band),
                "{:?} over the coherence band",
                l.text
            );
        }
        // Pane mode keeps the block at the top of the pane.
        let p = busy(&ViewState::default(), &Status::default());
        let title = |s: &TfScene| {
            s.scene.layers[0]
                .labels
                .iter()
                .find(|l| l.text == "Magnitude dB")
                .map(|l| l.pos[1])
        };
        assert_eq!(title(&p), Some(p.panes[0].plot.y + 4.0));
        assert_eq!(title(&s), Some(o.band.bottom() + 4.0));
    }

    #[test]
    fn narrow_panes_move_cursor_values_below_the_legend() {
        use crate::canvas::tests::{intersects, label_box};
        let a = cols(97);
        let mut ta = trace(&a, TraceKey::Live(MeasId(1)), 0.010);
        ta.name = "Main left hang".into();
        let mut tb = trace(&a, TraceKey::Live(MeasId(2)), 0.0115);
        tb.name = "Delay tower stage right".into();
        let view = ViewState {
            cursor_hz: Some(1000.0),
            ..ViewState::default()
        };
        let build = |w: f32| {
            let size = Viewport {
                width: w,
                height: 600.0,
            };
            transfer_scene(
                &[ta.clone(), tb.clone()],
                &DisplayCache::default(),
                &Status::default(),
                &view,
                &Theme::dark(),
                size,
            )
        };
        let row_y = |s: &TfScene, needle: &str| {
            s.scene.layers[2]
                .labels
                .iter()
                .find(|l| l.text.starts_with(needle))
                .map(|l| l.pos[1])
                .expect(needle)
        };
        // Wide: values share the legend rows, on a plate of their own at the right.
        let wide = build(1400.0);
        assert_eq!(row_y(&wide, "0.0 dB"), row_y(&wide, "Main left hang"));
        let (legend, values) = (
            wide.legend_box.as_ref().expect("legend").rect,
            wide.readout_box.expect("values"),
        );
        assert!(values.x > legend.right());
        // Narrow: the plates would meet, so the values go below the legend (its rows and
        // delay line); nothing overlaps.
        let narrow = build(360.0);
        let legend = narrow.legend_box.as_ref().expect("legend").rect;
        let values = narrow.readout_box.expect("values");
        assert!(values.y >= legend.bottom(), "{values:?} below {legend:?}");
        assert!(row_y(&narrow, "0.0 dB") > row_y(&narrow, "ref Main left hang"));
        let labels: Vec<&crate::primitives::Label> = narrow.scene.layers[2].labels.iter().collect();
        for (i, a) in labels.iter().enumerate() {
            for b in &labels[i + 1..] {
                assert!(
                    !intersects(label_box(a), label_box(b)),
                    "{:?} overlaps {:?}",
                    a.text,
                    b.text
                );
            }
        }
    }

    /// Eighteen curves: the legend stops at 70 % of the magnitude pane and scrolls, its rows on
    /// a translucent plate in the plot's colour, the rest counted; the cursor values follow
    /// the shown rows. The pointer on it makes the plate opaque.
    #[test]
    fn many_curves_scroll_on_a_plate() {
        use crate::legend::LegendHover;
        let a = cols(97);
        let ts: Vec<TfTrace<'_>> = (0..18)
            .map(|i| {
                let mut t = trace(&a, TraceKey::Stored(TraceId(i)), 0.010);
                t.name = format!("xc-xone-tf-cap {i}");
                t
            })
            .collect();
        let theme = Theme::light();
        let mut view = ViewState {
            cursor_hz: Some(1000.0),
            ..ViewState::default()
        };
        let build = |view: &ViewState| {
            transfer_scene(
                &ts,
                &DisplayCache::default(),
                &Status::default(),
                view,
                &theme,
                SIZE,
            )
        };
        let s = build(&view);
        let b = s.legend_box.clone().expect("legend");
        let mag = s.panes[0].plot;
        assert!(b.rect.h <= 0.7 * mag.h, "{:?} in {mag:?}", b.rect);
        assert!(b.shown > 3 && b.shown < 18, "{}", b.shown);
        assert_eq!(b.more, Some(format!("{} below", 18 - b.shown)));
        let overlay = &s.scene.layers[2];
        let plate = overlay
            .rects
            .iter()
            .find(|r| r.rect == b.rect)
            .expect("plate");
        assert_eq!(plate.color, theme.plot_background.with_alpha(0.88));
        let on_plate = |y: f32| y > b.rect.y && y < b.rect.bottom();
        for (text, y) in &b.rows {
            let l = overlay
                .labels
                .iter()
                .find(|l| &l.text == text)
                .expect("row");
            assert!(on_plate(l.pos[1]) && l.pos[1] == *y);
            assert_eq!(l.color, theme.text);
        }
        // The values of the shown rows only, and the frequency.
        let values = s.readout_box.expect("values");
        let n = overlay
            .labels
            .iter()
            .filter(|l| l.pos[1] > values.y && l.pos[1] < values.bottom() && l.pos[0] > values.x)
            .count();
        assert_eq!(n, b.shown + 1);
        // Scrolled to the end, the pointer on it.
        view.tf.legend.first = b.scrolled(100);
        view.tf.legend.hover = Some(LegendHover::Plate);
        let s = build(&view);
        let b = s.legend_box.clone().expect("legend");
        assert_eq!(b.first + b.shown, 18);
        assert_eq!(
            b.rows.last().map(|r| r.0.as_str()),
            Some("xc-xone-tf-cap 17 · Δt 0.00 ms")
        );
        assert_eq!(b.more, Some(format!("{} above", 18 - b.shown)));
        let plate = s.scene.layers[2]
            .rects
            .iter()
            .find(|r| r.rect == b.rect)
            .expect("plate");
        assert_eq!(plate.color, theme.plot_background);
    }

    /// Snapped to the bottom-right corner, the legend sits there and the values move to the
    /// left; hidden, no plate and each value names its trace.
    #[test]
    fn legend_corners_and_hidden() {
        let a = cols(97);
        let ta = trace(&a, TraceKey::Live(MeasId(1)), 0.010);
        let tb = trace(&a, TraceKey::Live(MeasId(2)), 0.0115);
        let mut view = ViewState {
            cursor_hz: Some(1000.0),
            ..ViewState::default()
        };
        view.tf
            .legend
            .snap(crate::legend::LegendCorner::BottomRight);
        let build = |view: &ViewState| {
            transfer_scene(
                &[ta.clone(), tb.clone()],
                &DisplayCache::default(),
                &Status::default(),
                view,
                &Theme::dark(),
                SIZE,
            )
        };
        let s = build(&view);
        let mag = s.panes[0].plot;
        let b = s.legend_box.as_ref().expect("legend");
        assert_eq!(b.rect.right(), mag.right() - crate::legend::INSET);
        assert_eq!(b.rect.bottom(), mag.bottom() - crate::legend::INSET);
        let values = s.readout_box.expect("values");
        assert_eq!(values.x, mag.x + crate::legend::INSET);
        view.tf.legend.hidden = true;
        let s = build(&view);
        assert_eq!(s.legend_box, None);
        let labels: Vec<&str> = s.scene.layers[2]
            .labels
            .iter()
            .map(|l| l.text.as_str())
            .collect();
        assert!(
            labels.iter().any(|l| l.starts_with("m2  0.0 dB  ")),
            "{labels:?}"
        );
        assert!(!labels.iter().any(|l| l.starts_with("m1 · ")), "{labels:?}");
    }
}
