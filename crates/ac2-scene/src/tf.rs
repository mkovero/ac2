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
use crate::view::Style;
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
    /// Where each shown legend row is, for a click to select its curve.
    pub legend_hits: Vec<crate::legend::LegendHit>,
    pub cursor: Option<CursorReadout>,
    /// The cursor values' plate.
    pub readout_box: Option<Rect>,
    /// Min / max tracking's peaks and dips of the tracked curve; `None` while it is off or
    /// the magnitude is not shown.
    pub extremes: Option<TfExtremes>,
    /// Their list as drawn; `None` also while plot chrome is off (T), when only the marks
    /// stay.
    pub extremes_list: Option<String>,
    /// Reference trace's delay: `ref m1 12.34 ms · 4.24 m @ 20 °C`.
    pub delay: Option<String>,
    /// Present when coherence is drawn over the magnitude pane.
    pub coherence_overlay: Option<CoherenceOverlay>,
    /// Banner strip above the panes; zero height when no banner is up.
    pub strip: Rect,
    pub banners: Vec<BannerRow>,
    /// Ranges the marked curve does not resolve at its grid (the selected curve, else the
    /// first of the pane's own), shaded in every pane; empty when the marker is off.
    pub unresolved: Vec<crate::unresolved::RangeMark>,
}

/// The valid-resolution marks of the selected shown curve that knows its ranges, else of
/// the first of the pane's own (not compared) that does: one curve's, so the shading
/// stays one statement.
fn unresolved_marks(
    traces: &[TfTrace<'_>],
    shown: &[DisplayTrace],
    view: &ViewState,
) -> Vec<crate::unresolved::RangeMark> {
    if !view.marks() {
        return Vec::new();
    }
    let known = || {
        traces
            .iter()
            .filter(|t| t.unresolved.is_some() && shown.iter().any(|d| d.key == t.key))
    };
    known()
        .find(|t| t.selected)
        .or_else(|| known().find(|t| !t.compared))
        .and_then(|t| {
            let src = t.unresolved?;
            Some(crate::unresolved::marks(
                src,
                t.freqs.first().copied()?,
                None,
            ))
        })
        .unwrap_or_default()
}

/// `nudge_s`: a stored trace's delay from its arrival (its display nudge); `stepped_s`: a
/// live curve's delay from its measured arrival, already in its columns and not part of
/// `Δ`. Either is tagged the same way: `+0.10 ms from arrival`.
fn legend_entry(
    t: &DisplayTrace,
    nudge_s: f64,
    stepped_s: f64,
    selected: bool,
    compared: bool,
) -> LegendEntry {
    // A compared curve is not of the pane's group: said first, before what it is.
    let mut tags: Vec<String> = compared
        .then(|| crate::trace::COMPARED.to_owned())
        .into_iter()
        .chain(t.note.iter().cloned())
        .collect();
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
    let dash = t.dashed.then_some(Dash {
        on: 6.0,
        off: 4.0,
        offset: 0.0,
    });
    Stroke {
        dash,
        ..Stroke::solid(t.color.with_alpha(a), theme.trace_width * w)
    }
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

/// What the transfer picture keeps right of its plots on its own: room for the overlaid
/// coherence axis when it is drawn.
pub(crate) fn right_margin(view: &ViewState) -> f32 {
    if overlaid(view) {
        OVERLAY_MARGIN_RIGHT
    } else {
        MARGINS.right
    }
}

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
fn overlay_frame(c: &mut Canvas, plot: Rect, o: &CoherenceOverlay, style: Style) {
    let Style { chrome, theme } = style;
    // The band's floor is drawn as a grid line, so it goes with the grid.
    if chrome.grid() {
        overlay_floor(c, plot, o, theme);
    }
    if !chrome.labels() {
        return;
    }
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

fn overlay_floor(c: &mut Canvas, plot: Rect, o: &CoherenceOverlay, theme: &Theme) {
    let floor = Stroke {
        dash: Some(Dash {
            on: 3.0,
            off: 3.0,
            offset: 0.0,
        }),
        ..theme.grid_major
    };
    canvas::hline(c, plot, o.band.bottom(), floor);
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
    let right = crate::freq_column::right(right_margin(view), view);
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
    let marks = unresolved_marks(traces, &shown, view);

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
            Style {
                chrome: view.chrome,
                theme,
            },
        );
        crate::unresolved::draw(&mut c, plot, &xm, &marks, theme);
        let ym = y_axis.mapping;
        if kind != TfPaneKind::Coherence {
            canvas::hline(&mut c, plot, ym.to_px(0.0), theme.zero_line);
        }
        if let Some(o) = &band {
            overlay_frame(
                &mut c,
                plot,
                o,
                Style {
                    chrome: view.chrome,
                    theme,
                },
            );
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
    let nudges: Vec<(f64, f64, bool)> = traces
        .iter()
        .map(|t| (t.nudge.0, t.delay_nudge.0, t.compared))
        .collect();
    let legend: Vec<LegendEntry> = shown
        .iter()
        .zip(&nudges)
        .map(|(t, (n, s, c))| legend_entry(t, *n, *s, selected == Some(t.key), *c))
        .collect();
    let mut cursor = view
        .cursor_hz
        .filter(|_| view.chrome.cursor())
        .and_then(|hz| readout::cursor_readout(&shown, hz, view.tf.phase));
    if let Some(cr) = cursor.as_mut()
        && let Some(m) = crate::unresolved::at(&marks, cr.freq_hz)
    {
        cr.freq = format!("{} · {}", cr.freq, m.note);
    }
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
    let mag = panes.iter().find(|p| p.kind == TfPaneKind::Magnitude);
    let extremes = match mag.filter(|_| view.tf.extremes) {
        Some(p) => extremes::extremes(
            &mut c,
            traces,
            &shown,
            cache,
            p.plot,
            &xm,
            &p.y_axis.mapping,
            view.chrome.peak_list(),
            theme,
        ),
        None => {
            if !view.tf.extremes {
                cache.clear_extremes();
            }
            None
        }
    };
    let extremes_list = extremes
        .as_ref()
        .filter(|e| view.chrome.peak_list() && !(e.max.is_empty() && e.min.is_empty()))
        .zip(mag)
        .map(|(e, p)| {
            let text = extremes::list_text(e, shown.len() > 1);
            let avoid: Vec<Rect> = legend_box
                .iter()
                .map(|b| b.rect)
                .chain(readout_box)
                .collect();
            extremes::draw_list(&mut c, &text, p.plot, &avoid, theme);
            text
        });
    let legend_hits = legend_box.as_ref().map_or_else(Vec::new, |b| {
        let keys: Vec<TraceKey> = legend.iter().map(|e| e.key).collect();
        b.hits(&keys)
    });

    TfScene {
        scene: c.into_scene(size),
        x_axis,
        panes,
        reference,
        traces: shown,
        legend,
        legend_box,
        legend_hits,
        cursor,
        readout_box,
        extremes,
        extremes_list,
        delay,
        coherence_overlay,
        strip: strip.rect,
        banners: strip.rows,
        unresolved: marks,
    }
}

mod extremes;
pub use extremes::{Comb, TfExtremes};

#[cfg(test)]
mod tests;
