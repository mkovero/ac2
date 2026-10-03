//! Sweep results: the fundamental's response over the harmonic distortion of each order and
//! the total (THD), in dB re the fundamental or in percent, with the noise floor shaded
//! (`docs/design/sweep-distortion.md`); the readouts the CLI prints come from here too.
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
    Band, BandPoint, Color, HAlign, Polyline, Rect, Scene, Stroke, VAlign, Viewport,
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
}

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
    /// `(name, colour)` of each drawn curve, in drawing order (orders, then THD, then the
    /// floor).
    pub legend: Vec<(String, Color)>,
    /// `Main L sweep · arrival 3.32 ms · 2 × 3.10 s · window 8 + 87 ms`.
    pub info: String,
    pub cursor: Option<DistortionCursor>,
    /// Why nothing is drawn, when that is the case.
    pub note: Option<String>,
    pub strip: Rect,
    pub banners: Vec<BannerRow>,
}

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

fn info_line(d: &TraceData, s: &SweepData) -> String {
    let i = &s.info;
    let mut t = format!(
        "{} · arrival {} · {} × {} s · window {} + {}",
        d.meta.edit.name,
        format::ms(i.arrival.0, 2),
        i.repeats,
        format::fixed(i.duration.0, 2),
        format::ms(i.window_pre.0, 0),
        format::ms(i.window_post.0, 0),
    );
    if i.clipped {
        t.push_str(" · CLIPPED");
    }
    t
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
        "Fundamental dB",
        theme,
    );

    let margin = sweep.map_or(6.0, |(_, s)| s.info.floor_margin.0);
    let y_range = match unit {
        DistortionUnit::Db => view.distortion.range_db,
        DistortionUnit::Percent => {
            // Up to the largest valid value (at least 1 %), from zero.
            let top = sweep.map_or(1.0, |(t, s)| {
                s.harmonics
                    .iter()
                    .map(|h| &h.curve)
                    .chain(std::iter::once(&s.thd))
                    .flat_map(|c| {
                        (0..t.freqs.len()).filter_map(move |i| match reading(c, i, margin) {
                            Reading::Level(v) => Some(percent(v)),
                            _ => None,
                        })
                    })
                    .fold(1.0f64, f64::max)
            });
            Range::new(0.0, top * 1.1)
        }
    };
    let title = match unit {
        DistortionUnit::Db => "dB re fundamental",
        DistortionUnit::Percent => "% of fundamental",
    };
    let y_unit = match unit {
        DistortionUnit::Db => "dB",
        DistortionUnit::Percent => "%",
    };
    let y_axis = axis::linear_axis(y_range, plot.bottom(), plot.y, y_unit);
    canvas::pane_frame(&mut c, plot, &x_axis, &y_axis, true, title, theme);
    let ym = y_axis.mapping;

    let mut legend = Vec::new();
    let mut info = String::new();
    let note = match sweep {
        None => Some("no sweep result".to_string()),
        Some(_) => None,
    };
    if let Some((t, s)) = sweep {
        info = info_line(t.data, s);
        let cols = visible_columns(t.freqs, xm.range.lo, xm.range.hi);
        let xs: Vec<f32> = t.freqs[cols.clone()].iter().map(|f| xm.to_px(*f)).collect();

        // The noise floor of the lowest order, shaded from the bottom of the plot up: any
        // curve under it is within the noise.
        if let Some(h) = s.harmonics.first() {
            let floor_color = theme.text_dim.with_alpha(0.25);
            let points: Vec<BandPoint> = cols
                .clone()
                .zip(&xs)
                .map(|(i, x)| {
                    let f = h.curve.floor_db.get(i).map_or(f64::NAN, |v| f64::from(*v));
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
                color: floor_color,
                clip: Some(plot),
            });
            legend.push((
                format!("noise floor ({})", order_name(h.order)),
                theme.text_dim,
            ));
        }
        let curves: Vec<(String, u8, &DistortionCurve)> = s
            .harmonics
            .iter()
            .map(|h| (order_name(h.order), h.order, &h.curve))
            .chain(std::iter::once(("THD".to_string(), 0u8, &s.thd)))
            .collect();
        for (name, order, curve) in &curves {
            let color = order_color(theme, *order);
            let ys: Vec<f32> = cols
                .clone()
                .map(|i| match reading(curve, i, margin) {
                    Reading::Level(v) => ym.to_px(y_value(v, unit)),
                    _ => f32::NAN,
                })
                .collect();
            let (points, _) = gapped(&xs, &ys, None, |_, _| false);
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
            legend.push((name.clone(), color));
        }
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
        let c0 = t.data.meta.edit.color;
        if !points.is_empty() {
            c.data.polylines.push(Polyline {
                points,
                alpha: vec![],
                stroke: Stroke::solid(
                    Color::from_rgba8([c0.r, c0.g, c0.b, 255]),
                    theme.trace_width,
                ),
                clip: Some(fundamental),
            });
        }
    }

    // Legend: one coloured name per curve, along the top of the distortion plot.
    let mut x = plot.x + 6.0 + canvas::text_width(title, theme.small_font_size) + 18.0;
    for (name, color) in &legend {
        c.overlay.labels.push(label(
            name.clone(),
            [x, plot.y + 4.0],
            anchor(HAlign::Left, VAlign::Top),
            theme.small_font_size,
            color.with_alpha(1.0),
        ));
        x += canvas::text_width(name, theme.small_font_size) + 12.0;
    }
    if !info.is_empty() {
        c.overlay.labels.push(label(
            info.clone(),
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
            [plot.right() - 8.0, plot.y + 22.0],
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

/// The sweep's impulse response in the IR view, with each harmonic's impulse marked at
/// `−L·ln k` and time zero named as the arrival.
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
    let mut sc = crate::ir::ir_scene(&frame, color, None, status, view, theme, size);
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
mod tests;
