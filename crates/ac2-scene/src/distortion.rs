//! Sweep results: the fundamental's response over the harmonic distortion of each order and
//! the total (THD), in dB re the fundamental or in percent, each order drawn at its own noise
//! floor where it is within the noise (`docs/design/sweep-distortion.md`, "Display"); the
//! readouts the CLI prints come from here too.
//!
//! A distortion value is shown only where the daemon's analysis says it is one: measured and
//! at least `info.floor_margin` above the noise in its window ([`DistortionCurve::valid`]).
//! Anywhere else it is "< floor" (with the floor's value), never a number that is really
//! noise. Harmonic `k` measured at `k·f` is drawn at the fundamental `f`.

use ac2_proto::model::{DistortionCurve, SweepData, TraceData};

use crate::axis::{self, Axis, Range};
use crate::banner::{BannerRow, Status};
use crate::canvas::{self, Canvas, MARGINS, PANE_GAP, anchor, gapped, label, visible_columns};
use crate::format;
use crate::grid::nearest_column;
use crate::primitives::{
    Band, BandPoint, Color, Dash, FillRect, HAlign, Polyline, Rect, Scene, Stroke, VAlign, Viewport,
};
use crate::theme::Theme;
use crate::view::Style;
use crate::view::{DistortionUnit, ViewState};

mod reading;
mod sweep_ir;
pub use reading::*;
pub use sweep_ir::{harmonic_marks, ir_frame, sweep_ir_scene};

/// Cursor readout of the distortion view.
#[derive(Clone, Debug, PartialEq)]
pub struct DistortionCursor {
    pub freq_hz: f64,
    pub freq: String,
    /// `(name, value)`: the fundamental, every order, THD, then the reference's own orders
    /// when drawn.
    pub rows: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DistortionScene {
    pub scene: Scene,
    /// Fundamental response.
    pub fundamental: Rect,
    /// Distortion.
    pub plot: Rect,
    pub x_axis: Axis,
    pub y_fundamental: Axis,
    pub y_axis: Axis,
    /// The orders (each name in its colour), THD, the reference's dotted mark when its
    /// orders are drawn, then the two floor marks.
    pub legend: Vec<LegendEntry>,
    /// `Main L sweep · arrival 3.32 ms · 2 × 3.10 s · window 8 + 87 ms`.
    pub info: String,
    /// The part of [`Self::info`] that fits beside the fundamental's axis title (empty when
    /// nothing does).
    pub caption: String,
    pub cursor: Option<DistortionCursor>,
    /// Where an order reads within [`REFERENCE_BOUND_DB`] of the reference's own
    /// ([`near_reference_text`]), one line each, along the bottom of the distortion plot;
    /// shown whether or not the reference's lines are.
    pub near_reference: Vec<String>,
    /// Ranges the sweep's response does not resolve at its grid (shaded in the fundamental's
    /// pane) and those its harmonics do not (shaded in the distortion pane); empty when the
    /// marker is off or the trace does not say.
    pub unresolved_response: Vec<crate::unresolved::Mark>,
    pub unresolved_harmonics: Vec<crate::unresolved::Mark>,
    /// Why nothing is drawn, when that is the case.
    pub note: Option<String>,
    pub strip: Rect,
    pub banners: Vec<BannerRow>,
}

/// Axis title of the fundamental's response.
const FUNDAMENTAL_TITLE: &str = "Fundamental dB";

/// Colour of harmonic order `k` (and THD as order 0).
pub fn order_color(theme: &Theme, order: u8) -> Color {
    if order == 0 {
        theme.text
    } else {
        theme.trace_color(usize::from(order))
    }
}

/// y value drawn for a level in dB re fundamental.
fn y_value(db: f64, unit: DistortionUnit) -> f64 {
    match unit {
        DistortionUnit::Db => db,
        DistortionUnit::Percent => percent(db),
    }
}

/// The sweep's caption, longest first: then without the name, without the repeats, the
/// arrival alone. CLIPPED stays on every one (and alone last): it says the result is wrong.
fn info_lines(d: &TraceData, s: &SweepData) -> Vec<String> {
    let i = &s.info;
    let arrival = format!("arrival {}", format::delay(i.arrival.0));
    let runs = format!("{} × {} s", i.repeats, format::fixed(i.duration.0, 2));
    let window = format!(
        "window {} + {}",
        format::ms(i.window_pre.0, 0),
        format::ms(i.window_post.0, 0)
    );
    let clipped = if i.clipped { " · CLIPPED" } else { "" };
    let mut out: Vec<String> = [
        format!("{} · {arrival} · {runs} · {window}", d.meta.edit.name),
        format!("{arrival} · {runs} · {window}"),
        format!("{arrival} · {window}"),
        arrival,
    ]
    .into_iter()
    .map(|t| format!("{t}{clipped}"))
    .collect();
    if i.clipped {
        out.push("CLIPPED".to_string());
    }
    out
}

/// The first of `lines` no wider than `width` at `size`, else nothing.
fn fitting(lines: &[String], width: f32, size: f32) -> String {
    lines
        .iter()
        .find(|t| canvas::text_width(t, size) <= width)
        .cloned()
        .unwrap_or_default()
}

/// Height of one legend row.
fn legend_row_h(theme: &Theme) -> f32 {
    theme.small_font_size * 1.25 + 2.0
}

/// Width of a legend entry's sample (before its name).
const SAMPLE_W: f32 = 14.0;

/// Draws `legend` in rows: the first from `x0` (beside the axis title), the next ones from
/// `left`, wrapping before `right`. Returns the bottom of the last row.
fn draw_legend(
    c: &mut Canvas,
    legend: &[LegendEntry],
    x0: f32,
    left: f32,
    right: f32,
    top: f32,
    theme: &Theme,
) -> f32 {
    let size = theme.small_font_size;
    let row_h = legend_row_h(theme);
    let (mut x, mut y) = (x0, top);
    for e in legend {
        let sample = if e.mark == LegendMark::Name {
            0.0
        } else {
            SAMPLE_W + 4.0
        };
        let w = sample + canvas::text_width(&e.name, size);
        if x + w > right && x > left {
            x = left;
            y += row_h;
        }
        let mid = y + size * 0.62;
        match e.mark {
            LegendMark::Name => {}
            LegendMark::Dashed => c.overlay.polylines.push(Polyline {
                points: vec![[x, mid], [x + SAMPLE_W, mid]],
                alpha: vec![],
                stroke: floor_stroke(e.color, theme),
                clip: None,
            }),
            LegendMark::Dotted => c.overlay.polylines.push(Polyline {
                points: vec![[x, mid], [x + SAMPLE_W, mid]],
                alpha: vec![],
                stroke: reference_stroke(e.color, theme),
                clip: None,
            }),
            LegendMark::Shade => c.overlay.rects.push(FillRect {
                rect: Rect::new(x, mid - 4.0, SAMPLE_W, 8.0),
                color: noise_color(theme),
                clip: None,
            }),
        }
        c.overlay.labels.push(label(
            e.name.clone(),
            [x + sample, y],
            anchor(HAlign::Left, VAlign::Top),
            size,
            e.color.with_alpha(1.0),
        ));
        x += w + 12.0;
    }
    y + row_h
}

/// An order within the noise, drawn at its floor: thin, dashed, its colour faded.
fn floor_stroke(color: Color, theme: &Theme) -> Stroke {
    Stroke {
        color: color.with_alpha(0.6),
        width: theme.trace_width * 0.75,
        dash: Some(Dash {
            on: 4.0,
            off: 3.0,
            offset: 0.0,
        }),
    }
}

/// The reference input's own order: dotted and fainter than a floor line, so it reads as
/// context behind the measurement rather than as one.
fn reference_stroke(color: Color, theme: &Theme) -> Stroke {
    Stroke {
        color: color.with_alpha(0.45),
        width: theme.trace_width * 0.75,
        dash: Some(Dash {
            on: 1.5,
            off: 3.0,
            offset: 0.0,
        }),
    }
}

/// Shading under the lowest order's floor.
fn noise_color(theme: &Theme) -> Color {
    theme.text_dim.with_alpha(0.25)
}

/// Polyline through the runs of finite `ys`, broken between runs. A run of one column is
/// drawn across that column's cell (half-way to each neighbour): the value stands for the
/// band around the column, and a lone point would otherwise be a dot in a small pane.
fn runs(xs: &[f32], ys: &[f32]) -> Vec<[f32; 2]> {
    let n = xs.len().min(ys.len());
    let ok = |i: usize| xs[i].is_finite() && ys[i].is_finite();
    let mut out: Vec<[f32; 2]> = Vec::new();
    let mut i = 0;
    while i < n {
        if !ok(i) {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && ok(i) {
            i += 1;
        }
        if !out.is_empty() {
            out.push([f32::NAN, f32::NAN]);
        }
        if i - start == 1 {
            let x = xs[start];
            let left = (start > 0).then(|| (x - xs[start - 1]) / 2.0);
            let right = (start + 1 < xs.len()).then(|| (xs[start + 1] - x) / 2.0);
            let (l, r) = match (left, right) {
                (Some(l), Some(r)) => (l, r),
                (Some(l), None) => (l, l),
                (None, Some(r)) => (r, r),
                (None, None) => (1.0, 1.0),
            };
            let (l, r) = (
                if l.is_finite() { l } else { 1.0 },
                if r.is_finite() { r } else { 1.0 },
            );
            out.push([x - l, ys[start]]);
            out.push([x + r, ys[start]]);
        } else {
            out.extend((start..i).map(|k| [xs[k], ys[k]]));
        }
    }
    out
}

/// A compared run's legend name: `Run 2 · cmp`.
pub fn compared_name(name: &str) -> String {
    format!("{name} · {}", crate::trace::COMPARED)
}

/// A compared run (C) over the pane's own: its fundamental in the upper pane, and its
/// orders and THD where they are valid, all in the run's own colour — the order colours
/// belong to the pane's run, and a compared run's floors would be clutter read as levels.
#[allow(clippy::too_many_arguments)]
fn draw_compared(
    c: &mut Canvas,
    v: &SweepView<'_>,
    s: &SweepData,
    plot: Rect,
    fundamental: Rect,
    xm: &axis::Mapping,
    ym: &axis::Mapping,
    fm: &axis::Mapping,
    unit: DistortionUnit,
    theme: &Theme,
) {
    let margin = s.info.floor_margin.0;
    let cols = visible_columns(v.freqs, xm.range.lo, xm.range.hi);
    let xs: Vec<f32> = v.freqs[cols.clone()].iter().map(|f| xm.to_px(*f)).collect();
    let curves = s
        .harmonics
        .iter()
        .map(|h| (h.order, &h.curve))
        .chain(std::iter::once((0u8, &s.thd)));
    for (order, curve) in curves {
        let ys: Vec<f32> = cols
            .clone()
            .map(|i| match reading(curve, i, margin) {
                Reading::Level(l) => ym.to_px(y_value(l, unit)),
                _ => f32::NAN,
            })
            .collect();
        let points = runs(&xs, &ys);
        if !points.is_empty() {
            let width = if order == 0 { 1.2 } else { 0.8 } * theme.trace_width;
            c.data.polylines.push(Polyline {
                points,
                alpha: vec![],
                stroke: Stroke::solid(v.color.with_alpha(0.8), width),
                clip: Some(plot),
            });
        }
    }
    let ys: Vec<f32> = cols
        .map(|i| {
            v.data
                .mag_db
                .get(i)
                .map_or(f32::NAN, |m| fm.to_px(f64::from(*m)))
        })
        .collect();
    let (points, _) = gapped(&xs, &ys, None, |_, _| false);
    if !points.is_empty() {
        c.data.polylines.push(Polyline {
            points,
            alpha: vec![],
            stroke: Stroke::solid(v.color, theme.trace_width),
            clip: Some(fundamental),
        });
    }
}

/// The distortion view of `t`: the fundamental's magnitude above, the distortion below.
/// `compared` runs (C) are drawn over it in their own colours, tagged `cmp` in the legend.
pub fn distortion_scene(
    t: Option<SweepView<'_>>,
    compared: &[SweepView<'_>],
    status: &Status,
    view: &ViewState,
    theme: &Theme,
    size: Viewport,
) -> DistortionScene {
    let mut c = Canvas::new(size, theme);
    let right = crate::freq_column::right(MARGINS.right, view);
    let plot_w = (size.width - MARGINS.left - right).max(1.0);
    let strip = canvas::banner_strip(&mut c, status, MARGINS.left, plot_w, size, theme);
    let area = canvas::plot_area(size, strip.rect.bottom(), right);
    // Fundamental : distortion heights 1 : 2.
    let top_h = ((area.h - PANE_GAP) / 3.0).max(1.0);
    let fundamental = Rect::new(area.x, area.y, area.w, top_h);
    let plot = Rect::new(
        area.x,
        area.y + top_h + PANE_GAP,
        area.w,
        (area.h - top_h - PANE_GAP).max(1.0),
    );
    let unit = view.distortion.unit;
    let x_axis = axis::freq_axis(view.freq.range(), plot.x, plot.right());
    let xm = x_axis.mapping;
    let sweep = t.and_then(|t| t.data.sweep.as_ref().map(|s| (t, s)));
    let (unresolved_response, unresolved_harmonics) = unresolved_marks(sweep, view);

    let compared: Vec<(&SweepView<'_>, &SweepData)> = compared
        .iter()
        .filter_map(|v| Some((v, v.data.sweep.as_ref()?)))
        .collect();
    // Fundamental: its own range, 40 dB under its maximum (the pane's run's, else the
    // highest compared one's).
    let fund_max = sweep
        .map(|(t, _)| t)
        .into_iter()
        .chain(compared.iter().map(|(v, _)| **v))
        .flat_map(|t| t.data.mag_db.iter().map(|v| f64::from(*v)))
        .filter(|v| v.is_finite())
        .fold(f64::NAN, f64::max);
    let fund_range = if fund_max.is_finite() {
        let hi = (fund_max / 5.0).ceil() * 5.0 + 5.0;
        Range::new(hi - 45.0, hi)
    } else {
        Range::new(-40.0, 5.0)
    };
    let y_fundamental = axis::linear_axis(fund_range, fundamental.bottom(), fundamental.y, "dB");
    let fund_x = axis::freq_axis(view.freq.range(), fundamental.x, fundamental.right());
    canvas::pane_frame(
        &mut c,
        fundamental,
        &fund_x,
        &y_fundamental,
        false,
        FUNDAMENTAL_TITLE,
        Style {
            chrome: view.chrome,
            theme,
        },
    );
    crate::unresolved::draw(
        &mut c,
        fundamental,
        &fund_x.mapping,
        &unresolved_response,
        theme,
    );

    let margin = sweep.map_or(6.0, |(_, s)| s.info.floor_margin.0);
    // Percent on a log axis over the same ratios as the dB view: equal ratios keep equal
    // distances, so 0.01 % and 10 % are both readable and the picture does not change shape
    // when the unit does.
    let title = match unit {
        DistortionUnit::Db => "dB re fundamental",
        DistortionUnit::Percent => "% of fundamental",
    };
    let range_db = view.distortion.range_db;
    let y_axis = match unit {
        DistortionUnit::Db => axis::linear_axis(range_db, plot.bottom(), plot.y, "dB"),
        DistortionUnit::Percent => axis::percent_axis(
            Range::new(percent(range_db.lo), percent(range_db.hi)),
            plot.bottom(),
            plot.y,
            "%",
        ),
    };
    canvas::pane_frame(
        &mut c,
        plot,
        &x_axis,
        &y_axis,
        true,
        title,
        Style {
            chrome: view.chrome,
            theme,
        },
    );
    crate::unresolved::draw(&mut c, plot, &xm, &unresolved_harmonics, theme);
    let ym = y_axis.mapping;

    let mut legend = Vec::new();
    let mut info = String::new();
    let mut caption = String::new();
    let near_reference: Vec<String> = sweep
        .map(|(t, s)| summary(s, t.freqs).near_reference)
        .unwrap_or_default()
        .iter()
        .map(near_reference_text)
        .collect();
    // The reference input's own orders, those the measurement has, when shown.
    let reference: Vec<&ac2_proto::model::HarmonicCurve> = sweep
        .filter(|_| view.distortion.reference)
        .and_then(|(_, s)| {
            let r = s.reference_harmonics.as_deref()?;
            Some(
                r.iter()
                    .filter(|r| s.harmonics.iter().any(|h| h.order == r.order))
                    .collect(),
            )
        })
        .unwrap_or_default();
    let note = match sweep {
        None => Some("no sweep result".to_string()),
        Some(_) => None,
    };
    // Under the pane's own run, so that one stays on top.
    for (v, s) in &compared {
        draw_compared(
            &mut c,
            v,
            s,
            plot,
            fundamental,
            &xm,
            &ym,
            &y_fundamental.mapping,
            unit,
            theme,
        );
    }
    if let Some((t, s)) = sweep {
        let lines = info_lines(t.data, s);
        info = lines[0].clone();
        // Right of the axis title, with a gap; never over it.
        let room = fundamental.w
            - 12.0
            - canvas::text_width(FUNDAMENTAL_TITLE, theme.small_font_size)
            - 18.0;
        caption = fitting(&lines, room, theme.small_font_size);
        let cols = visible_columns(t.freqs, xm.range.lo, xm.range.hi);
        let xs: Vec<f32> = t.freqs[cols.clone()].iter().map(|f| xm.to_px(*f)).collect();

        // Under the lowest order's floor every order is within the noise: shaded from the
        // bottom of the plot up.
        let points: Vec<BandPoint> = cols
            .clone()
            .zip(&xs)
            .map(|(i, x)| {
                let f = s
                    .harmonics
                    .iter()
                    .filter_map(|h| h.curve.floor_db.get(i).map(|v| f64::from(*v)))
                    .filter(|v| v.is_finite())
                    .fold(f64::NAN, f64::min);
                let y = ym.to_px(y_value(f, unit)).clamp(plot.y, plot.bottom());
                BandPoint {
                    x: *x,
                    y0: if f.is_finite() {
                        plot.bottom()
                    } else {
                        f32::NAN
                    },
                    y1: y,
                }
            })
            .collect();
        c.data.bands.push(Band {
            points,
            color: noise_color(theme),
            clip: Some(plot),
        });
        // Behind the measurement's own: only where the reference's order is valid (its
        // floor would be noise drawn as context).
        for r in &reference {
            let ys: Vec<f32> = cols
                .clone()
                .map(|i| match reading(&r.curve, i, margin) {
                    Reading::Level(v) => ym.to_px(y_value(v, unit)),
                    _ => f32::NAN,
                })
                .collect();
            let points = runs(&xs, &ys);
            if !points.is_empty() {
                c.data.polylines.push(Polyline {
                    points,
                    alpha: vec![],
                    stroke: reference_stroke(order_color(theme, r.order), theme),
                    clip: Some(plot),
                });
            }
        }
        let curves: Vec<(String, u8, &DistortionCurve)> = s
            .harmonics
            .iter()
            .map(|h| (order_name(h.order), h.order, &h.curve))
            .chain(std::iter::once(("THD".to_string(), 0u8, &s.thd)))
            .collect();
        for (name, order, curve) in &curves {
            let color = order_color(theme, *order);
            let readings: Vec<Reading> = cols.clone().map(|i| reading(curve, i, margin)).collect();
            // Where the order is within the noise, its floor (what the readout says it is
            // under), dashed; where it is valid, its level, solid.
            let floor_ys: Vec<f32> = readings
                .iter()
                .map(|r| match r {
                    Reading::BelowFloor(f) => ym.to_px(y_value(*f, unit)),
                    _ => f32::NAN,
                })
                .collect();
            let points = runs(&xs, &floor_ys);
            if !points.is_empty() {
                c.data.polylines.push(Polyline {
                    points,
                    alpha: vec![],
                    stroke: floor_stroke(color, theme),
                    clip: Some(plot),
                });
            }
            let ys: Vec<f32> = readings
                .iter()
                .map(|r| match r {
                    Reading::Level(v) => ym.to_px(y_value(*v, unit)),
                    _ => f32::NAN,
                })
                .collect();
            let points = runs(&xs, &ys);
            if !points.is_empty() {
                c.data.polylines.push(Polyline {
                    points,
                    alpha: vec![],
                    stroke: Stroke::solid(
                        color,
                        if *order == 0 {
                            theme.trace_width * 1.4
                        } else {
                            theme.trace_width
                        },
                    ),
                    clip: Some(plot),
                });
            }
            legend.push(LegendEntry {
                name: name.clone(),
                color,
                mark: LegendMark::Name,
            });
        }
        legend.extend(compared.iter().map(|(v, _)| LegendEntry {
            name: compared_name(&v.data.meta.edit.name),
            color: v.color,
            mark: LegendMark::Name,
        }));
        if !reference.is_empty() {
            legend.push(LegendEntry {
                name: REFERENCE.to_string(),
                color: theme.text_dim,
                mark: LegendMark::Dotted,
            });
        }
        legend.push(LegendEntry {
            name: BELOW_FLOOR.to_string(),
            color: theme.text_dim,
            mark: LegendMark::Dashed,
        });
        legend.push(LegendEntry {
            name: NOISE.to_string(),
            color: theme.text_dim,
            mark: LegendMark::Shade,
        });
        // Fundamental magnitude.
        let fm = y_fundamental.mapping;
        let ys: Vec<f32> = cols
            .clone()
            .map(|i| {
                t.data
                    .mag_db
                    .get(i)
                    .map_or(f32::NAN, |v| fm.to_px(f64::from(*v)))
            })
            .collect();
        let (points, _) = gapped(&xs, &ys, None, |_, _| false);
        if !points.is_empty() {
            c.data.polylines.push(Polyline {
                points,
                alpha: vec![],
                stroke: Stroke::solid(t.color, theme.trace_width),
                clip: Some(fundamental),
            });
        }
    }

    if sweep.is_none() {
        legend.extend(compared.iter().map(|(v, _)| LegendEntry {
            name: compared_name(&v.data.meta.edit.name),
            color: v.color,
            mark: LegendMark::Name,
        }));
    }
    // Legend along the top of the distortion plot, beside its axis title, wrapping under it
    // in a narrow pane.
    let legend_bottom = draw_legend(
        &mut c,
        &legend,
        plot.x + 6.0 + canvas::text_width(title, theme.small_font_size) + 18.0,
        plot.x + 6.0,
        plot.right() - 6.0,
        plot.y + 4.0,
        theme,
    );
    if !caption.is_empty() {
        c.overlay.labels.push(label(
            caption.clone(),
            [fundamental.right() - 6.0, fundamental.y + 4.0],
            anchor(HAlign::Right, VAlign::Top),
            theme.small_font_size,
            theme.text_dim,
        ));
    }

    let cursor_hz = view.cursor_hz.filter(|_| view.chrome.cursor());
    let cursor = cursor_hz.zip(sweep).and_then(|(hz, (t, s))| {
        let i = nearest_column(t.freqs, hz)?;
        let f = t.freqs[i];
        let mut rows = vec![(
            "fund".to_string(),
            format::db_readout(t.data.mag_db.get(i).map_or(f64::NAN, |v| f64::from(*v))),
        )];
        for h in &s.harmonics {
            rows.push((
                order_name(h.order),
                reading_text(reading(&h.curve, i, margin), unit),
            ));
        }
        rows.push((
            "THD".to_string(),
            reading_text(reading(&s.thd, i, margin), unit),
        ));
        for r in &reference {
            rows.push((
                reference_order_name(r.order),
                reading_text(reading(&r.curve, i, margin), unit),
            ));
        }
        // The harmonics' note first: the plate reads mostly harmonics.
        let freq = match crate::unresolved::at(&unresolved_harmonics, f)
            .or_else(|| crate::unresolved::at(&unresolved_response, f))
        {
            Some(m) => format!("{} · {}", format::freq_readout(f), m.note),
            None => format::freq_readout(f),
        };
        Some(DistortionCursor {
            freq_hz: f,
            freq,
            rows,
        })
    });
    if let Some(cur) = &cursor {
        let x = xm.to_px(cur.freq_hz);
        canvas::vline(&mut c.overlay, plot, x, theme.cursor);
        canvas::vline(&mut c.overlay, fundamental, x, theme.cursor);
        let mut lines = vec![cur.freq.clone()];
        lines.extend(cur.rows.iter().map(|(n, v)| format!("{n}  {v}")));
        c.overlay.labels.push(label(
            lines.join("\n"),
            [plot.right() - 8.0, legend_bottom + 4.0],
            anchor(HAlign::Right, VAlign::Top),
            theme.small_font_size,
            theme.text,
        ));
    }
    // Bottom up from the plot's lower edge, clear of the frequency labels under it.
    let row_h = legend_row_h(theme);
    for (k, line) in near_reference.iter().rev().enumerate() {
        c.overlay.labels.push(label(
            line.clone(),
            [plot.x + 6.0, plot.bottom() - 4.0 - row_h * k as f32],
            anchor(HAlign::Left, VAlign::Bottom),
            theme.small_font_size,
            theme.text_dim,
        ));
    }
    if let Some(n) = &note {
        c.overlay.labels.push(label(
            n.clone(),
            [plot.x + plot.w / 2.0, plot.y + plot.h / 2.0],
            anchor(HAlign::Center, VAlign::Center),
            theme.font_size,
            theme.text_dim,
        ));
    }
    DistortionScene {
        scene: c.into_scene(size),
        fundamental,
        plot,
        x_axis,
        y_fundamental,
        y_axis,
        legend,
        info,
        caption,
        cursor,
        near_reference,
        unresolved_response,
        unresolved_harmonics,
        note,
        strip: strip.rect,
        banners: strip.rows,
    }
}

/// The valid-resolution marks of a sweep's response and of its harmonics.
fn unresolved_marks(
    sweep: Option<(SweepView<'_>, &SweepData)>,
    view: &ViewState,
) -> (Vec<crate::unresolved::Mark>, Vec<crate::unresolved::Mark>) {
    use crate::unresolved::{Fill, Source, marks};
    let Some((t, Some(u))) = sweep.map(|(t, s)| (t, s.unresolved.as_ref())) else {
        return (Vec::new(), Vec::new());
    };
    let Some(&first) = t.freqs.first() else {
        return (Vec::new(), Vec::new());
    };
    if !view.marks() {
        return (Vec::new(), Vec::new());
    }
    let src = |unresolved| Source {
        unresolved,
        fill: Fill::Interpolated,
    };
    (
        marks(src(&u.response), first, None),
        marks(src(&u.harmonics), first, Some("harmonics")),
    )
}

#[cfg(test)]
pub(crate) mod tests;
