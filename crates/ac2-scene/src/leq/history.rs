//! The history strip: each window's value against its limit over the last hours.

use super::*;

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
#[derive(Clone, Debug, Default)]
pub struct LeqHistory {
    series: Vec<((u64, Weighting), VecDeque<HistoryPoint>)>,
    scale: Option<LevelScale>,
    /// The columns' scale as of the newest frame ([`column_range`] keeps it while the
    /// levels allow).
    range: Option<Range>,
    /// Seconds per point: 1 for what was received, more for a span of the log fetched in
    /// buckets ([`LeqHistory::from_span`]); 0 reads as 1.
    step: u32,
    /// Bumped by every change of the points, so a laid-out strip is reused until the next
    /// frame arrives however often the view is rebuilt.
    generation: u64,
    strip: StripCache,
}

impl PartialEq for LeqHistory {
    fn eq(&self, o: &Self) -> bool {
        self.series == o.series && self.scale == o.scale && self.range == o.range
    }
}

/// What a laid-out strip was made from.
#[derive(Clone, Debug, PartialEq)]
struct StripKey {
    generation: u64,
    span: Option<(f64, f64)>,
    utc_offset_s: i32,
    windows: Vec<LeqWindow>,
    judged: bool,
    plot: Rect,
    theme: Theme,
}

/// The last strip laid out from a history. A clone starts empty: it belongs to the
/// instance whose points it was made from.
#[derive(Default)]
struct StripCache(std::sync::Mutex<Option<(StripKey, HistoryStrip)>>);

impl Clone for StripCache {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl std::fmt::Debug for StripCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StripCache")
    }
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
        self.generation += 1;
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
            let p = HistoryPoint {
                t,
                leq: f.leq[i],
                over: f.flags[i].contains(LeqFlags::OVER),
            };
            // Frames come a second apart: one within half a second of the newest point is
            // of the same second (that point rebuilt from the log, stamped at the second's
            // end, or a frame sent again after a change of windows).
            match s.back_mut() {
                Some(b) if t - b.t < 0.5 => *b = p,
                _ => s.push_back(p),
            }
            while s.front().is_some_and(|p| p.t < t - HISTORY_S) {
                s.pop_front();
            }
        }
    }

    /// Forgets every point (a new log).
    pub fn clear(&mut self) {
        self.generation += 1;
        self.series.clear();
    }

    /// Puts the history the daemon rebuilt from the meter's log (`spl.history_get`) under
    /// what was received live: each of its windows' points replace the series up to the
    /// newest of them, and the frames received after that second continue it. Ignored when
    /// the frames received since are in another unit (the meter's calibration changed
    /// meanwhile).
    pub fn backfill(&mut self, h: &SplHistory) {
        let Some(newest) = h.at.last().map(|t| t.0 as f64 / 1e9) else {
            return;
        };
        if self.scale.is_some_and(|s| s != h.scale) && !self.is_empty() {
            return;
        }
        self.scale = Some(h.scale);
        self.generation += 1;
        for (i, w) in h.windows.iter().enumerate() {
            let (Some(leq), Some(over)) = (h.leq.get(i), h.over.get(i)) else {
                continue;
            };
            let k = key(w);
            let live: Vec<HistoryPoint> = match self.series.iter().find(|(sk, _)| *sk == k) {
                // A frame is stamped a little after the end of its second: one within half
                // a second of the newest rebuilt point is that point.
                Some((_, s)) => s.iter().filter(|p| p.t > newest + 0.5).copied().collect(),
                None => Vec::new(),
            };
            let rebuilt =
                h.at.iter()
                    .zip(leq.iter().zip(over))
                    .map(|(t, (l, o))| HistoryPoint {
                        t: t.0 as f64 / 1e9,
                        leq: *l,
                        over: *o,
                    });
            let mut s: VecDeque<HistoryPoint> = rebuilt.chain(live).collect();
            if let Some(n) = s.back().map(|p| p.t) {
                while s.front().is_some_and(|p| p.t < n - HISTORY_S) {
                    s.pop_front();
                }
            }
            match self.series.iter_mut().find(|(sk, _)| *sk == k) {
                Some((_, old)) => *old = s,
                None => self.series.push((k, s)),
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

    /// Oldest time of any point.
    pub fn oldest(&self) -> Option<f64> {
        self.series
            .iter()
            .filter_map(|(_, s)| s.front().map(|p| p.t))
            .fold(None, |a: Option<f64>, t| Some(a.map_or(t, |a| a.min(t))))
    }

    /// Seconds per point.
    pub fn step(&self) -> u32 {
        self.step.max(1)
    }

    /// A span of the log as the daemon computed it (`spl.history_get` with a range), every
    /// point kept: what a strip looking back draws. Each point stands for the `step_s`
    /// seconds up to it.
    pub fn from_span(h: &SplHistory) -> Self {
        let series = h
            .windows
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let pts =
                    h.at.iter()
                        .zip(h.leq.get(i).into_iter().flatten())
                        .zip(h.over.get(i).into_iter().flatten())
                        .map(|((t, l), o)| HistoryPoint {
                            t: t.0 as f64 / 1e9,
                            leq: *l,
                            over: *o,
                        })
                        .collect();
                (key(w), pts)
            })
            .collect();
        Self {
            series,
            scale: Some(h.scale),
            range: None,
            step: h.step_s.max(1),
            generation: 1,
            strip: StripCache::default(),
        }
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
    /// The time shown, daemon wall clock s: from the plot's left edge to its right.
    pub from: f64,
    pub to: f64,
    pub x: Axis,
    pub y: Axis,
    pub lines: Vec<HistoryLine>,
}

/// Lays the history out in `plot`. The strip is kept with the history and laid out again
/// only when a frame arrives or the inputs change.
pub fn history_strip(
    cfg: &LeqConfig,
    h: &LeqHistory,
    judged: bool,
    plot: Rect,
    theme: &Theme,
    look: StripSpan,
) -> HistoryStrip {
    let key = StripKey {
        generation: h.generation,
        span: look.span,
        utc_offset_s: look.utc_offset_s,
        windows: cfg.windows.clone(),
        judged,
        plot,
        theme: theme.clone(),
    };
    let mut cache = h.strip.0.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((k, s)) = cache.as_ref()
        && *k == key
    {
        return s.clone();
    }
    let s = lay_out_strip(cfg, h, judged, plot, theme, look);
    *cache = Some((key, s.clone()));
    s
}

/// The points of `ps` from time `from` on (they are in time order).
fn since(ps: &VecDeque<HistoryPoint>, from: f64) -> impl Iterator<Item = &HistoryPoint> {
    ps.range(ps.partition_point(|p| p.t < from)..)
}

/// `points` (NaN breaks between runs) thinned to what a stroke wider than a pixel shows:
/// within each pixel column of a run only its lowest and highest point, in time order, plus
/// each run's ends. The line still reaches every peak and dip and every column's whole
/// vertical extent, while hours of one-second points draw as a few points per column.
pub(super) fn decimate(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut out = Vec::with_capacity(points.len().min(4096));
    for run in points.split(|p| !p[0].is_finite() || !p[1].is_finite()) {
        if run.is_empty() {
            continue;
        }
        if out.last().is_some_and(|q: &[f32; 2]| q[0].is_finite()) {
            out.push([f32::NAN, f32::NAN]);
        }
        let last = run.len() - 1;
        let mut i = 0;
        while i < run.len() {
            let col = run[i][0].floor();
            let mut j = i;
            let (mut lo, mut hi) = (i, i);
            while j < run.len() && run[j][0].floor() == col {
                if run[j][1] < run[lo][1] {
                    lo = j;
                }
                if run[j][1] > run[hi][1] {
                    hi = j;
                }
                j += 1;
            }
            // Beyond the extremes only a run's own ends are needed.
            let first = if i == 0 { i } else { lo.min(hi) };
            let end = if j - 1 == last { j - 1 } else { lo.max(hi) };
            let keep = [first, lo.min(hi), lo.max(hi), end];
            let mut prev = usize::MAX;
            for k in keep {
                if k != prev {
                    out.push(run[k]);
                    prev = k;
                }
            }
            i = j;
        }
    }
    out
}

pub(super) fn lay_out_strip(
    cfg: &LeqConfig,
    h: &LeqHistory,
    judged: bool,
    plot: Rect,
    theme: &Theme,
    look: StripSpan,
) -> HistoryStrip {
    let step = f64::from(h.step());
    let (from, to, x, to_x): (f64, f64, Axis, Box<dyn Fn(f64) -> f64>) = match look.span {
        None => {
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
            (now - span, now, x, Box::new(move |t| (t - now) / unit))
        }
        Some((from, to)) => {
            // Looking back: local clock time along the axis.
            let off = f64::from(look.utc_offset_s);
            let xm = Mapping::new(
                Range::new(from + off, to + off),
                Scale::Linear,
                plot.x,
                plot.right(),
            );
            let x = Axis {
                ticks: clock_ticks(&xm, 70.0),
                mapping: xm,
                title: String::new(),
            };
            (from, to, x, Box::new(move |t| t + off))
        }
    };
    // y from what is in view: values and limits, at least 20 dB, on 5 dB steps.
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for w in &cfg.windows {
        if let Some(ps) = h.points(w) {
            for p in since(ps, from).take_while(|p| p.t <= to + step) {
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
            for p in h
                .points(w)
                .into_iter()
                .flat_map(|ps| since(ps, from - step))
                .take_while(|p| p.t <= to + 2.0 * step)
            {
                // More than two points' time without one: a break, not a line across.
                if last_t.is_some_and(|lt| p.t - lt > 2.5 * step)
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
                let xy = [x.mapping.to_px(to_x(p.t)), y.mapping.to_px(v)];
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
                points: decimate(&pts),
                over: over.iter().map(|r| decimate(r)).collect(),
                limit_y: w.limit.filter(|_| judged).map(|l| y.mapping.to_px(l.0)),
            }
        })
        .collect();
    HistoryStrip {
        plot,
        from,
        to,
        x,
        y,
        lines,
    }
}

/// Which time a history strip shows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StripSpan {
    /// From and to, daemon wall clock s; `None`: live, the newest [`history_span`]
    /// following each new second.
    pub span: Option<(f64, f64)>,
    /// Local time's offset from UTC, s: the clock labels of a span looked back at.
    pub utc_offset_s: i32,
}

/// Local `HH:MM` (`HH:MM:SS` with `seconds`) of local seconds `v`.
fn hms(v: f64, seconds: bool) -> String {
    let s = v.floor().rem_euclid(86_400.0) as u64;
    if seconds {
        format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 3600, (s % 3600) / 60)
    }
}

/// Clock ticks along `m` (local seconds): the finest of whole seconds, minutes and hours
/// that leaves `min_label_px` between labels, labelled `14:30` (`14:30:15` under a minute).
fn clock_ticks(m: &Mapping, min_label_px: f32) -> Vec<axis::Tick> {
    const STEPS: [f64; 17] = [
        1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0, 7200.0,
        10_800.0, 21_600.0, 43_200.0,
    ];
    let span = m.range.span().abs().max(1e-9);
    let px_per_s = m.len_px() / span as f32;
    let step = STEPS
        .iter()
        .copied()
        .find(|s| *s as f32 * px_per_s >= min_label_px)
        .unwrap_or(86_400.0);
    let mut out = Vec::new();
    let mut v = (m.range.lo / step).ceil() * step;
    while v <= m.range.hi + 1e-6 {
        out.push(axis::Tick {
            value: v,
            pos: m.to_px(v),
            kind: axis::TickKind::Major,
            label: Some(hms(v, step < 60.0)),
        });
        v += step;
    }
    out
}

/// What the caption of a strip looking back says: `history · 14:02–15:10 · Home: live`
/// (`loading` while its span is fetched).
pub fn history_caption(
    from: f64,
    to: f64,
    utc_offset_s: i32,
    live_key: Option<&str>,
    loading: bool,
) -> String {
    let off = f64::from(utc_offset_s);
    let fine = to - from < 600.0;
    let mut s = format!(
        "history · {}–{}",
        hms(from + off, fine),
        hms(to + off, fine)
    );
    if loading {
        s.push_str(" · loading");
    }
    if let Some(k) = live_key {
        s.push_str(&format!(" · {k}: live"));
    }
    s
}

/// One window's line of a strip's cursor readout.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadoutRow {
    /// Its index in the configuration (its colour).
    pub window: usize,
    /// `LAeq 15 min 97.3 dB · limit 99 · over`.
    pub text: String,
    pub over: bool,
}

/// The cursor readout of a strip at daemon wall time `t`: the local time (of the bucket,
/// with points of more than a second), and per window its Leq there with its limit and
/// whether it was over. `None` with no point near `t`.
#[derive(Clone, Debug, PartialEq)]
pub struct StripReadout {
    /// `14:32:05`, or `14:31:30–14:32:00 · highest` for a bucket.
    pub time: String,
    pub rows: Vec<ReadoutRow>,
}

/// The readout at `t` (s) of history `h` drawn for `cfg`: each window's point nearest `t`
/// within a point's time. `judged`: the limits are in force (a calibrated meter).
pub fn strip_readout(
    cfg: &LeqConfig,
    h: &LeqHistory,
    judged: bool,
    t: f64,
    utc_offset_s: i32,
) -> Option<StripReadout> {
    let step = f64::from(h.step());
    let unit = match h.scale {
        Some(LevelScale::Dbfs) => "dBFS",
        _ => "dB",
    };
    let mut at = None;
    let mut rows = Vec::new();
    for (i, w) in cfg.windows.iter().enumerate() {
        let Some(ps) = h.points(w) else {
            continue;
        };
        // A point stands for the time up to it: the first at or after `t`.
        let k = ps.partition_point(|p| p.t < t);
        let Some(p) = ps.get(k).filter(|p| p.t - t < step + 0.5) else {
            continue;
        };
        at.get_or_insert(p.t);
        let v = f64::from(p.leq);
        let mut text = format!(
            "{} {}",
            window_name(w),
            if v.is_finite() {
                format!("{} {unit}", format::fixed(v, 1))
            } else {
                format::NO_VALUE.to_string()
            }
        );
        if judged && let Some(l) = w.limit {
            text.push_str(&format!(" · limit {}", format::fixed(l.0, 1)));
            if p.over {
                text.push_str(" · over");
            }
        }
        rows.push(ReadoutRow {
            window: i,
            text,
            over: p.over,
        });
    }
    let end = at? + f64::from(utc_offset_s);
    let time = if step > 1.0 {
        format!("{}–{} · highest", hms(end - step, true), hms(end, true))
    } else {
        // A point is a second's end: the second it stands for starts a second before.
        hms(end - 1.0, true)
    };
    Some(StripReadout { time, rows })
}

/// How the Leq view's history strip looks: live or back at a span, with what it draws
/// then and the pointer over it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LeqStripView<'a> {
    pub span: StripSpan,
    /// The points of a span looked back at, fetched from the log; `None`: the history
    /// received (it covers the span, or nothing better arrived yet).
    pub points: Option<&'a LeqHistory>,
    /// A fetch of the span is under way.
    pub loading: bool,
    /// The pointer over the strip, daemon wall clock s.
    pub cursor: Option<f64>,
    /// The key that returns the strip to live (`Home`).
    pub live_key: Option<String>,
}
