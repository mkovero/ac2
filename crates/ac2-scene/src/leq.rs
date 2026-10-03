//! Rolling Leq windows (`docs/design/leq.md`): one tile per window, big enough to read from
//! across a room — amber when near its limit, red when over, back to normal when it
//! recovers — and a history strip of each window's value against its limit.
//!
//! Display truth only: the daemon judges (the state comes with the `leq` frame), this
//! module words and colours it. Window names follow IEC 61672 notation (`LAeq 30 min`).

use std::collections::VecDeque;

use ac2_proto::frame::{LeqFlags, LeqFrame};
use ac2_proto::model::{LeqConfig, LeqJudgement, LeqWindow, LevelScale, Weighting};

use crate::axis::{self, Axis, Mapping, Range, Scale, Steps};
use crate::banner::{BannerRow, Status};
use crate::canvas::{self, Canvas, anchor, label};
use crate::format;
use crate::primitives::{
    Color, Dash, FillRect, HAlign, Polyline, Rect, Scene, Stroke, VAlign, Viewport,
};
use crate::theme::Theme;
use crate::view::{LeqLayout, LeqStyle};

mod columns;
mod run;
pub use columns::{
    ABOVE_LIMIT_DB, BELOW_LIMIT_DB, FREE_SPAN_DB, LeqColumn, LeqColumns, column_colors,
    column_range,
};
pub use run::{LeqRunText, NewLogConfirm, new_log_confirm, run_text};

fn w_letter(w: Weighting) -> &'static str {
    match w {
        Weighting::A => "A",
        Weighting::C => "C",
        Weighting::Z => "Z",
    }
}

/// A window length in words: `30 s`, `5 min`, `90 min`, `3 h`, `1 min 30 s`. Minutes up to
/// two hours, as limits are written (LAeq,60min).
pub fn length(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return format::NO_VALUE.to_string();
    }
    let s = seconds.round() as u64;
    if s < 60 {
        format!("{s} s")
    } else if s > 7200 && s.is_multiple_of(3600) {
        format!("{} h", s / 3600)
    } else if s.is_multiple_of(60) {
        format!("{} min", s / 60)
    } else {
        format!("{} min {} s", s / 60, s % 60)
    }
}

/// `LAeq 30 min`.
pub fn window_name(w: &LeqWindow) -> String {
    format!("L{}eq {}", w_letter(w.weighting), length(w.duration.0))
}

/// Elapsed time as a clock: `0:05`, `12:30`, `1:02:03`.
pub fn clock(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return format::NO_VALUE.to_string();
    }
    let s = seconds.floor() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// How a tile is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TileState {
    /// No limit: values only.
    NoLimit,
    /// A limit, not judged (dBFS).
    NotCalibrated,
    Ok,
    Near,
    Over,
}

impl From<LeqJudgement> for TileState {
    fn from(j: LeqJudgement) -> Self {
        match j {
            LeqJudgement::NoLimit => TileState::NoLimit,
            LeqJudgement::NotCalibrated => TileState::NotCalibrated,
            LeqJudgement::Ok => TileState::Ok,
            LeqJudgement::Near => TileState::Near,
            LeqJudgement::Over => TileState::Over,
        }
    }
}

/// Every string of one window's tile.
#[derive(Clone, Debug, PartialEq)]
pub struct LeqTile {
    /// `LAeq 30 min`.
    pub name: String,
    /// `96.4`, or `—` before anything was measured.
    pub value: String,
    /// `dB SPL` or `dBFS`.
    pub unit: String,
    pub state: TileState,
    /// `OVER`, `NEAR`, `OK`, `not calibrated`; none without a limit.
    pub state_text: Option<String>,
    /// `limit 99.0 dB`.
    pub limit: Option<String>,
    /// `next 1 min ≤ 101.5 dB`, or `over — can't recover within 1 min`.
    pub headroom: Option<String>,
    /// `at the limit: back under in 7 min 30 s` when it cannot recover within the horizon.
    pub recover: Option<String>,
    /// `12:30 / 30:00` while the window fills.
    pub filling: Option<String>,
    /// `gaps: 28:10 of 30:00 measured`.
    pub incomplete: Option<String>,
    /// The figures the columns draw: the window's weighting and length (s), the Leq as
    /// shown (rounded to 0.1 dB, NaN before anything was measured), its limit when judged,
    /// the floored headroom when it can recover within the horizon, the time to recover
    /// when it cannot, and how much of the window has elapsed (s).
    pub weighting: Weighting,
    pub duration_s: f64,
    pub leq_db: f64,
    pub limit_db: Option<f64>,
    pub allowed_db: Option<f64>,
    pub recover_s: Option<f64>,
    pub elapsed_s: f64,
}

/// The tiles of a meter's windows from its configuration and newest `leq` frame. A frame
/// of another configuration (a different window count) gives none.
pub fn leq_tiles(cfg: &LeqConfig, f: &LeqFrame) -> Vec<LeqTile> {
    let n = cfg.windows.len();
    if [
        f.leq.len(),
        f.elapsed.len(),
        f.measured.len(),
        f.allowed.len(),
        f.recover.len(),
        f.flags.len(),
    ]
    .iter()
    .any(|&l| l != n)
    {
        return Vec::new();
    }
    let unit = match f.meta.scale {
        LevelScale::DbSpl => "dB SPL",
        LevelScale::Dbfs => "dBFS",
    };
    let horizon = length(f.meta.horizon.0);
    cfg.windows
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let flags = f.flags[i];
            let state = TileState::from(flags.judgement());
            let elapsed = f64::from(f.elapsed[i]);
            let measured = f64::from(f.measured[i]);
            let duration = w.duration.0;
            let judged = flags.contains(LeqFlags::JUDGED);
            let cannot = flags.contains(LeqFlags::CANNOT_RECOVER);
            // Floored: the figure is a ceiling to stay under (the margin only absorbs the
            // f32 the frame carries it in).
            let allowed_db = (judged && !cannot)
                .then(|| (f64::from(f.allowed[i]) * 10.0 + 1e-3).floor() / 10.0)
                .filter(|a| a.is_finite());
            let headroom = judged.then(|| {
                if cannot {
                    format!("over — can't recover within {horizon}")
                } else {
                    let a = allowed_db.unwrap_or(f64::NAN);
                    format!("next {horizon} ≤ {} dB", format::level(a))
                }
            });
            let recover_s = (judged && cannot)
                .then(|| f64::from(f.recover[i]))
                .filter(|r| r.is_finite());
            let recover =
                recover_s.map(|r| format!("at the limit: back under in {}", format::duration(r)));
            let leq = f64::from(f.leq[i]);
            LeqTile {
                name: window_name(w),
                value: format::level(leq),
                unit: unit.to_string(),
                state,
                state_text: match state {
                    TileState::NoLimit => None,
                    TileState::NotCalibrated => Some("not calibrated".into()),
                    TileState::Ok => Some("OK".into()),
                    TileState::Near => Some("NEAR".into()),
                    TileState::Over => Some("OVER".into()),
                },
                limit: w.limit.map(|l| format!("limit {} dB", format::level(l.0))),
                headroom,
                recover,
                filling: (elapsed < duration)
                    .then(|| format!("{} / {}", clock(elapsed), clock(duration))),
                incomplete: flags.contains(LeqFlags::INCOMPLETE).then(|| {
                    format!(
                        "gaps: {} of {} measured",
                        clock(measured),
                        clock(elapsed.min(duration))
                    )
                }),
                weighting: w.weighting,
                duration_s: duration,
                leq_db: if leq.is_finite() {
                    (leq * 10.0).round() / 10.0
                } else {
                    f64::NAN
                },
                limit_db: w.limit.filter(|_| judged).map(|l| l.0),
                allowed_db,
                recover_s,
                elapsed_s: elapsed,
            }
        })
        .collect()
}

/// Background and text colour of a tile.
pub fn tile_colors(state: TileState, theme: &Theme) -> (Color, Color) {
    match state {
        TileState::Over => (theme.banner_fault.background, theme.banner_fault.text),
        TileState::Near => (theme.banner_warning.background, theme.banner_warning.text),
        TileState::NoLimit | TileState::NotCalibrated | TileState::Ok => {
            (theme.plot_background, theme.text)
        }
    }
}

// ---------------------------------------------------------------------------------------
// History

/// Longest history kept, s.
pub const HISTORY_S: f64 = 4.0 * 3600.0;

/// One second of a window's history.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HistoryPoint {
    /// Daemon wall clock, s.
    pub t: f64,
    /// Leq (NaN: nothing measured).
    pub leq: f32,
    pub over: bool,
}

/// Each window's value over time, as received (one point per `leq` frame), keyed by the
/// window's length and weighting so a changed configuration keeps the windows it kept.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LeqHistory {
    series: Vec<((u64, Weighting), VecDeque<HistoryPoint>)>,
    scale: Option<LevelScale>,
    /// The columns' scale as of the newest frame ([`column_range`] keeps it while the
    /// levels allow).
    range: Option<Range>,
}

fn key(w: &LeqWindow) -> (u64, Weighting) {
    (w.duration.0.round() as u64, w.weighting)
}

impl LeqHistory {
    /// Adds the frame received at daemon wall time `t` (s). A frame of another
    /// configuration than `cfg` is ignored; a change of unit (a calibration) starts over.
    pub fn push(&mut self, cfg: &LeqConfig, f: &LeqFrame, t: f64) {
        if f.leq.len() != cfg.windows.len() || f.flags.len() != cfg.windows.len() {
            return;
        }
        if self.scale != Some(f.meta.scale) {
            self.series.clear();
            self.scale = Some(f.meta.scale);
            self.range = None;
        }
        let (limits, values) = scale_inputs(cfg, f);
        self.range = Some(column_range(&limits, &values, f.meta.scale, self.range));
        for (i, w) in cfg.windows.iter().enumerate() {
            let k = key(w);
            let at = match self.series.iter().position(|(sk, _)| *sk == k) {
                Some(p) => p,
                None => {
                    self.series.push((k, VecDeque::new()));
                    self.series.len() - 1
                }
            };
            let s = &mut self.series[at].1;
            if s.back().is_some_and(|p| p.t >= t) {
                continue;
            }
            s.push_back(HistoryPoint {
                t,
                leq: f.leq[i],
                over: f.flags[i].contains(LeqFlags::OVER),
            });
            while s.front().is_some_and(|p| p.t < t - HISTORY_S) {
                s.pop_front();
            }
        }
    }

    /// Points of window `w`, oldest first.
    pub fn points(&self, w: &LeqWindow) -> Option<&VecDeque<HistoryPoint>> {
        let k = key(w);
        self.series.iter().find(|(sk, _)| *sk == k).map(|(_, s)| s)
    }

    /// Newest time of any point.
    pub fn newest(&self) -> Option<f64> {
        self.series
            .iter()
            .filter_map(|(_, s)| s.back().map(|p| p.t))
            .fold(None, |a: Option<f64>, t| Some(a.map_or(t, |a| a.max(t))))
    }

    /// Whether anything was received.
    pub fn is_empty(&self) -> bool {
        self.series.iter().all(|(_, s)| s.is_empty())
    }

    /// The columns' scale as of the newest frame.
    pub fn range(&self) -> Option<Range> {
        self.range
    }
}

/// What the columns' scale follows: the limits of the windows the daemon judges, and every
/// window's value.
fn scale_inputs(cfg: &LeqConfig, f: &LeqFrame) -> (Vec<f64>, Vec<f64>) {
    let limits = cfg
        .windows
        .iter()
        .zip(&f.flags)
        .filter(|(_, fl)| fl.contains(LeqFlags::JUDGED))
        .filter_map(|(w, _)| w.limit.map(|l| l.0))
        .collect();
    let values = f.leq.iter().map(|v| f64::from(*v)).collect();
    (limits, values)
}

/// Time span the strip shows: twice the longest window, between 2 min and 2 h.
pub fn history_span(cfg: &LeqConfig) -> f64 {
    let longest = cfg.windows.iter().map(|w| w.duration.0).fold(0.0, f64::max);
    (2.0 * longest).clamp(120.0, 7200.0)
}

/// Time label of the strip: `now`, `−30 s`, `−10 min`, `−1 h 30 min`.
fn ago_label(v: f64, in_minutes: bool) -> String {
    if v.abs() < 1e-9 {
        return "now".into();
    }
    let s = if in_minutes { -v * 60.0 } else { -v };
    let t = length(s);
    format!("{}{t}", format::MINUS)
}

/// One window's line in the strip: the points as drawn and the over-limit runs.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryLine {
    pub name: String,
    pub color: Color,
    /// x, y in pixels; NaN breaks.
    pub points: Vec<[f32; 2]>,
    /// The parts drawn red: each run of points over the limit.
    pub over: Vec<Vec<[f32; 2]>>,
    /// y of the limit line, when the window has a judged limit in view.
    pub limit_y: Option<f32>,
}

/// The history strip as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryStrip {
    pub plot: Rect,
    pub x: Axis,
    pub y: Axis,
    pub lines: Vec<HistoryLine>,
}

/// Lays the history out in `plot`.
pub fn history_strip(
    cfg: &LeqConfig,
    h: &LeqHistory,
    judged: bool,
    plot: Rect,
    theme: &Theme,
) -> HistoryStrip {
    let span = history_span(cfg);
    let now = h.newest().unwrap_or(0.0);
    let in_minutes = span > 180.0;
    let unit = if in_minutes { 60.0 } else { 1.0 };
    let xm = Mapping::new(
        Range::new(-span / unit, 0.0),
        Scale::Linear,
        plot.x,
        plot.right(),
    );
    let x = Axis {
        ticks: axis::linear_ticks(&xm, Steps::Decimal, 70.0, 12.0, |v, _| {
            ago_label(v, in_minutes)
        }),
        mapping: xm,
        title: String::new(),
    };
    // y from what is in view: values and limits, at least 20 dB, on 5 dB steps.
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for w in &cfg.windows {
        if let Some(ps) = h.points(w) {
            for p in ps.iter().filter(|p| p.t >= now - span) {
                let v = f64::from(p.leq);
                if v.is_finite() {
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
        }
        if judged && let Some(l) = w.limit {
            lo = lo.min(l.0);
            hi = hi.max(l.0);
        }
    }
    if !lo.is_finite() {
        (lo, hi) = (60.0, 100.0);
    }
    let mut ylo = (lo / 5.0).floor() * 5.0 - 5.0;
    let mut yhi = (hi / 5.0).ceil() * 5.0 + 5.0;
    if yhi - ylo < 20.0 {
        let mid = (ylo + yhi) / 2.0;
        ylo = (mid - 10.0).floor();
        yhi = ylo + 20.0;
    }
    let y = axis::linear_axis(Range::new(ylo, yhi), plot.bottom(), plot.y, "dB");
    let lines = cfg
        .windows
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let color = theme.trace_color(i);
            let mut pts: Vec<[f32; 2]> = Vec::new();
            let mut over: Vec<Vec<[f32; 2]>> = Vec::new();
            let mut run: Vec<[f32; 2]> = Vec::new();
            let mut last_t: Option<f64> = None;
            for p in h.points(w).into_iter().flatten() {
                if p.t < now - span - 1.0 {
                    continue;
                }
                // More than two seconds without a frame: a break, not a line across.
                if last_t.is_some_and(|lt| p.t - lt > 2.5)
                    && pts.last().is_some_and(|q| q[0].is_finite())
                {
                    pts.push([f32::NAN, f32::NAN]);
                    if run.len() > 1 {
                        over.push(std::mem::take(&mut run));
                    }
                    run.clear();
                }
                last_t = Some(p.t);
                let v = f64::from(p.leq);
                if !v.is_finite() {
                    if pts.last().is_some_and(|q| q[0].is_finite()) {
                        pts.push([f32::NAN, f32::NAN]);
                    }
                    if run.len() > 1 {
                        over.push(std::mem::take(&mut run));
                    }
                    run.clear();
                    continue;
                }
                let xy = [x.mapping.to_px((p.t - now) / unit), y.mapping.to_px(v)];
                pts.push(xy);
                if p.over {
                    run.push(xy);
                } else if !run.is_empty() {
                    // The run ends where the line comes back under.
                    run.push(xy);
                    if run.len() > 1 {
                        over.push(std::mem::take(&mut run));
                    }
                    run.clear();
                }
            }
            if run.len() == 1 {
                // A single over second still shows: a short tick back to its left.
                let p = run[0];
                run.insert(0, [p[0] - 2.0, p[1]]);
            }
            if run.len() > 1 {
                over.push(run);
            }
            while pts.last().is_some_and(|q| !q[0].is_finite()) {
                pts.pop();
            }
            HistoryLine {
                name: window_name(w),
                color,
                points: pts,
                over,
                limit_y: w.limit.filter(|_| judged).map(|l| y.mapping.to_px(l.0)),
            }
        })
        .collect();
    HistoryStrip { plot, x, y, lines }
}

// ---------------------------------------------------------------------------------------
// Scene

/// What the Leq view shows.
#[derive(Clone, Debug, PartialEq)]
pub struct LeqView<'a> {
    /// `FOH SPL`.
    pub meter: String,
    /// Calibration text (`spl::cal_text`), with the mic.
    pub cal: String,
    pub cfg: &'a LeqConfig,
    pub tiles: Vec<LeqTile>,
    pub history: Option<&'a LeqHistory>,
    /// `STALE 3.2 s`.
    pub stale: Option<String>,
    /// What the values are in (the columns' scale without limits starts from it).
    pub scale: LevelScale,
    /// The headroom's horizon in words: `1 min`.
    pub horizon: String,
    pub layout: LeqLayout,
    /// The log as a whole: run clock, start, total, gaps (`None` before its first second).
    pub run: Option<LeqRunText>,
}

/// The Leq view as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct LeqScene {
    pub scene: Scene,
    /// Each tile's rectangle, in window order (tiles layout).
    pub tiles: Vec<Rect>,
    /// The columns, shortest window left (columns layout).
    pub columns: Option<LeqColumns>,
    /// The history strip, when there is room for it.
    pub history: Option<HistoryStrip>,
    pub strip: Rect,
    pub banners: Vec<BannerRow>,
    /// The caption above the windows: meter, run, calibration.
    pub caption: Rect,
    /// The run's wording as drawn, if there was room for any.
    pub run: Option<String>,
}

/// Columns of a grid of `n` tiles in `w × h` that makes the tiles largest at about 2:1.
fn columns(n: usize, w: f32, h: f32) -> usize {
    (1..=n.max(1))
        .max_by(|&a, &b| {
            let score = |c: usize| {
                let rows = n.div_ceil(c) as f32;
                let (tw, th) = (w / c as f32, h / rows);
                (tw / 2.0).min(th)
            };
            score(a).total_cmp(&score(b))
        })
        .unwrap_or(1)
}

fn draw_tile(c: &mut Canvas, t: &LeqTile, r: Rect, stale: bool, theme: &Theme) {
    let (bg, fg) = tile_colors(t.state, theme);
    c.base.rects.push(FillRect {
        rect: r,
        color: bg,
        clip: None,
    });
    let fg = if stale && t.state != TileState::Over && t.state != TileState::Near {
        theme.text_dim
    } else {
        fg
    };
    let pad = (r.h * 0.06).clamp(4.0, 14.0);
    let head = (r.h * 0.11).clamp(11.0, 30.0);
    let small = (r.h * 0.075).clamp(9.0, 20.0);
    let clip = Some(r);
    let mut push = |text: String, pos: [f32; 2], h: HAlign, v: VAlign, size: f32| {
        let mut l = label(text, pos, anchor(h, v), size, fg);
        l.clip = clip;
        c.overlay.labels.push(l);
    };
    push(
        t.name.clone(),
        [r.x + pad, r.y + pad],
        HAlign::Left,
        VAlign::Top,
        head,
    );
    if let Some(s) = &t.state_text {
        push(
            s.clone(),
            [r.right() - pad, r.y + pad],
            HAlign::Right,
            VAlign::Top,
            head,
        );
    }
    // The number as big as the tile allows: its height, and its width for "100.0".
    let lines_below = [&t.limit, &t.headroom, &t.recover, &t.filling, &t.incomplete]
        .iter()
        .filter(|x| x.is_some())
        .count()
        .min(3) as f32;
    let body_top = r.y + pad + head * 1.3;
    let body_bottom = r.bottom() - pad - lines_below * small * 1.3;
    let big = ((body_bottom - body_top) * 0.95)
        .min((r.w - 2.0 * pad) / (0.62 * 5.0 + 0.62 * 0.45 * 6.0))
        .max(10.0);
    let base = body_top + (body_bottom - body_top) * 0.5 + big * 0.36;
    let unit_size = (big * 0.3).max(9.0);
    let value_w = canvas::text_width(&t.value, big);
    let unit_w = canvas::text_width(&t.unit, unit_size);
    let x0 = r.x + (r.w - value_w - unit_w - 6.0) / 2.0 + value_w;
    push(
        t.value.clone(),
        [x0, base],
        HAlign::Right,
        VAlign::Baseline,
        big,
    );
    push(
        t.unit.clone(),
        [x0 + 6.0, base],
        HAlign::Left,
        VAlign::Baseline,
        unit_size,
    );
    // Below: limit and headroom first, then the recovery time, the filling and the gaps,
    // as many as fit.
    let mut below: Vec<String> = Vec::new();
    match (&t.limit, &t.headroom) {
        (Some(l), Some(hr)) => below.push(format!("{l} · {hr}")),
        (Some(l), None) => below.push(l.clone()),
        (None, Some(hr)) => below.push(hr.clone()),
        (None, None) => {}
    }
    below.extend(t.recover.iter().cloned());
    match (&t.filling, &t.incomplete) {
        (Some(f), Some(i)) => below.push(format!("{f} · {i}")),
        (Some(f), None) => below.push(f.clone()),
        (None, Some(i)) => below.push(i.clone()),
        (None, None) => {}
    }
    for (k, text) in below.into_iter().take(3).enumerate().rev() {
        let row = (lines_below - 1.0 - k as f32).max(0.0);
        push(
            text,
            [r.x + r.w / 2.0, r.bottom() - pad - row * small * 1.3],
            HAlign::Center,
            VAlign::Bottom,
            small,
        );
    }
}

fn draw_history(c: &mut Canvas, s: &HistoryStrip, theme: &Theme) {
    canvas::pane_frame(c, s.plot, &s.x, &s.y, true, "dB", theme);
    for l in &s.lines {
        if let Some(y) = l.limit_y {
            canvas::hline(
                c,
                s.plot,
                y,
                Stroke {
                    color: l.color,
                    width: 1.2,
                    dash: Some(Dash {
                        on: 6.0,
                        off: 4.0,
                        offset: 0.0,
                    }),
                },
            );
        }
        if l.points.len() > 1 {
            c.data.polylines.push(Polyline {
                points: l.points.clone(),
                alpha: vec![],
                stroke: Stroke::solid(l.color, theme.trace_width),
                clip: Some(s.plot),
            });
        }
        for run in &l.over {
            c.data.polylines.push(Polyline {
                points: run.clone(),
                alpha: vec![],
                stroke: Stroke::solid(theme.banner_fault.background, theme.trace_width * 2.5),
                clip: Some(s.plot),
            });
        }
    }
    // Legend: each window's name in its colour, along the top of the strip.
    let mut x = s.plot.x + 30.0;
    for l in &s.lines {
        c.overlay.labels.push(label(
            l.name.clone(),
            [x, s.plot.y + 4.0],
            anchor(HAlign::Left, VAlign::Top),
            theme.small_font_size,
            l.color,
        ));
        x += canvas::text_width(&l.name, theme.small_font_size) + 12.0;
    }
}

/// Lays the view out in `size` below the banner strip: the meter and calibration on one
/// line, the windows as columns or tiles, and the history strip below them when it is on
/// and the pane is tall enough.
pub fn leq_scene(v: &LeqView<'_>, status: &Status, theme: &Theme, size: Viewport) -> LeqScene {
    let mut c = Canvas::new(size, theme);
    let pad = 10.0;
    let strip = canvas::banner_strip(&mut c, status, pad, size.width - 2.0 * pad, size, theme);
    let top = strip.rect.bottom() + pad;
    let n = v.tiles.len();
    // The columns decide whether their names leave the weighting to the caption once the
    // caption's height is known; the run is placed clear of the longer of the two texts.
    let shared = v
        .tiles
        .first()
        .map(|t| t.weighting)
        .filter(|w| v.tiles.iter().all(|t| t.weighting == *w))
        .map(|w| format!("L{}eq", w_letter(w)));
    let right = match &v.stale {
        Some(s) => format!("{s} · {}", v.cal),
        None => v.cal.clone(),
    };
    let cap = caption(
        v,
        &caption_left(v, shared.as_deref()),
        &right,
        top,
        size,
        theme,
    );
    let area = Rect::new(
        pad,
        top + cap.height,
        (size.width - 2.0 * pad).max(1.0),
        (size.height - top - cap.height - pad).max(1.0),
    );
    // The strip takes a third of a tall pane; a short one is all windows.
    let show_history =
        v.layout.history && v.history.is_some() && area.h >= 300.0 && area.w >= 300.0;
    let (tiles_area, hist_area) = if show_history {
        let hh = (area.h * 0.34).min(260.0);
        (
            Rect::new(area.x, area.y, area.w, area.h - hh - pad),
            Some(Rect::new(
                area.x + canvas::MARGINS.left - pad,
                area.bottom() - hh + canvas::MARGINS.top,
                area.w - canvas::MARGINS.left + pad,
                hh - canvas::MARGINS.top - canvas::MARGINS.bottom,
            )),
        )
    } else {
        (area, None)
    };
    let mut rects = Vec::new();
    let mut cols = None;
    if n == 0 {
        c.overlay.labels.push(label(
            "no Leq windows: add them with Leq windows… (Ctrl+K)",
            [
                tiles_area.x + tiles_area.w / 2.0,
                tiles_area.y + tiles_area.h / 2.0,
            ],
            anchor(HAlign::Center, VAlign::Center),
            theme.font_size,
            theme.text_dim,
        ));
    } else if v.layout.style == LeqStyle::Columns {
        let limits: Vec<f64> = v.tiles.iter().filter_map(|t| t.limit_db).collect();
        let values: Vec<f64> = v.tiles.iter().map(|t| t.leq_db).collect();
        let range = column_range(
            &limits,
            &values,
            v.scale,
            v.history.and_then(LeqHistory::range),
        );
        cols = Some(columns::draw_columns(
            &mut c,
            &v.tiles,
            range,
            &v.horizon,
            tiles_area,
            v.stale.is_some(),
            theme,
        ));
    } else {
        let gap = 8.0;
        let ncols = columns(n, tiles_area.w, tiles_area.h);
        let rows = n.div_ceil(ncols);
        let tw = (tiles_area.w - gap * (ncols as f32 - 1.0)) / ncols as f32;
        let th = (tiles_area.h - gap * (rows as f32 - 1.0)) / rows as f32;
        for (i, t) in v.tiles.iter().enumerate() {
            let (row, col) = (i / ncols, i % ncols);
            let r = Rect::new(
                tiles_area.x + col as f32 * (tw + gap),
                tiles_area.y + row as f32 * (th + gap),
                tw.max(1.0),
                th.max(1.0),
            );
            draw_tile(&mut c, t, r, v.stale.is_some(), theme);
            rects.push(r);
        }
    }
    let left = caption_left(v, cols.as_ref().and_then(|k| k.weighting.as_deref()));
    // The caption: the meter (with the unit, and the weighting the columns' names leave
    // out), the run, and the calibration.
    c.overlay.labels.push(label(
        left,
        [pad, top],
        anchor(HAlign::Left, VAlign::Top),
        theme.font_size,
        theme.text_dim,
    ));
    if let Some((text, pos, size)) = &cap.run {
        c.overlay.labels.push(label(
            text.clone(),
            *pos,
            anchor(HAlign::Left, VAlign::Top),
            *size,
            if v.stale.is_some() {
                theme.text_dim
            } else {
                theme.text
            },
        ));
    }
    c.overlay.labels.push(label(
        right,
        [size.width - pad, top],
        anchor(HAlign::Right, VAlign::Top),
        theme.font_size,
        if v.stale.is_some() {
            theme.banner_warning.background
        } else {
            theme.text_dim
        },
    ));
    let judged = v
        .tiles
        .iter()
        .any(|t| matches!(t.state, TileState::Ok | TileState::Near | TileState::Over));
    let history = match (hist_area, v.history) {
        (Some(r), Some(h)) => {
            let s = history_strip(v.cfg, h, judged, r, theme);
            draw_history(&mut c, &s, theme);
            Some(s)
        }
        _ => None,
    };
    LeqScene {
        scene: c.into_scene(size),
        tiles: rects,
        columns: cols,
        history,
        strip: strip.rect,
        banners: strip.rows,
        caption: Rect::new(0.0, top, size.width, cap.height),
        run: cap.run.map(|(t, ..)| t),
    }
}

/// The caption's left part: the meter, and for columns the unit and the `weighting` their
/// names leave out.
fn caption_left(v: &LeqView<'_>, weighting: Option<&str>) -> String {
    if v.layout.style != LeqStyle::Columns || v.tiles.is_empty() {
        return v.meter.clone();
    }
    let unit = v.tiles.first().map_or("", |t| t.unit.as_str());
    match weighting {
        Some(w) => format!("{} · {w}, {unit}", v.meter),
        None => format!("{} · {unit}", v.meter),
    }
}

/// Where the run goes in the caption.
struct Caption {
    height: f32,
    /// Text, top-left position, size.
    run: Option<(String, [f32; 2], f32)>,
}

/// Lays the run into the caption: in large type between the meter and the calibration
/// when a wording down to the clock and the total fits there, else on a row of its own
/// below them in the longest wording that fits (at the caption's type size when nothing
/// fits large); left out only when not even the clock fits. Large type is a share of the
/// pane's height, so the run reads from a distance on a full-screen pane.
fn caption(
    v: &LeqView<'_>,
    left: &str,
    right: &str,
    top: f32,
    size: Viewport,
    theme: &Theme,
) -> Caption {
    let pad = 10.0;
    let gap = theme.font_size;
    let base_h = theme.font_size * 1.6;
    let Some(run) = &v.run else {
        return Caption {
            height: base_h,
            run: None,
        };
    };
    let big = (size.height * 0.04).clamp(theme.font_size, theme.font_size * 2.0);
    let variants = run.variants();
    let left_end = pad + canvas::text_width(left, theme.font_size);
    let right_start = size.width - pad - canvas::text_width(right, theme.font_size);
    let free = right_start - left_end - 2.0 * gap;
    // On the meter's row: down to the clock and the total, never the bare clock alone.
    let inline = variants.len().saturating_sub(1).max(1);
    if let Some(t) = variants
        .iter()
        .take(inline)
        .find(|t| canvas::text_width(t, big) <= free)
    {
        let w = canvas::text_width(t, big);
        let x = left_end + gap + (free - w) / 2.0;
        return Caption {
            height: (big * 1.35).max(base_h),
            run: Some((t.clone(), [x, top], big)),
        };
    }
    let row_y = top + base_h;
    let width = size.width - 2.0 * pad;
    for s in [big, theme.font_size] {
        if let Some(t) = variants.iter().find(|t| canvas::text_width(t, s) <= width) {
            return Caption {
                height: base_h + (s * 1.35).max(base_h),
                run: Some((t.clone(), [pad, row_y], s)),
            };
        }
    }
    Caption {
        height: base_h,
        run: None,
    }
}

#[cfg(test)]
mod tests;
