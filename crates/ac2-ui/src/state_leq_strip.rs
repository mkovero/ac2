//! The Leq history strip looked back at: per SPL pane, a span of the meter's log that the
//! mouse and the navigation keys move (the wheel zooms time about the pointer, a drag pans,
//! I / O zoom, ← / → pan, Home returns to live), the spans of each meter's log fetched at
//! the strip's resolution (`spl.history_get` with a range), and the pointer's time for the
//! cursor readout. The values are the daemon's: this only picks which span to ask for.

use super::*;
use ac2_proto::model::{LeqConfig, SplHistory};
use ac2_proto::units::WallNs;
use ac2_scene::leq::{LeqHistory, LeqStripView, StripSpan};

/// Shortest span a strip shows, s.
pub const STRIP_MIN_S: f64 = 30.0;
/// Fetched spans kept per meter: a few zoom levels to go back to without asking again.
const SPANS_KEPT: usize = 4;
/// Seconds per point a fetch asks for: steps the clock's minutes and hours divide into, so
/// buckets stay put as the strip pans.
const STEPS: [u32; 16] = [
    1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 14_400, 28_800,
];
/// Widest strip a fetch's resolution follows, px: the view at its step stays well within
/// a reply's [`SplHistory::MAX_POINTS`], leaving room for a span either side.
const RESOLUTION_PX: f32 = 1200.0;

/// A span of a meter's log fetched at a resolution.
#[derive(Clone, Debug)]
pub struct LeqSpan {
    /// Daemon wall clock, s.
    pub from: f64,
    pub to: f64,
    /// Seconds per point.
    pub step: u32,
    pub points: LeqHistory,
}

impl LeqSpan {
    /// Whether it holds `[from, to]` (each end within a point's time).
    fn covers(&self, from: f64, to: f64) -> bool {
        let slack = f64::from(self.step);
        self.from <= from + slack && self.to >= to - slack
    }
}

/// A fetch under way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SpanAsk {
    meas: MeasId,
    from: f64,
    to: f64,
    step: u32,
    ask: u64,
}

/// The mouse on an SPL pane's history strip. `shown` is the span the strip was drawn with
/// (daemon wall clock, s) and `width_px` its plot's width: the reducer moves from what the
/// operator saw, at what the screen resolves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StripMsg {
    /// Wheel: time zooms by `factor` (> 1 in) about `about` (s).
    Zoom {
        shown: (f64, f64),
        about: f64,
        factor: f64,
        width_px: f32,
    },
    /// Drag: the span moves by `by` s (positive: later).
    Pan {
        shown: (f64, f64),
        by: f64,
        width_px: f32,
    },
    /// The pointer over the strip at a time (s), or off it.
    Hover(Option<f64>),
}

/// The commands that move a strip when its pane is focused.
pub(super) fn is_strip_nav(c: CommandId) -> bool {
    use CommandId as C;
    matches!(
        c,
        C::ZoomIn | C::ZoomOut | C::PanLeft | C::PanRight | C::ResetView
    )
}

/// The largest step of [`STEPS`] at or under `raw` s a point (at least 1 s).
fn step_for(raw: f64) -> u32 {
    STEPS
        .iter()
        .copied()
        .take_while(|s| f64::from(*s) <= raw)
        .last()
        .unwrap_or(1)
}

impl AppState {
    /// The SPL meter pane `pane`'s Leq view shows and its windows.
    pub(crate) fn strip_meter(&self, pane: PaneId) -> Option<(MeasId, LeqConfig)> {
        let id = crate::scenes::leq_meter(self, pane)?;
        self.measurements()
            .iter()
            .find(|m| m.id == id)
            .and_then(|m| match &m.config.kind {
                MeasKind::Spl { config } => Some((id, config.leq.clone())),
                _ => None,
            })
    }

    /// The span a live strip of meter `meas` shows: its windows' span up to the newest
    /// second received.
    fn strip_live_span(&self, meas: MeasId, cfg: &LeqConfig) -> Option<(f64, f64)> {
        let newest = self.leq.history.get(&meas)?.1.newest()?;
        Some((newest - ac2_scene::leq::history_span(cfg), newest))
    }

    /// The meter pane `pane`'s strip shows and the span it shows.
    pub(crate) fn strip_shown(&self, pane: PaneId) -> Option<(MeasId, LeqConfig, (f64, f64))> {
        let (meas, cfg) = self.strip_meter(pane)?;
        let span = match self.leq.strips.get(&pane) {
            Some(s) => *s,
            None => self.strip_live_span(meas, &cfg)?,
        };
        Some((meas, cfg, span))
    }

    /// The whole log of meter `meas`: its oldest kept second's start to its newest's end
    /// (the `leq` frame's run), else what was received.
    fn log_extent(&self, meas: MeasId) -> Option<(f64, f64)> {
        if let Some(r) = self.leq_run(meas) {
            return Some((r.started_at.0 as f64 / 1e9, r.until.0 as f64 / 1e9));
        }
        let h = &self.leq.history.get(&meas)?.1;
        Some((h.oldest()?, h.newest()?))
    }

    /// The focused pane, when it shows a history strip with something in it.
    pub(super) fn leq_strip_target(&self) -> Option<PaneId> {
        let pane = self.layout.focus;
        if self.layout.kind(pane) != PaneKind::Spl || !self.view.spl.layout.history {
            return None;
        }
        if !matches!(self.pane_modes(pane).spl, SplMode::Leq | SplMode::MeterLeq) {
            return None;
        }
        self.strip_shown(pane).map(|_| pane)
    }

    /// A navigation key ([`is_strip_nav`]) on the focused pane's strip.
    pub(super) fn leq_strip_key(&mut self, pane: PaneId, c: CommandId, out: &mut Vec<Request>) {
        use CommandId as C;
        let Some((meas, cfg, (from, to))) = self.strip_shown(pane) else {
            return;
        };
        let span = to - from;
        let px = self.leq.strip_px;
        match c {
            C::ZoomIn | C::ZoomOut => {
                // About the cursor while it is on the strip, else the middle.
                let about = self
                    .leq
                    .cursor
                    .filter(|(p, t)| *p == pane && *t > from && *t < to)
                    .map_or((from + to) / 2.0, |(_, t)| t);
                let f = if c == C::ZoomIn {
                    ZOOM_FACTOR
                } else {
                    1.0 / ZOOM_FACTOR
                };
                self.zoom_strip(pane, meas, &cfg, (from, to), about, f, px, out);
            }
            C::PanLeft => self.move_strip(
                pane,
                meas,
                &cfg,
                from - span / 10.0,
                to - span / 10.0,
                px,
                out,
            ),
            C::PanRight => self.move_strip(
                pane,
                meas,
                &cfg,
                from + span / 10.0,
                to + span / 10.0,
                px,
                out,
            ),
            C::ResetView if self.leq.strips.remove(&pane).is_some() => {
                self.toast("Leq history: live");
            }
            _ => {}
        }
    }

    /// The mouse on pane `pane`'s strip.
    pub(super) fn leq_strip_msg(&mut self, pane: PaneId, m: StripMsg, out: &mut Vec<Request>) {
        match m {
            StripMsg::Hover(t) => {
                self.leq.cursor = t.map(|t| (pane, t));
            }
            StripMsg::Zoom {
                shown,
                about,
                factor,
                width_px,
            } => {
                if let Some((meas, cfg)) = self.strip_meter(pane) {
                    self.zoom_strip(pane, meas, &cfg, shown, about, factor, width_px, out);
                }
            }
            StripMsg::Pan {
                shown,
                by,
                width_px,
            } => {
                if let Some((meas, cfg)) = self.strip_meter(pane) {
                    self.move_strip(pane, meas, &cfg, shown.0 + by, shown.1 + by, width_px, out);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn zoom_strip(
        &mut self,
        pane: PaneId,
        meas: MeasId,
        cfg: &LeqConfig,
        (from, to): (f64, f64),
        about: f64,
        factor: f64,
        width_px: f32,
        out: &mut Vec<Request>,
    ) {
        if !(factor.is_finite() && factor > 0.0) {
            return;
        }
        let about = about.clamp(from, to);
        let span = ((to - from) / factor).max(STRIP_MIN_S);
        let k = (about - from) / (to - from).max(1e-9);
        let from = about - k * span;
        self.move_strip(pane, meas, cfg, from, from + span, width_px, out);
    }

    /// Pane `pane`'s strip looks at `[from, to]`, kept within the log (a span no wider
    /// than the log or the live span, none of it after the newest second) and at least
    /// [`STRIP_MIN_S`]; then the span is fetched if what is held does not cover it.
    #[allow(clippy::too_many_arguments)]
    fn move_strip(
        &mut self,
        pane: PaneId,
        meas: MeasId,
        cfg: &LeqConfig,
        from: f64,
        to: f64,
        width_px: f32,
        out: &mut Vec<Request>,
    ) {
        if !(from.is_finite() && to.is_finite()) {
            return;
        }
        let Some((lo, hi)) = self.log_extent(meas) else {
            return;
        };
        let widest = (hi - lo).max(ac2_scene::leq::history_span(cfg));
        let span = (to - from).clamp(STRIP_MIN_S, widest * 1.05);
        // A little room before the log's first second shows where it starts.
        let to = to.min(hi).max(lo - 0.05 * span + span);
        self.leq.strips.insert(pane, (to - span, to));
        if width_px > 1.0 {
            self.leq.strip_px = width_px;
        }
        self.want_span(pane, meas, out);
    }

    /// The step pane `pane`'s strip wants: about a point per pixel, on [`STEPS`].
    fn strip_step(&self, from: f64, to: f64) -> u32 {
        let px = f64::from(self.leq.strip_px.clamp(100.0, RESOLUTION_PX));
        step_for((to - from) / px)
    }

    /// What pane `pane`'s strip looking back needs of meter `meas`'s log, held: the span
    /// within the log, by the seconds received (a point a second, the newest hours) or by
    /// a span fetched at least as finely as the strip resolves.
    fn strip_need(&self, pane: PaneId, meas: MeasId) -> Option<(f64, f64, u32)> {
        let (from, to) = *self.leq.strips.get(&pane)?;
        let (lo, hi) = self.log_extent(meas)?;
        let (from, to) = (from.max(lo), to.min(hi));
        if to <= from {
            return None;
        }
        Some((from, to, self.strip_step(from, to)))
    }

    /// The fetched span that draws `[from, to]` at `step` or finer, the finest first.
    fn covering_span(&self, meas: MeasId, from: f64, to: f64, step: u32) -> Option<&LeqSpan> {
        self.leq
            .spans
            .get(&meas)?
            .iter()
            .filter(|s| s.step <= step && s.covers(from, to))
            .min_by_key(|s| s.step)
    }

    /// Whether the seconds received hold `[from, ...]` of meter `meas`.
    fn live_covers(&self, meas: MeasId, from: f64) -> bool {
        self.leq
            .history
            .get(&meas)
            .and_then(|(_, h)| h.oldest())
            .is_some_and(|o| o <= from + 1.0)
    }

    /// Asks for the span pane `pane`'s strip shows, at its resolution, with a span either
    /// side for panning, unless it is held or already asked for.
    fn want_span(&mut self, pane: PaneId, meas: MeasId, out: &mut Vec<Request>) {
        let Some((from, to, step)) = self.strip_need(pane, meas) else {
            return;
        };
        if self.live_covers(meas, from) || self.covering_span(meas, from, to, step).is_some() {
            return;
        }
        if let Some(p) = self.leq.span_pending
            && p.meas == meas
            && p.step <= step
            && p.from <= from + f64::from(p.step)
            && p.to >= to - f64::from(p.step)
        {
            return;
        }
        let Some((lo, hi)) = self.log_extent(meas) else {
            return;
        };
        let s = f64::from(step);
        let room = (f64::from(SplHistory::MAX_POINTS - 2) * s - (to - from)).max(0.0) / 2.0;
        let margin = (to - from).min(room);
        let a = ((from - margin).max(lo - s) / s).floor() * s;
        let b = ((to + margin).min(hi + s) / s).ceil() * s;
        self.leq.span_ask += 1;
        let ask = self.leq.span_ask;
        self.leq.span_pending = Some(SpanAsk {
            meas,
            from: a,
            to: b,
            step,
            ask,
        });
        out.push(Request::LeqSpan {
            meas,
            from: WallNs((a.max(0.0) * 1e9) as u64),
            to: WallNs((b.max(0.0) * 1e9) as u64),
            step_s: step,
            ask,
        });
    }

    /// A span fetched: kept (the oldest beyond [`SPANS_KEPT`] dropped); an error is said.
    pub(super) fn leq_span_fetched(
        &mut self,
        meas: MeasId,
        ask: u64,
        result: Result<Box<SplHistory>, String>,
    ) {
        let asked = self.leq.span_pending.filter(|p| p.ask == ask);
        if asked.is_some() {
            self.leq.span_pending = None;
        }
        match result {
            Ok(h) => {
                let Some(a) = asked else {
                    return;
                };
                let spans = self.leq.spans.entry(meas).or_default();
                spans.push(LeqSpan {
                    from: a.from,
                    to: a.to,
                    step: h.step_s.max(1),
                    points: LeqHistory::from_span(&h),
                });
                let over = spans.len().saturating_sub(SPANS_KEPT);
                spans.drain(..over);
            }
            Err(e) if asked.is_some() => {
                self.fault(format!("Leq history: {e}"));
            }
            Err(_) => {}
        }
    }

    /// A new log of meter `meas` (or new windows): its fetched spans are of the old one,
    /// and the strips on it go back to live.
    pub(super) fn forget_leq_spans(&mut self, meas: MeasId) {
        self.leq.spans.remove(&meas);
        if self.leq.span_pending.is_some_and(|p| p.meas == meas) {
            self.leq.span_pending = None;
        }
        let panes: Vec<PaneId> = self
            .leq
            .strips
            .keys()
            .copied()
            .filter(|p| self.strip_meter(*p).is_some_and(|(m, _)| m == meas))
            .collect();
        for p in panes {
            self.leq.strips.remove(&p);
        }
    }

    /// How pane `pane`'s strip of meter `meas` is drawn: live or the span it looks at, the
    /// points that cover it, the pointer. `live_key` names the key back to live.
    pub(crate) fn strip_view(
        &self,
        pane: PaneId,
        meas: MeasId,
        live_key: Option<String>,
    ) -> LeqStripView<'_> {
        let offset = |t: f64| self.local_zone.offset_s(WallNs((t.max(0.0) * 1e9) as u64));
        let cursor = self.leq.cursor.filter(|(p, _)| *p == pane).map(|(_, t)| t);
        let Some(&(from, to)) = self.leq.strips.get(&pane) else {
            let newest = self.leq.history.get(&meas).and_then(|(_, h)| h.newest());
            return LeqStripView {
                span: StripSpan {
                    span: None,
                    utc_offset_s: offset(newest.unwrap_or(0.0)),
                },
                cursor,
                ..LeqStripView::default()
            };
        };
        let span = StripSpan {
            span: Some((from, to)),
            utc_offset_s: offset((from + to) / 2.0),
        };
        let need = self.strip_need(pane, meas);
        let (points, loading) = match need {
            Some((a, _, _)) if self.live_covers(meas, a) => (None, false),
            Some((a, b, step)) => match self.covering_span(meas, a, b, step) {
                Some(s) => (Some(&s.points), false),
                None => {
                    // Meanwhile the newest span fetched that reaches into it, if any.
                    let near = self
                        .leq
                        .spans
                        .get(&meas)
                        .and_then(|v| v.iter().rev().find(|s| s.from < b && s.to > a));
                    (near.map(|s| &s.points), self.leq.span_pending.is_some())
                }
            },
            None => (None, false),
        };
        LeqStripView {
            span,
            points,
            loading,
            cursor,
            live_key,
        }
    }
}
