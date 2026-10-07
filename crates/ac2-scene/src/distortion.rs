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
use crate::view::{DistortionUnit, ViewState};

/// What a distortion curve says at one column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reading {
    /// Not measured there (outside the band this order can be measured in).
    NotMeasured,
    /// Within the noise: below this floor (dB re fundamental).
    BelowFloor(f64),
    /// Distortion, dB re fundamental.
    Level(f64),
}

/// The reading of `c` at column `i` under the analysis' margin `margin_db`.
pub fn reading(c: &DistortionCurve, i: usize, margin_db: f64) -> Reading {
    let level = c.level_db.get(i).map_or(f64::NAN, |v| f64::from(*v));
    let floor = c.floor_db.get(i).map_or(f64::NAN, |v| f64::from(*v));
    if !level.is_finite() {
        Reading::NotMeasured
    } else if c.valid(i, ac2_proto::units::Db(margin_db)) {
        Reading::Level(level)
    } else {
        Reading::BelowFloor(floor)
    }
}

/// dB re fundamental as percent.
pub fn percent(db: f64) -> f64 {
    100.0 * 10f64.powf(db / 20.0)
}

/// Percent of the fundamental as dB re it (the inverse of [`percent`]); NaN for a value
/// that is not a positive ratio.
pub fn db_of_percent(p: f64) -> f64 {
    if p > 0.0 {
        20.0 * (p / 100.0).log10()
    } else {
        f64::NAN
    }
}

/// `1.00 %`, `0.0316 %`: three significant digits.
pub fn percent_text(db: f64) -> String {
    let p = percent(db);
    if !p.is_finite() || p <= 0.0 {
        return format::NO_VALUE.to_string();
    }
    let decimals = (2 - p.log10().floor() as i32).clamp(0, 6) as usize;
    format!("{} %", format::fixed(p, decimals))
}

/// `−40.1 dB`.
pub fn db_text(db: f64) -> String {
    if db.is_finite() {
        format!("{} dB", format::fixed(db, 1))
    } else {
        format::NO_VALUE.to_string()
    }
}

/// A value in `unit`.
pub fn value_text(db: f64, unit: DistortionUnit) -> String {
    match unit {
        DistortionUnit::Db => db_text(db),
        DistortionUnit::Percent => percent_text(db),
    }
}

/// `−40.1 dB`, `< −72.0 dB` (within the noise), `—` (not measured).
pub fn reading_text(r: Reading, unit: DistortionUnit) -> String {
    match r {
        Reading::NotMeasured => format::NO_VALUE.to_string(),
        Reading::BelowFloor(f) if f.is_finite() => format!("< {}", value_text(f, unit)),
        Reading::BelowFloor(_) => "< floor".to_string(),
        Reading::Level(v) => value_text(v, unit),
    }
}

/// The highest valid point of an order: (frequency, dB re fundamental).
pub fn peak(c: &DistortionCurve, freqs: &[f64], margin_db: f64) -> Option<(f64, f64)> {
    (0..freqs.len())
        .filter_map(|i| match reading(c, i, margin_db) {
            Reading::Level(v) => Some((freqs[i], v)),
            _ => None,
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
}

/// The reading at `hz`: the nearest column within 1/12 octave, else not measured.
pub fn reading_at(c: &DistortionCurve, freqs: &[f64], hz: f64, margin_db: f64) -> Reading {
    match nearest_column(freqs, hz) {
        Some(i) if (freqs[i] / hz).log2().abs() <= 1.0 / 12.0 => reading(c, i, margin_db),
        _ => Reading::NotMeasured,
    }
}

/// Short summary of a sweep: THD at a few frequencies, each order's highest point.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    /// `(frequency, THD reading)`.
    pub thd: Vec<(f64, Reading)>,
    /// `(order, highest valid point (frequency, dB))`.
    pub peaks: Vec<(u8, Option<(f64, f64)>)>,
}

/// THD at 100 Hz, 1 kHz and 10 kHz and every order's highest point.
pub fn summary(s: &SweepData, freqs: &[f64]) -> Summary {
    let m = s.info.floor_margin.0;
    Summary {
        thd: [100.0, 1000.0, 10_000.0]
            .into_iter()
            .map(|f| (f, reading_at(&s.thd, freqs, f, m)))
            .collect(),
        peaks: s
            .harmonics
            .iter()
            .map(|h| (h.order, peak(&h.curve, freqs, m)))
            .collect(),
    }
}

/// `H2`, …, `THD`.
pub fn order_name(order: u8) -> String {
    format!("H{order}")
}

/// One sweep trace to draw.
#[derive(Clone, Copy, Debug)]
pub struct SweepView<'a> {
    pub data: &'a TraceData,
    /// Column frequencies of its grid.
    pub freqs: &'a [f64],
    /// The colour its fundamental is drawn in ([`crate::families`]).
    pub color: Color,
}

/// How a legend entry shows what it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegendMark {
    /// The name in the curve's colour.
    Name,
    /// A dashed sample: an order within the noise, drawn at its floor in its own colour.
    Dashed,
    /// A shaded sample: under the lowest order's floor.
    Shade,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LegendEntry {
    pub name: String,
    pub color: Color,
    pub mark: LegendMark,
}

/// Legend name of the dashed floor lines.
pub const BELOW_FLOOR: &str = "< floor";
/// Legend name of the shading under every order's floor.
pub const NOISE: &str = "noise";

/// Cursor readout of the distortion view.
#[derive(Clone, Debug, PartialEq)]
pub struct DistortionCursor {
    pub freq_hz: f64,
    pub freq: String,
    /// `(name, value)`: the fundamental, every order, THD.
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
    /// The orders (each name in its colour), THD, then the two floor marks.
    pub legend: Vec<LegendEntry>,
    /// `Main L sweep · arrival 3.32 ms · 2 × 3.10 s · window 8 + 87 ms`.
    pub info: String,
    /// The part of [`Self::info`] that fits beside the fundamental's axis title (empty when
    /// nothing does).
    pub caption: String,
    pub cursor: Option<DistortionCursor>,
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
    let arrival = format!("arrival {}", format::ms(i.arrival.0, 2));
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

/// The distortion view of `t`: the fundamental's magnitude above, the distortion below.
pub fn distortion_scene(
    t: Option<SweepView<'_>>,
    status: &Status,
    view: &ViewState,
    theme: &Theme,
    size: Viewport,
) -> DistortionScene {
    let mut c = Canvas::new(size, theme);
    let plot_w = (size.width - MARGINS.left - MARGINS.right).max(1.0);
    let strip = canvas::banner_strip(&mut c, status, MARGINS.left, plot_w, size, theme);
    let area = canvas::plot_area(size, strip.rect.bottom(), MARGINS.right);
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

    // Fundamental: its own range, 40 dB under its maximum.
    let fund_max = sweep.map_or(f64::NAN, |(t, _)| {
        t.data
            .mag_db
            .iter()
            .map(|v| f64::from(*v))
            .filter(|v| v.is_finite())
            .fold(f64::NEG_INFINITY, f64::max)
    });
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
    canvas::pane_frame(&mut c, plot, &x_axis, &y_axis, true, title, theme);
    let ym = y_axis.mapping;

    let mut legend = Vec::new();
    let mut info = String::new();
    let mut caption = String::new();
    let note = match sweep {
        None => Some("no sweep result".to_string()),
        Some(_) => None,
    };
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

    let cursor = view.cursor_hz.zip(sweep).and_then(|(hz, (t, s))| {
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
        Some(DistortionCursor {
            freq_hz: f,
            freq: format::freq_readout(f),
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
        note,
        strip: strip.rect,
        banners: strip.rows,
    }
}

/// The sweep's impulse response as an IR frame for [`crate::ir::ir_scene`]: time zero at
/// the arrival, the harmonics' impulses before it.
pub fn ir_frame(d: &TraceData) -> Option<ac2_proto::frame::IrFrame> {
    let s = d.sweep.as_ref()?;
    Some(ac2_proto::frame::IrFrame {
        meas: ac2_proto::units::MeasId(0),
        meta: ac2_proto::frame::IrMeta {
            sample_rate: s.info.sample_rate,
            t0: s.ir.t0,
            dt: s.ir.dt,
            inserted_delay: s.info.arrival,
        },
        linear: s.ir.linear.clone(),
        etc: Some(s.ir.etc_db.clone()),
    })
}

/// Shortest IR plot left above the room table, logical pixels; a shorter pane shows the
/// IR alone.
const MIN_IR_HEIGHT: f32 = 140.0;

/// The sweep's impulse response in the IR view, with each harmonic's impulse marked at
/// `−L·ln k` and time zero named as the arrival; below it, when the pane is tall enough,
/// the room parameters of its octave bands ([`crate::room`]).
pub fn sweep_ir_scene(
    d: &TraceData,
    color: Color,
    status: &Status,
    view: &ViewState,
    theme: &Theme,
    size: Viewport,
) -> Option<crate::ir::IrScene> {
    let s = d.sweep.as_ref()?;
    let frame = ir_frame(d)?;
    let table = s
        .room
        .as_ref()
        .map(|r| crate::room::room_table(r, crate::room::BandSet::Octave))
        .filter(|t| size.height - crate::room::table_height(t, theme) >= MIN_IR_HEIGHT);
    let ir_size = Viewport {
        height: size.height
            - table
                .as_ref()
                .map_or(0.0, |t| crate::room::table_height(t, theme)),
        ..size
    };
    let mut sc = crate::ir::ir_scene(
        &frame,
        color,
        None,
        status,
        view,
        &view.distortion.ir,
        theme,
        ir_size,
    );
    sc.room = table.clone();
    if let Some(t) = &table {
        let rect = Rect::new(
            MARGINS.left,
            ir_size.height,
            (size.width - MARGINS.left - MARGINS.right).max(1.0),
            size.height - ir_size.height,
        );
        sc.scene.viewport = size;
        sc.scene
            .layers
            .push(crate::room::table_layer(t, rect, theme));
    }
    let origin = format!(
        "t = 0 at the arrival {}",
        crate::readout::delay_readout(s.info.arrival.0, view.temperature_c)
    );
    for layer in &mut sc.scene.layers {
        for l in &mut layer.labels {
            if l.text == sc.origin {
                l.text.clone_from(&origin);
            }
        }
    }
    sc.origin = origin;
    let xm = sc.x_axis.mapping;
    let plot = sc.plot;
    if let Some(overlay) = sc.scene.layers.get_mut(2) {
        for (t_ms, name) in harmonic_marks(s) {
            let x = xm.to_px(t_ms);
            if !(x >= plot.x && x <= plot.right()) {
                continue;
            }
            overlay.polylines.push(Polyline {
                points: vec![[x, plot.y], [x, plot.bottom() - 16.0]],
                alpha: vec![],
                stroke: Stroke {
                    color: theme.text_dim,
                    width: 1.0,
                    dash: Some(crate::primitives::Dash {
                        on: 3.0,
                        off: 3.0,
                        offset: 0.0,
                    }),
                },
                clip: Some(plot),
            });
            overlay.labels.push(label(
                name,
                [x + 3.0, plot.bottom() - 4.0],
                anchor(HAlign::Left, VAlign::Bottom),
                theme.small_font_size,
                theme.text_dim,
            ));
        }
    }
    Some(sc)
}

/// Times (ms re the arrival) and names of the harmonic impulses: `(−L·ln k, "H k")`.
pub fn harmonic_marks(s: &SweepData) -> Vec<(f64, String)> {
    s.harmonics
        .iter()
        .map(|h| {
            (
                -s.info.rate.0 * f64::from(h.order).ln() * 1000.0,
                order_name(h.order),
            )
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests;
