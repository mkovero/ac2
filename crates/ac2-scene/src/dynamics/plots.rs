//! Envelopes (the picked step's onset above its release, every curve on one matched scale)
//! and Release (release τ and recovery against burst length).

use super::draw::{DynamicsFrame, DynamicsScene, contains, legend_row};
use super::*;
use crate::axis::{self, Axis, Mapping, Range, Scale, Tick, TickKind};
use crate::canvas::{self, Canvas, PANE_GAP};
use crate::primitives::{Dash, Point, Points, Polyline, Rect, Stroke};
use crate::view::Style;

/// A time axis from an edge: linear up to one carrier period after it (the edge itself and
/// what comes before it), log from there, so a 1 ms attack and a 10 s release both read.
#[derive(Clone, Copy, Debug)]
struct EdgeAxis {
    lo: f64,
    p: f64,
    hi: f64,
    x0: f32,
    xs: f32,
    x1: f32,
}

/// The share of the width the linear stretch takes when a log stretch follows it.
const LINEAR_SHARE: f32 = 0.3;

impl EdgeAxis {
    fn new(lo: f64, p: f64, hi: f64, x0: f32, x1: f32) -> Self {
        let p = p.max(lo + 1e-6);
        let (p, xs) = if hi > p * 1.05 {
            (p, x0 + (x1 - x0) * LINEAR_SHARE)
        } else {
            (hi.max(p), x1)
        };
        Self {
            lo,
            p,
            hi: hi.max(p),
            x0,
            xs,
            x1,
        }
    }

    fn px(&self, t: f64) -> f32 {
        if !t.is_finite() || t < self.lo || t > self.hi * 1.000_001 {
            return f32::NAN;
        }
        if t <= self.p {
            self.x0 + ((t - self.lo) / (self.p - self.lo)) as f32 * (self.xs - self.x0)
        } else {
            self.xs + ((t / self.p).ln() / (self.hi / self.p).ln()) as f32 * (self.x1 - self.xs)
        }
    }

    fn t(&self, x: f32) -> f64 {
        if x <= self.xs {
            self.lo + f64::from((x - self.x0) / (self.xs - self.x0).max(1e-6)) * (self.p - self.lo)
        } else {
            self.p * (self.hi / self.p).powf(f64::from((x - self.xs) / (self.x1 - self.xs)))
        }
    }

    /// Ticks: the range's start and the edge on the linear stretch, the 1-2-5 values of each
    /// decade on the log stretch, labelled at the decades.
    fn axis(&self) -> Axis {
        let mut ticks = Vec::new();
        let mut push = |v: f64, major: bool| {
            ticks.push(Tick {
                value: v,
                pos: self.px(v),
                kind: if major {
                    TickKind::Major
                } else {
                    TickKind::Minor
                },
                label: major.then(|| tick_text(v)),
            });
        };
        if self.lo < 0.0 {
            push(self.lo, true);
        }
        push(0.0, true);
        if self.xs < self.x1 {
            let mut d = 10f64.powf(self.p.log10().ceil());
            while d <= self.hi * 1.000_001 {
                push(d, true);
                for k in [2.0, 5.0] {
                    if d * k <= self.hi {
                        push(d * k, false);
                    }
                }
                d *= 10.0;
            }
        }
        Axis {
            mapping: Mapping::new(
                Range::new(self.lo, self.hi),
                Scale::Linear,
                self.x0,
                self.x1,
            ),
            ticks,
            title: "s".to_string(),
        }
    }
}

fn tick_text(t: f64) -> String {
    if t == 0.0 {
        "0".to_string()
    } else if t.abs() < 1.0 {
        format!("{} ms", fixed(t * 1e3, 0))
    } else {
        format!("{} s", fixed(t, 0))
    }
}

/// What an envelope point reads: gain reduction, or THD (NaN where blind).
fn y_of(e: &ac2_proto::model::DynamicsEdgeCurve, i: usize, thd: bool) -> f32 {
    if thd {
        thd_db([e.h_db[0][i], e.h_db[1][i], e.h_db[2][i], e.h_db[3][i]])
    } else {
        e.gr_db[i]
    }
}

/// The upper plot's time labels sit under it: the gap leaves them a row clear of the
/// lower plot's level labels.
const TICK_ROW: f32 = 18.0;

fn split(area: Rect) -> (Rect, Rect) {
    let gap = PANE_GAP + TICK_ROW;
    let h = ((area.h - gap) / 2.0).max(1.0);
    (
        Rect::new(area.x, area.y, area.w, h),
        Rect::new(area.x, area.y + h + gap, area.w, h),
    )
}

/// Where the pick reads before the curves' envelopes arrive.
pub const FETCHING: &str = "reading envelopes…";

pub(super) fn envelopes(
    c: &mut Canvas,
    out: &mut DynamicsScene,
    frame: &DynamicsFrame<'_>,
    p: DynamicsStepPick,
    area: Rect,
    style: Style<'_>,
) -> Vec<(String, Color)> {
    let theme = style.theme;
    let thd = frame.modes.thd;
    let steps = super::draw::curve_steps(frame);
    // Each curve with its step and envelope.
    let shown: Vec<(&DynCurve<'_>, usize, &DynamicsEnvelope)> = frame
        .curves
        .iter()
        .zip(&steps)
        .filter_map(|(cv, s)| {
            let s = (*s)? as usize;
            let e = cv.envelope.filter(|e| e.step as usize == s)?;
            Some((cv, s, e))
        })
        .collect();
    let (top, bottom) = split(area);
    out.plots = vec![top, bottom];
    let burst = shown
        .iter()
        .filter_map(|(cv, s, _)| cv.info().steps.get(*s).map(|x| x.burst.0))
        .fold(p.burst.0, f64::max);
    let rest = shown
        .iter()
        .filter_map(|(cv, s, _)| cv.info().steps.get(*s).map(|x| x.rest.0))
        .fold(0.0, f64::max)
        .max(0.05);
    let period = frame.curves.first().map_or(1e-3, |cv| {
        f64::from(cv.info().period) / cv.info().sample_rate_hz.0
    });
    let onset = EdgeAxis::new(-0.005, period, burst, top.x, top.right());
    let release = EdgeAxis::new(0.0, period, rest, bottom.x, bottom.right());
    // One y scale for both plots and every curve.
    let ys = shown.iter().flat_map(|(_, _, e)| {
        [&e.start, &e.end]
            .into_iter()
            .flat_map(move |edge| (0..edge.t.len()).map(move |i| y_of(edge, i, thd)))
    });
    let (lo, hi) = ys
        .filter(|v| v.is_finite())
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(l, h), v| {
            (l.min(v), h.max(v))
        });
    let range = if thd {
        if lo.is_finite() {
            Range::new(
                f64::from((lo / 10.0).floor() * 10.0),
                f64::from((hi / 10.0).ceil() * 10.0 + 10.0),
            )
        } else {
            Range::new(-100.0, -20.0)
        }
    } else if lo.is_finite() {
        Range::new(
            f64::from(lo.min(0.0).floor()) - 1.0,
            f64::from(hi.max(1.0).ceil()) + 1.0,
        )
    } else {
        Range::new(-1.0, 10.0)
    };
    let title = if thd { "dB re carrier" } else { "dB GR" };
    let ya = axis::linear_axis(range, top.bottom(), top.y, title);
    let yb = axis::linear_axis(range, bottom.bottom(), bottom.y, title);
    canvas::pane_frame(
        c,
        top,
        &onset.axis(),
        &ya,
        true,
        &format!("onset · {title}"),
        style,
    );
    canvas::pane_frame(
        c,
        bottom,
        &release.axis(),
        &yb,
        true,
        &format!("release · {title}"),
        style,
    );
    for (cv, s, e) in &shown {
        let flags = flagged(&baselines(cv.info(), cv.rows()));
        // A step after a moved baseline started from elsewhere: dashed.
        let after_moved = *s > 0 && flags.get(*s) == Some(&true);
        let stroke = Stroke {
            color: cv.color,
            width: theme.trace_width,
            dash: after_moved.then_some(Dash {
                on: 6.0,
                off: 4.0,
                offset: 0.0,
            }),
        };
        for (edge, ax, ym, plot) in [
            (&e.start, &onset, &ya.mapping, top),
            (&e.end, &release, &yb.mapping, bottom),
        ] {
            let pts: Vec<[f32; 2]> = (0..edge.t.len())
                .filter(|&i| f64::from(edge.t[i]) >= ax.lo)
                .map(|i| {
                    [
                        ax.px(f64::from(edge.t[i])),
                        ym.to_px(f64::from(y_of(edge, i, thd))),
                    ]
                })
                .collect();
            c.data.polylines.push(Polyline::clipped(pts, stroke, plot));
        }
        out.legend.push(legend_row(cv, cv.name.clone(), cv.color));
    }
    let mut lines = vec![(pick_text(p), theme.text)];
    for (cv, s) in frame.curves.iter().zip(&steps) {
        if s.is_none() {
            lines.push((format!("{}  {NO_SUCH_STEP}", cv.name), cv.color));
        } else if !shown.iter().any(|(x, _, _)| x.id == cv.id) {
            lines.push((format!("{}  {FETCHING}", cv.name), cv.color));
        }
    }
    let hover = frame.pointer.and_then(|q| {
        if contains(top, q) {
            Some((onset.t(q[0]), true))
        } else if contains(bottom, q) {
            Some((release.t(q[0]), false))
        } else {
            None
        }
    });
    if let Some((t, start)) = hover {
        let x = if start { onset.px(t) } else { release.px(t) };
        canvas::vline(
            &mut c.overlay,
            if start { top } else { bottom },
            x,
            theme.cursor,
        );
        lines.push((
            format!(
                "{} {}",
                if start { "onset" } else { "release" },
                signed_time(t)
            ),
            theme.text,
        ));
        for (cv, _, e) in &shown {
            let edge = if start { &e.start } else { &e.end };
            let v = nearest(&edge.t, t).map_or(f32::NAN, |i| y_of(edge, i, thd));
            let text = if v.is_finite() {
                format!("{} dB", fixed(f64::from(v), 1))
            } else {
                BELOW_FLOOR.to_string()
            };
            lines.push((format!("{}  {text}", cv.name), cv.color));
        }
    }
    lines
}

fn signed_time(t: f64) -> String {
    if t < 0.0 {
        format!("{MINUS}{}", time_text(-t))
    } else {
        time_text(t)
    }
}

fn nearest(ts: &[f32], t: f64) -> Option<usize> {
    (0..ts.len()).min_by(|&i, &j| {
        (f64::from(ts[i]) - t)
            .abs()
            .total_cmp(&(f64::from(ts[j]) - t).abs())
    })
}

/// One point of a Release plot: its length, the value, and whether it is a bound (hollow).
struct ReleasePoint {
    x: f64,
    y: f64,
    /// What the point is a bound of: drawn hollow, read as the word.
    bound: Option<&'static str>,
}

/// Release τ (`recovery` false) or recovery to 0.5 dB of `cv`'s steps at `level`: a time,
/// or at its bound, hollow ("≤ 1 period" at one period, "can't tell" at the rest it was not
/// within).
fn release_points(cv: &DynCurve<'_>, level: f64, recovery: bool) -> Vec<ReleasePoint> {
    let info = cv.info();
    let period = f64::from(info.period) / info.sample_rate_hz.0;
    info.steps
        .iter()
        .zip(cv.rows())
        .filter(|(s, _)| (s.level.0 - level).abs() <= 0.5)
        .filter_map(|(s, r)| {
            let t = if recovery {
                r.recovery_0_5db
            } else {
                r.release_tau
            };
            let (y, bound) = match t {
                DynamicsTime::Seconds { value } => (value.0, None),
                DynamicsTime::WithinPeriod => (period, Some(WITHIN_PERIOD)),
                DynamicsTime::CantTell => (s.rest.0, Some(CANT_TELL)),
                DynamicsTime::NoGainReduction => return None,
            };
            Some(ReleasePoint {
                x: s.burst.0,
                y,
                bound,
            })
        })
        .collect()
}

fn time_axis(range: Range, lo: f32, hi: f32, title: &str) -> Axis {
    let mapping = Mapping::new(range, Scale::Log, lo, hi);
    let label_px = if hi > lo {
        axis::MIN_LABEL_PX_NUMBERS
    } else {
        axis::MIN_LABEL_PX_VERTICAL
    };
    Axis {
        ticks: axis::log_ticks(&mapping, label_px, axis::MIN_MINOR_PX, tick_text),
        mapping,
        title: title.to_string(),
    }
}

fn log_range(vals: impl Iterator<Item = f64>, fallback: (f64, f64)) -> Range {
    let (lo, hi) = vals
        .filter(|v| v.is_finite() && *v > 0.0)
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), v| {
            (l.min(v), h.max(v))
        });
    if lo.is_finite() {
        Range::new(lo / 2.0, hi * 2.0)
    } else {
        Range::new(fallback.0, fallback.1)
    }
}

/// The ramp of levels, quietest darkest: levels are an ordered magnitude.
fn ramp(i: usize, n: usize) -> Color {
    let lut = crate::colormap::lut(ac2_proto::model::Colormap::Viridis);
    let t = if n <= 1 {
        1.0
    } else {
        i as f64 / (n - 1) as f64
    };
    Color::from_rgba8(lut[(60.0 + t * 190.0) as usize])
}

#[allow(clippy::too_many_arguments)]
pub(super) fn release(
    c: &mut Canvas,
    out: &mut DynamicsScene,
    frame: &DynamicsFrame<'_>,
    ai: usize,
    p: DynamicsStepPick,
    area: Rect,
    style: Style<'_>,
) -> Vec<(String, Color)> {
    let theme = style.theme;
    let (top, bottom) = split(area);
    out.plots = vec![top, bottom];
    // (name, colour, release τ, recovery) per drawn series.
    let mut series: Vec<(String, Color, Vec<ReleasePoint>, Vec<ReleasePoint>)> = Vec::new();
    let a = &frame.curves[ai];
    if frame.modes.all_levels {
        let lv = levels(a.info());
        for (i, l) in lv.iter().enumerate() {
            series.push((
                level_text(*l),
                ramp(i, lv.len()),
                release_points(a, *l, false),
                release_points(a, *l, true),
            ));
        }
    } else {
        for cv in frame.curves {
            series.push((
                cv.name.clone(),
                cv.color,
                release_points(cv, p.level.0, false),
                release_points(cv, p.level.0, true),
            ));
        }
    }
    let xr = log_range(
        frame
            .curves
            .iter()
            .flat_map(|cv| cv.info().durations.iter().map(|d| d.0)),
        (1e-3, 10.0),
    );
    let yr = log_range(
        series
            .iter()
            .flat_map(|s| s.2.iter().chain(&s.3).map(|q| q.y)),
        (1e-3, 10.0),
    );
    let xa = time_axis(xr, top.x, top.right(), "burst");
    let xb = time_axis(xr, bottom.x, bottom.right(), "burst");
    let ya = time_axis(yr, top.bottom(), top.y, "s");
    let yb = time_axis(yr, bottom.bottom(), bottom.y, "s");
    canvas::pane_frame(
        c,
        top,
        &xa,
        &ya,
        true,
        metric_name(DynamicsMetric::ReleaseTau),
        style,
    );
    canvas::pane_frame(
        c,
        bottom,
        &xb,
        &yb,
        true,
        metric_name(DynamicsMetric::Recovery05Db),
        style,
    );
    for (name, color, tau, rec) in &series {
        for (pts, xm, ym, plot) in [
            (tau, &xa.mapping, &ya.mapping, top),
            (rec, &xb.mapping, &yb.mapping, bottom),
        ] {
            let line: Vec<[f32; 2]> = pts.iter().map(|q| [xm.to_px(q.x), ym.to_px(q.y)]).collect();
            c.data.polylines.push(Polyline::clipped(
                line.clone(),
                Stroke::solid(color.with_alpha(0.6), theme.trace_width),
                plot,
            ));
            let solid: Vec<Point> = pts
                .iter()
                .zip(&line)
                .filter(|(q, _)| q.bound.is_none())
                .map(|(_, xy)| Point {
                    pos: *xy,
                    color: *color,
                })
                .collect();
            c.data.points.push(Points {
                points: solid,
                size_px: 7.0,
                blend: crate::primitives::Blend::Alpha,
                clip: Some(plot),
            });
            for (_, xy) in pts.iter().zip(&line).filter(|(q, _)| q.bound.is_some()) {
                c.data.polylines.push(Polyline::clipped(
                    ring(*xy, 3.5),
                    Stroke::solid(*color, 1.2),
                    plot,
                ));
            }
        }
        out.legend.push(legend_row(a, name.clone(), *color));
    }
    // The readout: the hovered length (else the picked one), every series' two times.
    let len = frame
        .pointer
        .filter(|q| contains(top, *q) || contains(bottom, *q))
        .map(|q| xa.mapping.from_px(q[0]))
        .unwrap_or(p.burst.0);
    let lengths = lengths(a.info());
    let at = lengths
        .iter()
        .copied()
        .min_by(|x, y| (x / len).ln().abs().total_cmp(&(y / len).ln().abs()))
        .unwrap_or(p.burst.0);
    let mut lines = vec![(
        if frame.modes.all_levels {
            format!("{} · {}", a.name, length_text(at))
        } else {
            pick_text(DynamicsStepPick {
                level: p.level,
                burst: ac2_proto::units::Seconds(at),
            })
        },
        theme.text,
    )];
    for (name, color, tau, rec) in &series {
        let read = |v: &[ReleasePoint]| {
            v.iter().find(|q| (q.x / at).ln().abs() < 0.05).map_or_else(
                || NO_SUCH_STEP.to_string(),
                |q| q.bound.map_or_else(|| time_text(q.y), str::to_string),
            )
        };
        lines.push((
            format!("{name}  τ {} · recovery {}", read(tau), read(rec)),
            *color,
        ));
    }
    lines
}

/// A small circle: a hollow point.
fn ring(c: [f32; 2], r: f32) -> Vec<[f32; 2]> {
    (0..=16)
        .map(|k| {
            let a = k as f32 * std::f32::consts::TAU / 16.0;
            [c[0] + r * a.cos(), c[1] + r * a.sin()]
        })
        .collect()
}
