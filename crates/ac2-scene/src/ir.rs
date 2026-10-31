//! Impulse-response pane: linear, log (|h| in dB re peak) or ETC, on a time axis whose
//! origin is the inserted delay.
//!
//! The IR frame's point `i` is at `t0 + i·dt` seconds relative to the inserted delay, so
//! t = 0 is where the delay finder put the arrival; the absolute time is `t + inserted`.
//! The log and ETC views are normalised to their own peak (0 dB = strongest point), which
//! is display scaling, not a level claim. A stored transfer trace captured with an IR
//! ([`stored_ir_scene`]) is drawn the same way, time zero at its delay.

use ac2_proto::frame::IrFrame;
use ac2_proto::model::TraceData;

use crate::axis::{self, Axis, Range};
use crate::banner::{BannerRow, Status};
use crate::canvas::{self, Canvas, MARGINS, anchor, gapped, label};
use crate::format;
use crate::primitives::{Color, HAlign, Polyline, Rect, Scene, Stroke, VAlign, Viewport};
use crate::readout;
use crate::theme::Theme;
use crate::time::Freshness;
use crate::view::{IrAxes, IrExtent, IrMode, Style, ViewState};

#[derive(Clone, Debug, PartialEq)]
pub struct IrScene {
    pub scene: Scene,
    pub plot: Rect,
    pub x_axis: Axis,
    pub y_axis: Axis,
    pub title: String,
    /// `t = 0 at inserted delay 12.34 ms · 4.24 m @ 20 °C`.
    pub origin: String,
    /// Why nothing is drawn, when that is the case.
    pub note: Option<String>,
    /// The curve's freshness as its transfer curve's legend says it (`stopped`,
    /// `STALE 3.2 s`, `audio stopped`), or a stored trace's name, drawn after the origin.
    pub tag: Option<String>,
    /// Banner strip above the plot; zero height when no banner is up.
    pub strip: Rect,
    pub banners: Vec<BannerRow>,
    /// Room parameters drawn under the plot (a sweep trace's IR), if any.
    pub room: Option<crate::room::RoomTable>,
    /// The cursor's reading, when the cursor is on.
    pub cursor: Option<IrCursor>,
    /// Simulated reflections marked over a sweep's measured response
    /// ([`crate::room::sim_marks`]); `None` when none are drawn.
    pub sim: Option<crate::room::sim_marks::SimMarks>,
}

/// What the IR cursor reads: the sample nearest its time, as the view draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct IrCursor {
    /// The sample's time, ms re t = 0.
    pub t_ms: f64,
    /// `1.25 ms`.
    pub time: String,
    /// `+0.500 FS` (linear), `−12.3 dB` (log, ETC).
    pub value: String,
}

impl IrCursor {
    /// The readout line: `1.25 ms · +0.500 FS`.
    pub fn text(&self) -> String {
        format!("{} · {}", self.time, self.value)
    }
}

/// Time of every point, ms re the inserted delay.
pub fn times_ms(frame: &IrFrame) -> Vec<f64> {
    let (t0, dt) = (frame.meta.t0.0, frame.meta.dt.0);
    (0..frame.linear.len())
        .map(|i| (t0 + i as f64 * dt) * 1000.0)
        .collect()
}

/// Where `frame` lies in time: the extent its time axis is navigated within.
pub fn extent(frame: &IrFrame) -> IrExtent {
    IrExtent::of(frame.meta.t0.0, frame.meta.dt.0, frame.linear.len())
}

/// The value axis's title in `mode`.
pub fn title(mode: IrMode) -> &'static str {
    match mode {
        IrMode::Linear => "Amplitude (FS)",
        IrMode::Log => "dB re peak",
        IrMode::Etc => "ETC dB re peak",
    }
}

fn finite_max(v: impl Iterator<Item = f64>) -> f64 {
    v.filter(|x| x.is_finite())
        .fold(f64::NEG_INFINITY, f64::max)
}

/// Values drawn for `mode`: amplitude (FS) or dB re the peak; `None` when the mode has no
/// data. A zero sample has no level: NaN in the log view.
pub fn ir_values(frame: &IrFrame, mode: IrMode) -> Option<Vec<f64>> {
    match mode {
        IrMode::Linear => Some(frame.linear.iter().map(|x| f64::from(*x)).collect()),
        IrMode::Log => {
            let peak = finite_max(frame.linear.iter().map(|x| f64::from(x.abs())));
            if peak.is_nan() || peak <= 0.0 {
                return None;
            }
            let v = frame
                .linear
                .iter()
                .map(|x| {
                    let a = f64::from(x.abs());
                    if a > 0.0 && a.is_finite() {
                        20.0 * (a / peak).log10()
                    } else {
                        f64::NAN
                    }
                })
                .collect();
            Some(v)
        }
        IrMode::Etc => {
            let etc = frame.etc.as_ref()?;
            let peak = finite_max(etc.iter().map(|x| f64::from(*x)));
            if !peak.is_finite() {
                return None;
            }
            Some(etc.iter().map(|x| f64::from(*x) - peak).collect())
        }
    }
}

/// The linear view's amplitude range when none was chosen: symmetric, 10 % over the peak,
/// so both polarities of the arrival show at the same scale.
pub fn auto_amplitude(frame: &IrFrame) -> Range {
    let peak = finite_max(frame.linear.iter().map(|x| f64::from(x.abs())));
    let lim = if peak > 0.0 { peak * 1.1 } else { 1.0 };
    Range::new(-lim, lim)
}

/// The value axis's range in `mode` with `axes`.
pub fn y_range(frame: &IrFrame, mode: IrMode, axes: &IrAxes) -> Range {
    match mode {
        IrMode::Linear => axes
            .amplitude
            .filter(Range::is_valid)
            .unwrap_or_else(|| auto_amplitude(frame)),
        IrMode::Log | IrMode::Etc => axes.level_db,
    }
}

/// The time axis's range with `axes`: the chosen one, else the whole IR.
pub fn time_range(frame: &IrFrame, axes: &IrAxes) -> Range {
    axes.time_ms
        .filter(Range::is_valid)
        .unwrap_or(extent(frame).full)
}

/// The cursor's reading at `axes.cursor_ms`, at the nearest sample of the values `mode`
/// draws.
pub fn cursor_reading(frame: &IrFrame, mode: IrMode, axes: &IrAxes) -> Option<IrCursor> {
    let t = axes.cursor_ms?;
    let e = extent(frame);
    let i = e.nearest(t)?;
    let t_ms = e.time_of(i);
    let v = ir_values(frame, mode)
        .and_then(|v| v.get(i).copied())
        .unwrap_or(f64::NAN);
    let value = match mode {
        IrMode::Linear => format::amplitude_readout(v),
        IrMode::Log | IrMode::Etc => format::db_readout(v),
    };
    Some(IrCursor {
        t_ms,
        time: format::ir_time(t_ms, e.dt_ms),
        value,
    })
}

/// What an IR pane draws around its curve: the banners, the view (its mode) on `axes` (the
/// IR pane's or the sweep view's), the style and the pane's size.
#[derive(Clone, Copy, Debug)]
pub struct IrCtx<'a> {
    pub status: &'a Status,
    pub view: &'a ViewState,
    pub axes: &'a IrAxes,
    pub style: Style<'a>,
    pub size: Viewport,
}

/// The IR `frame` in the view's mode on the context's axes.
pub fn ir_scene(
    frame: &IrFrame,
    color: Color,
    freshness: Option<Freshness>,
    ctx: &IrCtx,
) -> IrScene {
    let tag = freshness.and_then(|f| f.tag());
    draw(frame, color, freshness, tag, ctx)
}

/// A stored transfer trace's impulse response as an IR frame: the samples it was captured
/// with, time zero at its `delay` (the inserted delay at capture).
pub fn stored_frame(d: &TraceData) -> Option<IrFrame> {
    let s = d.ir.as_ref()?;
    Some(IrFrame {
        meas: ac2_proto::units::MeasId(0),
        meta: ac2_proto::frame::IrMeta {
            sample_rate: s.sample_rate,
            t0: s.ir.t0,
            dt: s.ir.dt,
            inserted_delay: d.meta.delay,
        },
        linear: s.ir.linear.clone(),
        etc: Some(s.ir.etc_db.clone()),
    })
}

/// A stored transfer trace's impulse response in the transfer IR view: drawn in the trace's
/// colour and tagged with its name, as its row in the transfer legend names it. `None` for
/// a trace captured without one.
pub fn stored_ir_scene(d: &TraceData, color: Color, ctx: &IrCtx) -> Option<IrScene> {
    let frame = stored_frame(d)?;
    let tag = Some(d.meta.edit.name.clone());
    Some(draw(&frame, color, None, tag, ctx))
}

/// Points above which an IR line is drawn [`per_pixel`]: a live IR (4096 points) is drawn
/// through every sample, a long stored or imported one through its pixel extremes.
const THIN_ABOVE: usize = 16_384;

/// The polyline through `xs`/`ys` (pixels, `xs` ascending) with at most two points per pixel
/// column: each column's lowest and highest point in their order. A long IR (an imported
/// file of 10⁵ samples) draws as the same envelope a full-rate line would, with one vertex
/// per pixel edge instead of one per sample. A NaN (a gap) is kept as it is.
fn per_pixel(xs: &[f32], ys: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let (mut ox, mut oy) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i < xs.len() {
        let col = xs[i].floor();
        let mut j = i;
        let (mut lo, mut hi): (Option<usize>, Option<usize>) = (None, None);
        while j < xs.len() && xs[j].floor() == col && !ys[j].is_nan() {
            if lo.is_none_or(|k| ys[j] < ys[k]) {
                lo = Some(j);
            }
            if hi.is_none_or(|k| ys[j] > ys[k]) {
                hi = Some(j);
            }
            j += 1;
        }
        let mut pick: Vec<usize> = [lo, hi].into_iter().flatten().collect();
        pick.sort_unstable();
        pick.dedup();
        for k in pick {
            ox.push(xs[k]);
            oy.push(ys[k]);
        }
        if j == i {
            // A NaN (or an x that is NaN): a gap, kept.
            ox.push(xs[i]);
            oy.push(ys[i]);
            j += 1;
        }
        i = j;
    }
    (ox, oy)
}

fn draw(
    frame: &IrFrame,
    color: Color,
    freshness: Option<Freshness>,
    tag: Option<String>,
    ctx: &IrCtx,
) -> IrScene {
    let &IrCtx {
        status,
        view,
        axes,
        style: Style { chrome, theme },
        size,
    } = ctx;
    let mode = view.ir.mode;
    let mut c = Canvas::new(size, theme);
    let plot_w = (size.width - MARGINS.left - MARGINS.right).max(1.0);
    let strip = canvas::banner_strip(&mut c, status, MARGINS.left, plot_w, size, theme);
    let plot = canvas::plot_area(size, strip.rect.bottom(), MARGINS.right);
    let t = times_ms(frame);
    let trange = time_range(frame, axes);
    let x_axis = axis::linear_axis(trange, plot.x, plot.right(), "ms");
    let values = ir_values(frame, mode);
    let yrange = y_range(frame, mode, axes);
    let title = title(mode).to_string();
    let y_unit = if mode == IrMode::Linear { "FS" } else { "dB" };
    let y_axis = axis::linear_axis(yrange, plot.bottom(), plot.y, y_unit);
    canvas::pane_frame(
        &mut c,
        plot,
        &x_axis,
        &y_axis,
        true,
        &title,
        Style { chrome, theme },
    );
    let (xm, ym) = (x_axis.mapping, y_axis.mapping);
    if mode == IrMode::Linear {
        canvas::hline(&mut c, plot, ym.to_px(0.0), theme.zero_line);
    }
    // t = 0: where the inserted delay puts the arrival.
    canvas::vline(&mut c.base, plot, xm.to_px(0.0), theme.zero_line);
    let origin = format!(
        "t = 0 at inserted delay {}",
        readout::delay_readout(frame.meta.inserted_delay.0, view.temperature_c)
    );
    let origin_line = match &tag {
        Some(t) => format!("{origin} · {t}"),
        None => origin.clone(),
    };
    // Beside the title when both fit, else on the row below it.
    let side_by_side = 6.0
        + canvas::text_width(&title, theme.small_font_size)
        + 16.0
        + canvas::text_width(&origin_line, theme.small_font_size)
        + 6.0
        <= plot.w;
    let origin_y = if side_by_side {
        plot.y + 4.0
    } else {
        plot.y + 4.0 + 1.4 * theme.small_font_size
    };
    c.base.labels.push(label(
        origin_line,
        [plot.right() - 6.0, origin_y],
        anchor(HAlign::Right, VAlign::Top),
        theme.small_font_size,
        theme.text_dim,
    ));

    let note = match &values {
        None if mode == IrMode::Etc && frame.etc.is_none() => {
            Some("ETC not published for this measurement".to_string())
        }
        None => Some("no impulse energy".to_string()),
        Some(_) => None,
    };
    if let Some(v) = &values {
        let xs: Vec<f32> = t.iter().map(|x| xm.to_px(*x)).collect();
        // Log views: points below the axis sit on its floor instead of vanishing, so the
        // decay stays a connected line.
        let floor = yrange.lo;
        let ys: Vec<f32> = v
            .iter()
            .map(|y| {
                if mode != IrMode::Linear && y.is_nan() {
                    ym.to_px(floor)
                } else if mode != IrMode::Linear {
                    ym.to_px(y.max(floor))
                } else {
                    ym.to_px(*y)
                }
            })
            .collect();
        let (xs, ys) = if xs.len() > THIN_ABOVE {
            per_pixel(&xs, &ys)
        } else {
            (xs, ys)
        };
        let (points, _) = gapped(&xs, &ys, None, |_, _| false);
        let dim = if freshness.is_some_and(|f| f.is_stale()) {
            theme.stale_alpha
        } else {
            1.0
        };
        c.data.polylines.push(Polyline {
            points,
            alpha: vec![],
            stroke: Stroke::solid(color.with_alpha(dim), (theme.trace_width * 0.75).max(1.0)),
            clip: Some(plot),
        });
    }
    let cursor = values
        .as_ref()
        .filter(|_| chrome.cursor())
        .and_then(|_| cursor_reading(frame, mode, axes));
    if let Some(cur) = &cursor {
        canvas::vline(&mut c.overlay, plot, xm.to_px(cur.t_ms), theme.cursor);
        // Under the origin line, right-aligned with it.
        c.overlay.labels.push(label(
            cur.text(),
            [plot.right() - 6.0, origin_y + 1.4 * theme.small_font_size],
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
    IrScene {
        scene: c.into_scene(size),
        plot,
        x_axis,
        y_axis,
        title,
        origin,
        note,
        tag,
        strip: strip.rect,
        banners: strip.rows,
        room: None,
        cursor,
        sim: None,
    }
}

/// `note` as it fits `width`: one line at the pane's font, else at the small font, else
/// broken after its name (`: ` or ` — `) into two lines, each cut with `…` if need be.
pub fn note_lines(note: &str, width: f32, theme: &Theme) -> (Vec<String>, f32) {
    let fits = |t: &str, size: f32| canvas::text_width(t, size) <= width;
    if fits(note, theme.font_size) {
        return (vec![note.to_string()], theme.font_size);
    }
    let size = theme.small_font_size;
    if fits(note, size) {
        return (vec![note.to_string()], size);
    }
    let cut = |t: &str| {
        if fits(t, size) {
            return t.to_string();
        }
        let mut out = t.to_string();
        while !out.is_empty() && !fits(&format!("{out}…"), size) {
            out.pop();
        }
        format!("{}…", out.trim_end())
    };
    let split = note
        .split_once(" — ")
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .or_else(|| {
            note.split_once(": ")
                .map(|(a, b)| (format!("{a}:"), b.to_string()))
        });
    match split {
        Some((a, b)) => (vec![cut(&a), cut(&b)], size),
        None => (vec![cut(note)], size),
    }
}

/// Why the IR pane has no picture of its transfer measurement's impulse response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IrMissing {
    /// The measurement is stopped: its last IR is not drawn as live.
    Stopped,
    /// The session's audio stopped.
    AudioStopped,
    /// Its transfer stream says the reference carries nothing, with this app's stimulus
    /// doing this.
    NoReference(crate::stimulus::Drive),
    /// Its transfer stream says the measurement input is below its floor.
    NoSignal,
    /// Running, nothing wrong known: the first frame is on its way.
    NotYet,
    /// Its curves are hidden in this app (it keeps measuring).
    Hidden,
}

/// The empty IR pane's text for measurement `name`; `key` brings the IR back: the key that
/// starts a stopped measurement, or shows a hidden one.
pub fn missing_text(name: &str, why: IrMissing, key: &str) -> String {
    match why {
        IrMissing::Stopped => format!("{name} stopped — {key} starts it"),
        IrMissing::Hidden => format!("{name} hidden — {key} shows it"),
        IrMissing::AudioStopped => format!("{name}: the audio stopped — the IR returns with it"),
        IrMissing::NoReference(d) => crate::stimulus::no_reference_note(&d),
        IrMissing::NoSignal => "no signal: the measurement input is below its floor".into(),
        IrMissing::NotYet => format!("{name}: no IR frame yet"),
    }
}

/// The IR pane without an IR to draw: the banners of its transfer measurement and an empty
/// plot that says why (`note`).
pub fn missing_scene(
    note: String,
    status: &Status,
    axes: &IrAxes,
    style: Style,
    size: Viewport,
) -> IrScene {
    let Style { chrome, theme } = style;
    let mut c = Canvas::new(size, theme);
    let plot_w = (size.width - MARGINS.left - MARGINS.right).max(1.0);
    let strip = canvas::banner_strip(&mut c, status, MARGINS.left, plot_w, size, theme);
    let plot = canvas::plot_area(size, strip.rect.bottom(), MARGINS.right);
    let trange = axes
        .time_ms
        .filter(Range::is_valid)
        .unwrap_or(Range::new(-1.0, 10.0));
    let x_axis = axis::linear_axis(trange, plot.x, plot.right(), "ms");
    let y_axis = axis::linear_axis(Range::new(-1.0, 1.0), plot.bottom(), plot.y, "FS");
    canvas::pane_frame(
        &mut c,
        plot,
        &x_axis,
        &y_axis,
        true,
        "",
        Style { chrome, theme },
    );
    let (lines, font) = note_lines(&note, plot.w - 12.0, theme);
    let pitch = font * 1.4;
    let top = plot.y + plot.h / 2.0 - pitch * (lines.len() as f32 - 1.0) / 2.0;
    for (i, line) in lines.into_iter().enumerate() {
        c.overlay.labels.push(crate::primitives::Label {
            clip: Some(plot),
            ..label(
                line,
                [plot.x + plot.w / 2.0, top + pitch * i as f32],
                anchor(HAlign::Center, VAlign::Center),
                font,
                theme.text_dim,
            )
        });
    }
    IrScene {
        scene: c.into_scene(size),
        plot,
        x_axis,
        y_axis,
        title: String::new(),
        origin: String::new(),
        note: Some(note),
        tag: None,
        strip: strip.rect,
        banners: strip.rows,
        room: None,
        cursor: None,
        sim: None,
    }
}

#[cfg(test)]
mod tests;
