//! A spectrum pane's level axis following what the pane shows, gently: up fast, because a
//! clipped curve is wrong now; down slowly, because a low one is only less legible and a
//! song gap or a quiet verse is no new level; wider when the content no longer fits, never
//! narrower (a fit narrows). Pure: the app feeds each frame's [`Sample`] with its arrival
//! time and asks [`Follow::decide`] when the pane may move; the move eases ([`Ease`]).
//!
//! The constants are provisional (`docs/design/level-autocentre.md`, *Validation*).

use std::collections::VecDeque;

use super::{level, nice};
use crate::axis::Range;

/// Lowest column a sample counts, Hz: the frequency axis's limit, not the shown zoom. The
/// frequency axis is shared across panes; zooming one must not move levels.
pub const BAND_LO_HZ: f64 = 20.0;
/// Highest column a sample counts, Hz.
pub const BAND_HI_HZ: f64 = 20_000.0;
/// The window the up rule reads, s: short, so a clipped curve is put right within seconds.
pub const UP_WINDOW_S: f64 = 3.0;
/// The up rule: the window's maximum within this share of the span under the top. A peak
/// that close is about to leave the plot, and a peak over the top is a curve drawn wrong:
/// the axis moves before it clips, not after most frames have.
pub const UP_AT: f64 = 0.1;
/// The window the down and widen rules read, s: longer than any song gap, speech break or
/// quiet verse, so only a lasting change (an idle mic, a gain change) moves the axis down.
pub const SLOW_WINDOW_S: f64 = 240.0;
/// The least time between a move and a down or widen move, s.
pub const LOCKOUT_S: f64 = 60.0;
/// Where a move puts the window's maximum, and the least room a fit leaves above the
/// content's: this share of the span under the top, so the next louder passage still fits
/// and does not reach [`UP_AT`] at once.
pub const TOP_PAD: f64 = 0.2;
/// The down rule: the window's maximum under this share of the span above the bottom.
pub const SQUASH: f64 = 0.35;
/// The widen rule: the median frame's extent (top minus floor) over this share of the span.
pub const WIDEN_AT: f64 = 0.8;
/// A widened span holds the content's extent in at most this share, so it does not
/// re-trigger at once.
pub const WIDEN_TO: f64 = 0.6;
/// The longest gap between frames a window may hold, s: a multi-resolution spectrum or a
/// slow RTA sends a few frames a second, so coverage is by time, and a stalled stream counts
/// no time.
pub const MAX_GAP_S: f64 = 2.0;
/// The quantile of a frame's columns taken as its floor.
pub const FLOOR_QUANTILE: f64 = 0.05;
/// How long a move eases, s.
pub const EASE_S: f64 = 0.8;
/// How long the gutter's shade fades after a move, s.
pub const SHADE_S: f64 = 1.5;
/// No move this long after a key or pointer input on the pane, s: the operator is working
/// on it.
pub const HANDS_OFF_S: f64 = 30.0;

/// One frame of a pane, as the follower reads it: its highest column (what a fit frames to
/// at its top) and its floor ([`FLOOR_QUANTILE`] of its columns), dB as drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub top: f64,
    pub floor: f64,
}

impl Sample {
    /// The sample of a pane drawing `curves`, each its column frequencies and levels as drawn
    /// (offset included; NaN where a column has no valid value): the highest column between
    /// [`BAND_LO_HZ`] and [`BAND_HI_HZ`] of any curve, each on its own grid, and the floor of
    /// all of them. No minimum column count: a 1/1-octave RTA has ten. `None` without a
    /// finite column.
    pub fn of<'a>(curves: impl IntoIterator<Item = (&'a [f64], &'a [f64])>) -> Option<Self> {
        let mut v: Vec<f64> = curves
            .into_iter()
            .flat_map(|(f, l)| f.iter().zip(l))
            .filter(|(f, l)| (BAND_LO_HZ..=BAND_HI_HZ).contains(*f) && l.is_finite())
            .map(|(_, l)| *l)
            .collect();
        if v.is_empty() {
            return None;
        }
        v.sort_by(f64::total_cmp);
        Some(Self {
            top: v[v.len() - 1],
            floor: quantile_sorted(&v, FLOOR_QUANTILE),
        })
    }

    /// Two panes on one range read as one: the higher top, the lower floor.
    #[must_use]
    pub fn with(self, other: Self) -> Self {
        Self {
            top: self.top.max(other.top),
            floor: self.floor.min(other.floor),
        }
    }
}

/// Why the follower moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Up,
    Down,
    Widen,
}

/// A move the follower decided: the range to ease to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Move {
    pub to: Range,
    pub why: Why,
}

/// The frames of one followed range, by arrival time, and when it last moved.
#[derive(Clone, Debug, Default)]
pub struct Follow {
    ring: VecDeque<(f64, Sample)>,
    /// The first frame of the run of frames up to the newest, none more than [`MAX_GAP_S`]
    /// apart.
    run_start: Option<f64>,
    moved_at: Option<f64>,
    /// When the slow rules last looked: they read minutes, so once a second is enough.
    slow_at: Option<f64>,
    /// The values a rule takes a quantile of, kept between ticks: a decision runs every
    /// frame drawn, and its window holds minutes of frames.
    scratch: Vec<f64>,
}

impl Follow {
    /// Frame `s` arrived at `t` (s, the app's clock). An older time than the last is dropped.
    pub fn push(&mut self, t: f64, s: Sample) {
        if !(t.is_finite() && s.top.is_finite() && s.floor.is_finite()) {
            return;
        }
        match self.ring.back() {
            Some((last, _)) if t < *last => return,
            Some((last, _)) if t - *last <= MAX_GAP_S => {}
            _ => self.run_start = Some(t),
        }
        self.ring.push_back((t, s));
        let keep = t - SLOW_WINDOW_S - MAX_GAP_S;
        while self.ring.front().is_some_and(|(at, _)| *at < keep) {
            self.ring.pop_front();
        }
    }

    /// A new signal history (a restarted daemon, a new run): nothing heard so far counts.
    pub fn clear(&mut self) {
        self.ring.clear();
        self.run_start = None;
    }

    /// The range was framed (a fit) or moved at `t`: the up window starts again and the
    /// slow rules wait out their lockout.
    pub fn moved(&mut self, t: f64) {
        self.moved_at = Some(t);
    }

    /// No frame kept.
    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }

    /// How many of the newest frames are the last `w` seconds up to the newest, when they
    /// cover it: the newest no older than [`MAX_GAP_S`] at `now`, none more than that apart,
    /// the run reaching back `w` from no earlier than `after`. A stalled stream counts no
    /// time: the window ends on its newest frame. `None` when they do not cover it: the
    /// decision waits.
    fn window(&self, now: f64, w: f64, after: Option<f64>) -> Option<usize> {
        // Frame times are sums of float steps: a window that ends on a frame covers it.
        const EPS: f64 = 1e-9;
        let (last, _) = *self.ring.back()?;
        let start = self.run_start?.max(after.unwrap_or(f64::NEG_INFINITY));
        let from = last - w;
        if now - last > MAX_GAP_S || start > from + EPS {
            return None;
        }
        Some(
            self.ring
                .iter()
                .rev()
                .take_while(|(t, _)| *t >= from - EPS)
                .count(),
        )
    }

    /// The newest `n` frames, newest first.
    fn newest(&self, n: usize) -> impl Iterator<Item = Sample> + '_ {
        self.ring.iter().rev().take(n).map(|(_, s)| *s)
    }

    /// The `q` quantile of `f` over the newest `n` frames.
    fn quantile_of(&mut self, n: usize, q: f64, f: impl Fn(Sample) -> f64) -> f64 {
        let mut v = std::mem::take(&mut self.scratch);
        v.clear();
        v.extend(self.newest(n).map(f));
        let x = quantile(&mut v, q);
        self.scratch = v;
        x
    }

    /// Whether range `r` should move at `now`, and where to; a move counts as made (call it
    /// only when the range may move: not while the operator works on the pane, nor mid-ease).
    pub fn decide(&mut self, now: f64, r: Range) -> Option<Move> {
        if !r.is_valid() {
            return None;
        }
        let slow_due = self.slow_at.is_none_or(|t| now - t >= 1.0 || now < t);
        let m = match self.up(now, r) {
            Some(m) => m,
            None if slow_due => {
                self.slow_at = Some(now);
                self.slow(now, r)?
            }
            None => return None,
        };
        if m.to == r {
            return None;
        }
        self.moved_at = Some(now);
        Some(m)
    }

    fn up(&mut self, now: f64, r: Range) -> Option<Move> {
        let n = self.window(now, UP_WINDOW_S, self.moved_at)?;
        let max = self
            .newest(n)
            .map(|s| s.top)
            .fold(f64::NEG_INFINITY, f64::max);
        if max <= r.hi - UP_AT * r.span() {
            return None;
        }
        Some(Move {
            to: shift(r, max, false),
            why: Why::Up,
        })
    }

    fn slow(&mut self, now: f64, r: Range) -> Option<Move> {
        if self.moved_at.is_some_and(|t| now - t < LOCKOUT_S) {
            return None;
        }
        let n = self.window(now, SLOW_WINDOW_S, None)?;
        let max = self
            .newest(n)
            .map(|s| s.top)
            .fold(f64::NEG_INFINITY, f64::max);
        if self.quantile_of(n, 0.5, |s| s.top - s.floor) > WIDEN_AT * r.span() {
            let to = headroom(widened(self.quantile_of(n, 0.5, |s| s.floor), max)?, max);
            // The follower never narrows: an idle mic's floor is no reason to zoom in.
            if to.span() > r.span() {
                return Some(Move {
                    to,
                    why: Why::Widen,
                });
            }
            return None;
        }
        if max < r.lo + SQUASH * r.span() {
            return Some(Move {
                to: shift(r, max, true),
                why: Why::Down,
            });
        }
        None
    }
}

/// `r` moved by whole grid steps (`nice(span / 8)`, as [`level::fit`] rounds) so that `max`
/// sits at [`TOP_PAD`] under the top or lower: the fewest steps that do (at least one, up or
/// for a move down, `down`, down). The labels end on the same grid lines; only the numbers
/// change.
fn shift(r: Range, max: f64, down: bool) -> Range {
    // A max a hair over the place is on it.
    const EPS: f64 = 1e-9;
    let step = nice(r.span() / 8.0);
    let d = (max - (r.hi - TOP_PAD * r.span())) / step;
    let n = (d - EPS).ceil();
    let n = match (n, down) {
        (n, false) if n < 1.0 => 1.0,
        (n, true) if n > -1.0 => -1.0,
        (n, _) => n,
    };
    level::pan(r, n * step)
}

/// A spectrum pane's fit: [`level::fit`] of `values` with [`headroom`] over their maximum,
/// so a fit leaves the room above the content a follow move leaves and the next louder
/// passage does not move the axis at once. `None` without a finite value.
pub fn fit(values: impl IntoIterator<Item = f64>) -> Option<Range> {
    let v: Vec<f64> = values.into_iter().filter(|x| x.is_finite()).collect();
    let r = level::fit(v.iter().copied())?;
    let max = v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Some(headroom(r, max))
}

/// `r` raised at its top by whole steps of its own grid until `max` sits at [`TOP_PAD`] of
/// the span under the top or lower; its edges stay on the grid `nice(span / 8)` of the span
/// it ends with, which [`shift`] and the labels step by.
fn headroom(mut r: Range, max: f64) -> Range {
    // Edges are sums of float steps: one a hair off a grid line is on it.
    const EPS: f64 = 1e-9;
    // Each round either aligns the edges or adds a step: a few rounds end it.
    for _ in 0..32 {
        let step = nice(r.span() / 8.0);
        let on = Range::new(
            (r.lo / step + EPS).floor() * step,
            (r.hi / step - EPS).ceil() * step,
        );
        if (on.lo - r.lo).abs() > EPS * step || (on.hi - r.hi).abs() > EPS * step {
            r = on;
            continue;
        }
        if max <= r.hi - TOP_PAD * r.span() + EPS * step {
            break;
        }
        r = Range::new(r.lo, r.hi + step);
    }
    level::clamp(r)
}

/// The range that frames content from `floor` to `top` with that extent filling at most
/// [`WIDEN_TO`] of it: [`level::fit`] of the two, widened downward. Its edges lie on its own
/// grid, `nice(span / 8)`, which [`shift`] and the axis labels step by: a span rounded out to
/// whole steps can reach the next 1-2-5 step's range, so the step is chosen with the span.
fn widened(floor: f64, top: f64) -> Option<Range> {
    // Edges are sums of float steps: one a hair off a grid line is on it.
    const EPS: f64 = 1e-9;
    let fit = level::fit([floor, top])?;
    let extent = top - floor;
    let wanted = fit.span().max(extent / WIDEN_TO);
    let same = |a: f64, b: f64| (a / b - 1.0).abs() < EPS;
    let mut step = nice(wanted / 8.0);
    // Each round only a coarser step: the rounds end within a few.
    for _ in 0..8 {
        let hi = (fit.hi / step - EPS).ceil() * step;
        // The bottom edge holds the fit's bottom and the wanted span under the top.
        let need = fit.lo.min(hi - wanted);
        let mut n = ((hi - (need / step + EPS).floor() * step) / step).round();
        if n > 8.0 && hi - 8.0 * step <= need + EPS {
            n = 8.0;
        }
        while nice(n * step / 8.0) < step * (1.0 - EPS) {
            n += 1.0;
        }
        let own = nice(n * step / 8.0);
        if same(own, step) {
            return Some(level::clamp(Range::new(hi - n * step, hi)));
        }
        step = own;
    }
    Some(fit)
}

/// The `q` quantile of ascending `v` (non-empty), the nearest rank.
fn quantile_sorted(v: &[f64], q: f64) -> f64 {
    let i = ((v.len() - 1) as f64 * q).round() as usize;
    v[i.min(v.len() - 1)]
}

/// The `q` quantile of `v` (non-empty, reordered), the nearest rank.
fn quantile(v: &mut [f64], q: f64) -> f64 {
    let i = (((v.len() - 1) as f64 * q).round() as usize).min(v.len() - 1);
    *v.select_nth_unstable_by(i, f64::total_cmp).1
}

/// A move under way: cosine in-out over [`EASE_S`] on the range itself, so curves, grid,
/// spectrograph colours and 3D heights move together on one consistent range each frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ease {
    pub from: Range,
    pub to: Range,
    pub start_s: f64,
}

impl Ease {
    fn progress(&self, now: f64) -> f64 {
        let u = ((now - self.start_s) / EASE_S).clamp(0.0, 1.0);
        (1.0 - (std::f64::consts::PI * u).cos()) / 2.0
    }

    /// The range at `now`: `to` once done.
    pub fn at(&self, now: f64) -> Range {
        if self.done(now) {
            return self.to;
        }
        let w = self.progress(now);
        Range::new(
            self.from.lo + (self.to.lo - self.from.lo) * w,
            self.from.hi + (self.to.hi - self.from.hi) * w,
        )
    }

    /// The ease has arrived.
    pub fn done(&self, now: f64) -> bool {
        now - self.start_s >= EASE_S
    }

    /// The level gutter's shade, 0 to 1: full while easing, fading over [`SHADE_S`] after.
    /// The axis numbers are the truth and they change; the shade only says they just did.
    pub fn shade(&self, now: f64) -> f32 {
        let after = now - self.start_s - EASE_S;
        if now < self.start_s {
            0.0
        } else if after <= 0.0 {
            1.0
        } else {
            (1.0 - after / SHADE_S).clamp(0.0, 1.0) as f32
        }
    }
}

#[cfg(test)]
mod tests;
