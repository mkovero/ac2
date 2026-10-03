//! Axis mappings (value ↔ logical pixel) and tick generation.
//!
//! One [`Mapping`] is used for both a trace's points and its axis ticks, so a tick and a
//! data point at the same value always land on the same pixel.
//!
//! Tick rules:
//! - **Log frequency.** Labelled ticks come from per-decade sets, coarse to fine:
//!   `{1}`, `{1, 2, 5}`, `{1 … 9}` × 10ⁿ. The finest set whose neighbouring labels are at
//!   least `min_label_px` apart and that puts at least three labels in range wins; the next
//!   finer set gives the minor ticks. Linear 1-2-5 steps in Hz (spaced for the densest,
//!   high-frequency end) are used instead when they label more of the axis, which happens
//!   below about a decade of span (deep zoom).
//! - **Linear.** The smallest step from the 1-2-5 sequence (or the degree sequence for
//!   phase: 1, 2, 5, 10, 20, 45, 90, 180, 360, 720, …) that keeps labels `min_label_px`
//!   apart. Minor ticks subdivide a step when they stay `min_minor_px` apart.
//! - Labels are the bare number; the unit is the axis title (frequency labels carry the
//!   `k` suffix because it is part of the number).

use crate::format;

/// Smallest pixel distance between labelled ticks.
pub const MIN_LABEL_PX: f32 = 40.0;
/// Smallest pixel distance between minor ticks.
pub const MIN_MINOR_PX: f32 = 7.0;

/// Value scale of an axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scale {
    Linear,
    /// Base-10 log; values must be positive.
    Log,
}

/// Value range `lo..hi` of an axis (`lo < hi`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    pub lo: f64,
    pub hi: f64,
}

impl Range {
    pub const fn new(lo: f64, hi: f64) -> Self {
        Self { lo, hi }
    }

    pub fn span(&self) -> f64 {
        self.hi - self.lo
    }

    /// Valid for a linear axis.
    pub fn is_valid(&self) -> bool {
        self.lo.is_finite() && self.hi.is_finite() && self.hi > self.lo
    }
}

/// Maps values in `range` to logical pixels `px_lo..px_hi` along one axis. For a y axis
/// `px_lo` is the bottom (larger y), so higher values are drawn higher.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mapping {
    pub range: Range,
    pub scale: Scale,
    pub px_lo: f32,
    pub px_hi: f32,
}

impl Mapping {
    pub fn new(range: Range, scale: Scale, px_lo: f32, px_hi: f32) -> Self {
        Self {
            range,
            scale,
            px_lo,
            px_hi,
        }
    }

    fn t(&self, v: f64) -> f64 {
        match self.scale {
            Scale::Linear => (v - self.range.lo) / self.range.span(),
            Scale::Log => (v / self.range.lo).ln() / (self.range.hi / self.range.lo).ln(),
        }
    }

    /// Pixel of `v`; NaN when `v` is not finite (or not positive on a log axis), which the
    /// renderer draws as a gap. Values outside the range map outside `px_lo..px_hi`.
    pub fn to_px(&self, v: f64) -> f32 {
        if !v.is_finite() || (self.scale == Scale::Log && v <= 0.0) {
            return f32::NAN;
        }
        let t = self.t(v);
        (f64::from(self.px_lo) + t * f64::from(self.px_hi - self.px_lo)) as f32
    }

    /// Value at pixel `px` (inverse of [`Mapping::to_px`]).
    pub fn from_px(&self, px: f32) -> f64 {
        let t = f64::from(px - self.px_lo) / f64::from(self.px_hi - self.px_lo);
        match self.scale {
            Scale::Linear => self.range.lo + t * self.range.span(),
            Scale::Log => self.range.lo * (self.range.hi / self.range.lo).powf(t),
        }
    }

    /// Length in pixels.
    pub fn len_px(&self) -> f32 {
        (self.px_hi - self.px_lo).abs()
    }

    /// True when `v` lies in the range (with a little float slack at the ends).
    pub fn contains(&self, v: f64) -> bool {
        let eps = 1e-9 * self.range.span().abs().max(self.range.hi.abs());
        v >= self.range.lo - eps && v <= self.range.hi + eps
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickKind {
    Major,
    Minor,
}

/// One tick: value, pixel and (for majors) the label text.
#[derive(Clone, Debug, PartialEq)]
pub struct Tick {
    pub value: f64,
    pub pos: f32,
    pub kind: TickKind,
    pub label: Option<String>,
}

impl Tick {
    fn major(m: &Mapping, value: f64, label: String) -> Self {
        Self {
            value,
            pos: m.to_px(value),
            kind: TickKind::Major,
            label: Some(label),
        }
    }

    fn minor(m: &Mapping, value: f64) -> Self {
        Self {
            value,
            pos: m.to_px(value),
            kind: TickKind::Minor,
            label: None,
        }
    }
}

/// An axis ready to draw: mapping, ticks and title (the unit).
#[derive(Clone, Debug, PartialEq)]
pub struct Axis {
    pub mapping: Mapping,
    pub ticks: Vec<Tick>,
    pub title: String,
}

impl Axis {
    /// Labels of the major ticks in order (handy for tests and for the CLI).
    pub fn labels(&self) -> Vec<&str> {
        self.ticks
            .iter()
            .filter_map(|t| t.label.as_deref())
            .collect()
    }
}

const DECADE_SETS: [&[f64]; 3] = [
    &[1.0],
    &[1.0, 2.0, 5.0],
    &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0],
];

/// Values `mult · 10ⁿ` within the mapping's range.
fn decade_values(m: &Mapping, set: &[f64]) -> Vec<f64> {
    let lo = m.range.lo;
    let hi = m.range.hi;
    let d0 = lo.log10().floor() as i32 - 1;
    let d1 = hi.log10().ceil() as i32 + 1;
    let mut out = Vec::new();
    for d in d0..=d1 {
        let p = 10f64.powi(d);
        for &k in set {
            // Round so 3·10⁻¹ etc. print cleanly.
            let v = round_to_step(k * p, p / 1000.0);
            if m.contains(v) {
                out.push(v);
            }
        }
    }
    out.sort_by(f64::total_cmp);
    out
}

fn round_to_step(v: f64, step: f64) -> f64 {
    (v / step).round() * step
}

fn min_gap_px(m: &Mapping, values: &[f64]) -> f32 {
    values
        .windows(2)
        .map(|w| (m.to_px(w[1]) - m.to_px(w[0])).abs())
        .fold(f32::INFINITY, f32::min)
}

/// Ticks of a log-frequency axis; labels like `20`, `50`, `100`, `1k`, `20k`.
pub fn log_freq_ticks(m: &Mapping, min_label_px: f32, min_minor_px: f32) -> Vec<Tick> {
    if !(m.range.lo > 0.0 && m.range.hi > m.range.lo && m.range.hi.is_finite()) {
        return Vec::new();
    }
    // Over less than about a decade, linear steps can label more of the axis than the
    // per-decade sets (900 Hz–2.1 kHz: 1k, 1.2k, … instead of 900, 1k, 2k); use whichever
    // labels more.
    let linear = log_axis_linear_fallback(m, min_label_px, min_minor_px);
    match decade_ticks(m, min_label_px, min_minor_px, format::freq_tick) {
        Some(t) if labelled(&t) >= labelled(&linear) => t,
        _ => linear,
    }
}

fn labelled(ticks: &[Tick]) -> usize {
    ticks.iter().filter(|t| t.label.is_some()).count()
}

/// Labelled ticks from the finest per-decade set that keeps labels `min_label_px` apart and
/// puts at least three in range, minors from the next finer one; `None` when no set does.
fn decade_ticks(
    m: &Mapping,
    min_label_px: f32,
    min_minor_px: f32,
    fmt: impl Fn(f64) -> String,
) -> Option<Vec<Tick>> {
    let (level, majors) = (0..DECADE_SETS.len()).rev().find_map(|level| {
        let v = decade_values(m, DECADE_SETS[level]);
        (v.len() >= 3 && min_gap_px(m, &v) >= min_label_px).then_some((level, v))
    })?;
    let mut ticks: Vec<Tick> = majors.iter().map(|&v| Tick::major(m, v, fmt(v))).collect();
    // Minor ticks: the finest finer set whose spacing stays readable; for the finest set a
    // half-step subdivision.
    let finer: Vec<Vec<f64>> = if level + 1 < DECADE_SETS.len() {
        ((level + 1)..DECADE_SETS.len())
            .rev()
            .map(|l| decade_values(m, DECADE_SETS[l]))
            .collect()
    } else {
        let halves: Vec<f64> = (2..20).map(|i| f64::from(i) / 2.0).collect();
        vec![decade_values(m, &halves)]
    };
    if let Some(minor) = finer
        .into_iter()
        .find(|v| v.len() >= 2 && min_gap_px(m, v) >= min_minor_px)
    {
        ticks.extend(
            minor
                .into_iter()
                .filter(|v| !majors.iter().any(|mv| close(*mv, *v)))
                .map(|v| Tick::minor(m, v)),
        );
    }
    sort_ticks(&mut ticks);
    Some(ticks)
}

/// Ticks of a log value axis spanning several decades (distortion in percent): the
/// per-decade sets as for frequency, and on an axis too short for a label per decade every
/// second (third, …) decade labelled, the others minor.
pub fn log_ticks(
    m: &Mapping,
    min_label_px: f32,
    min_minor_px: f32,
    fmt: impl Fn(f64) -> String,
) -> Vec<Tick> {
    if !(m.range.lo > 0.0 && m.range.hi > m.range.lo && m.range.hi.is_finite()) {
        return Vec::new();
    }
    if let Some(t) = decade_ticks(m, min_label_px, min_minor_px, &fmt) {
        return t;
    }
    let decades = decade_values(m, DECADE_SETS[0]);
    let exp = |v: f64| v.log10().round() as i64;
    let every = |s: i64| -> Vec<f64> {
        decades
            .iter()
            .copied()
            .filter(|v| exp(*v).rem_euclid(s) == 0)
            .collect()
    };
    let stride = (2..=decades.len().max(2) as i64)
        .find(|s| {
            let v = every(*s);
            v.len() < 2 || min_gap_px(m, &v) >= min_label_px
        })
        .unwrap_or(2);
    let mut ticks: Vec<Tick> = decades
        .iter()
        .map(|&v| {
            if exp(v).rem_euclid(stride) == 0 {
                Tick::major(m, v, fmt(v))
            } else {
                Tick::minor(m, v)
            }
        })
        .collect();
    sort_ticks(&mut ticks);
    ticks
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1e-12)
}

fn sort_ticks(ticks: &mut [Tick]) {
    ticks.sort_by(|a, b| a.value.total_cmp(&b.value));
}

/// Deep zoom on a log axis: linear steps in Hz, chosen where the axis is densest (the top).
fn log_axis_linear_fallback(m: &Mapping, min_label_px: f32, min_minor_px: f32) -> Vec<Tick> {
    // d(px)/d(f) of a log axis is len / (f · ln(hi/lo)); smallest at f = hi.
    let px_per_hz = f64::from(m.len_px()) / (m.range.hi * (m.range.hi / m.range.lo).ln());
    linear_ticks_with(
        m,
        px_per_hz,
        Steps::Decimal,
        min_label_px,
        min_minor_px,
        |v, _| format::freq_tick(v),
    )
}

/// Which "nice" step sequence a linear axis uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Steps {
    /// 1, 2, 5 × 10ⁿ.
    Decimal,
    /// 1, 2, 5, 10, 20, 45, 90, 180, 360, 720, … (phase in degrees).
    Degrees,
}

/// A step and how many minor intervals it divides into, in preference order.
fn step_candidates(steps: Steps, at_least: f64) -> Vec<(f64, &'static [u32])> {
    let mut out = Vec::new();
    match steps {
        Steps::Decimal => {
            let e0 = at_least.log10().floor() as i32 - 1;
            for e in e0..e0 + 4 {
                let p = 10f64.powi(e);
                out.push((p, &[5u32, 2][..]));
                out.push((2.0 * p, &[4u32, 2][..]));
                out.push((5.0 * p, &[5u32][..]));
            }
        }
        Steps::Degrees => {
            for (s, div) in [
                (1.0, &[5u32, 2][..]),
                (2.0, &[4, 2][..]),
                (5.0, &[5][..]),
                (10.0, &[5, 2][..]),
                (20.0, &[4, 2][..]),
                (45.0, &[3][..]),
                (90.0, &[3, 2][..]),
                (180.0, &[4, 2][..]),
            ] {
                out.push((s, div));
            }
            let mut s = 360.0;
            while s < at_least * 4.0 || s <= 360.0 {
                out.push((s, &[4u32, 2][..]));
                s *= 2.0;
            }
        }
    }
    out
}

/// Ticks of a linear axis; `fmt(value, decimals)` makes the label.
pub fn linear_ticks(
    m: &Mapping,
    steps: Steps,
    min_label_px: f32,
    min_minor_px: f32,
    fmt: impl Fn(f64, usize) -> String,
) -> Vec<Tick> {
    if !m.range.is_valid() {
        return Vec::new();
    }
    let px_per_unit = f64::from(m.len_px()) / m.range.span();
    linear_ticks_with(m, px_per_unit, steps, min_label_px, min_minor_px, fmt)
}

fn linear_ticks_with(
    m: &Mapping,
    px_per_unit: f64,
    steps: Steps,
    min_label_px: f32,
    min_minor_px: f32,
    fmt: impl Fn(f64, usize) -> String,
) -> Vec<Tick> {
    if !(px_per_unit > 0.0 && px_per_unit.is_finite()) {
        return Vec::new();
    }
    let need = f64::from(min_label_px) / px_per_unit;
    let Some((step, divs)) = step_candidates(steps, need)
        .into_iter()
        .find(|(s, _)| *s >= need * (1.0 - 1e-9))
    else {
        return Vec::new();
    };
    let decimals = (-(step.log10().floor())).max(0.0) as usize;
    let first = (m.range.lo / step - 1e-9).ceil() as i64;
    let last = (m.range.hi / step + 1e-9).floor() as i64;
    let mut ticks = Vec::new();
    for i in first..=last {
        let v = i as f64 * step;
        // Clean float noise and negative zero.
        let v = if i == 0 { 0.0 } else { v };
        ticks.push(Tick::major(m, v, fmt(v, decimals)));
    }
    if let Some(&div) = divs
        .iter()
        .find(|&&d| step / f64::from(d) * px_per_unit >= f64::from(min_minor_px))
    {
        let minor = step / f64::from(div);
        let first = (m.range.lo / minor - 1e-9).ceil() as i64;
        let last = (m.range.hi / minor + 1e-9).floor() as i64;
        for i in first..=last {
            if i % i64::from(div) != 0 {
                ticks.push(Tick::minor(m, i as f64 * minor));
            }
        }
    }
    sort_ticks(&mut ticks);
    ticks
}

/// Plain number label: decimals from the step, typographic minus.
pub fn number_label(v: f64, decimals: usize) -> String {
    format::fixed(v, decimals)
}

/// Phase label in whole degrees (`−180`, `−90`, `0`, `90`, `180`).
pub fn degree_label(v: f64, decimals: usize) -> String {
    format::fixed(v, decimals)
}

// ---------------------------------------------------------------------------------------
// Concrete axes

/// Log-frequency axis over `range` Hz.
pub fn freq_axis(range: Range, px_lo: f32, px_hi: f32) -> Axis {
    let mapping = Mapping::new(range, Scale::Log, px_lo, px_hi);
    Axis {
        ticks: log_freq_ticks(&mapping, MIN_LABEL_PX, MIN_MINOR_PX),
        mapping,
        title: "Hz".to_string(),
    }
}

/// Label spacing along a vertical axis: labels stack by their height, not their width.
pub const MIN_LABEL_PX_VERTICAL: f32 = 24.0;
/// Label spacing along a horizontal number axis: `−0.25` is wider than `1k`.
pub const MIN_LABEL_PX_NUMBERS: f32 = 50.0;

/// Label spacing for a linear axis: horizontal when pixels grow with the value.
fn linear_label_px(px_lo: f32, px_hi: f32) -> f32 {
    if px_hi > px_lo {
        MIN_LABEL_PX_NUMBERS
    } else {
        MIN_LABEL_PX_VERTICAL
    }
}

/// Linear axis with 1-2-5 ticks and number labels. Horizontal when `px_hi > px_lo`
/// (labels need [`MIN_LABEL_PX_NUMBERS`]), vertical otherwise ([`MIN_LABEL_PX_VERTICAL`]).
pub fn linear_axis(range: Range, px_lo: f32, px_hi: f32, title: &str) -> Axis {
    let mapping = Mapping::new(range, Scale::Linear, px_lo, px_hi);
    Axis {
        ticks: linear_ticks(
            &mapping,
            Steps::Decimal,
            linear_label_px(px_lo, px_hi),
            MIN_MINOR_PX,
            number_label,
        ),
        mapping,
        title: title.to_string(),
    }
}

/// Phase axis in degrees (wrapped: `−180..180`, unwrapped: any range).
pub fn phase_axis(range: Range, px_lo: f32, px_hi: f32) -> Axis {
    let mapping = Mapping::new(range, Scale::Linear, px_lo, px_hi);
    Axis {
        ticks: linear_ticks(
            &mapping,
            Steps::Degrees,
            linear_label_px(px_lo, px_hi),
            MIN_MINOR_PX,
            degree_label,
        ),
        mapping,
        title: "°".to_string(),
    }
}

/// `0.01`, `0.1`, `1`, `10`, `100`, `0.005`: as many decimals as the value's decade needs.
pub fn percent_label(v: f64) -> String {
    // Slack for decades that log10 puts a hair under the integer.
    let decimals = (-(v.log10() + 1e-9).floor()).max(0.0) as usize;
    format::fixed(v, decimals)
}

/// Log axis of a ratio in percent (distortion): decade labels `0.01` … `100`, the unit in
/// the title. Equal ratios get equal distances, as on the dB axis it stands in for.
pub fn percent_axis(range: Range, px_lo: f32, px_hi: f32, title: &str) -> Axis {
    let mapping = Mapping::new(range, Scale::Log, px_lo, px_hi);
    Axis {
        ticks: log_ticks(
            &mapping,
            linear_label_px(px_lo, px_hi),
            MIN_MINOR_PX,
            percent_label,
        ),
        mapping,
        title: title.to_string(),
    }
}

/// Like [`linear_axis`] / [`phase_axis`] (`steps` picks which), but the tick step is chosen
/// as if the axis were `density_px` long. Panes that change height between layouts keep
/// the same steps, so switching a layout never relabels an axis whose range did not change.
pub fn axis_with_density(
    range: Range,
    px_lo: f32,
    px_hi: f32,
    title: &str,
    steps: Steps,
    density_px: f32,
) -> Axis {
    let mapping = Mapping::new(range, Scale::Linear, px_lo, px_hi);
    let px_per_unit = if range.is_valid() {
        f64::from(density_px.abs()) / range.span()
    } else {
        0.0
    };
    let fmt = match steps {
        Steps::Decimal => number_label,
        Steps::Degrees => degree_label,
    };
    Axis {
        ticks: linear_ticks_with(
            &mapping,
            px_per_unit,
            steps,
            linear_label_px(px_lo, px_hi),
            MIN_MINOR_PX,
            fmt,
        ),
        mapping,
        title: title.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f_axis(lo: f64, hi: f64, w: f32) -> Axis {
        freq_axis(Range::new(lo, hi), 0.0, w)
    }

    #[test]
    fn mapping_round_trip_and_gaps() {
        let m = Mapping::new(Range::new(20.0, 20_000.0), Scale::Log, 0.0, 300.0);
        assert!((m.to_px(20.0) - 0.0).abs() < 1e-4);
        assert!((m.to_px(20_000.0) - 300.0).abs() < 1e-3);
        // One decade = a third of three decades.
        assert!((m.to_px(200.0) - 100.0).abs() < 1e-3);
        assert!((m.from_px(m.to_px(1234.0)) - 1234.0).abs() < 1e-6 * 1234.0 * 100.0);
        assert!(m.to_px(f64::NAN).is_nan());
        assert!(m.to_px(0.0).is_nan());
        // y axis: low values at the bottom.
        let y = Mapping::new(Range::new(-30.0, 30.0), Scale::Linear, 200.0, 0.0);
        assert_eq!(y.to_px(-30.0), 200.0);
        assert_eq!(y.to_px(30.0), 0.0);
        assert_eq!(y.to_px(0.0), 100.0);
    }

    #[test]
    fn default_log_axis_is_1_2_5() {
        let a = f_axis(20.0, 20_000.0, 900.0);
        assert_eq!(
            a.labels(),
            [
                "20", "50", "100", "200", "500", "1k", "2k", "5k", "10k", "20k"
            ]
        );
        // Minors are the remaining 1…9 multiples.
        let minors: Vec<f64> = a
            .ticks
            .iter()
            .filter(|t| t.kind == TickKind::Minor)
            .map(|t| t.value)
            .collect();
        assert!(minors.contains(&30.0) && minors.contains(&9000.0));
        assert!(!minors.contains(&50.0));
        // Ticks and data share one mapping: 1 kHz sits exactly half-way.
        let k = a.ticks.iter().find(|t| t.value == 1000.0).expect("1k");
        let want = 900.0 * (50f64.ln() / 1000f64.ln()) as f32;
        assert!((k.pos - want).abs() < 1e-3);
        assert_eq!(k.pos, a.mapping.to_px(1000.0));
    }

    #[test]
    fn narrow_pane_drops_to_decades() {
        let a = f_axis(20.0, 20_000.0, 300.0);
        assert_eq!(a.labels(), ["100", "1k", "10k"]);
    }

    #[test]
    fn wide_or_zoomed_uses_every_integer() {
        let a = f_axis(20.0, 200.0, 900.0);
        assert_eq!(
            a.labels(),
            ["20", "30", "40", "50", "60", "70", "80", "90", "100", "200"]
        );
        let a = f_axis(100.0, 1000.0, 900.0);
        assert_eq!(a.labels()[0], "100");
        assert_eq!(*a.labels().last().expect("labels"), "1k");
    }

    #[test]
    fn deep_zoom_falls_back_to_linear_steps() {
        let a = f_axis(950.0, 1050.0, 800.0);
        assert_eq!(
            a.labels(),
            [
                "950", "960", "970", "980", "990", "1k", "1.01k", "1.02k", "1.03k", "1.04k",
                "1.05k"
            ]
        );
        let a = f_axis(900.0, 2100.0, 800.0);
        assert_eq!(
            a.labels(),
            [
                "900", "1k", "1.1k", "1.2k", "1.3k", "1.4k", "1.5k", "1.6k", "1.7k", "1.8k",
                "1.9k", "2k", "2.1k"
            ]
        );
        // Narrower pane: wider steps.
        let a = f_axis(900.0, 2100.0, 400.0);
        assert_eq!(a.labels(), ["1k", "1.2k", "1.4k", "1.6k", "1.8k", "2k"]);
    }

    #[test]
    fn labels_never_collide() {
        for (lo, hi, w) in [
            (20.0, 20_000.0, 1200.0),
            (20.0, 20_000.0, 2600.0),
            (31.0, 47.0, 500.0),
            (5000.0, 24_000.0, 700.0),
            (1.0, 100_000.0, 640.0),
        ] {
            let a = f_axis(lo, hi, w);
            let pos: Vec<f32> = a
                .ticks
                .iter()
                .filter(|t| t.label.is_some())
                .map(|t| t.pos)
                .collect();
            assert!(pos.len() >= 2, "{lo}..{hi}: {:?}", a.labels());
            for p in pos.windows(2) {
                assert!(
                    p[1] - p[0] >= MIN_LABEL_PX - 1e-3,
                    "{lo}..{hi} {:?}",
                    a.labels()
                );
            }
        }
    }

    #[test]
    fn db_axis_steps() {
        let a = linear_axis(Range::new(-30.0, 30.0), 150.0, 0.0, "dB");
        assert_eq!(a.labels(), ["−30", "−20", "−10", "0", "10", "20", "30"]);
        let a = linear_axis(Range::new(-30.0, 30.0), 120.0, 0.0, "dB");
        assert_eq!(a.labels(), ["−20", "0", "20"]);
        let a = linear_axis(Range::new(-3.0, 3.0), 150.0, 0.0, "dB");
        assert_eq!(a.labels(), ["−3", "−2", "−1", "0", "1", "2", "3"]);
        let a = linear_axis(Range::new(0.0, 1.0), 100.0, 0.0, "γ²");
        assert_eq!(a.labels(), ["0.0", "0.5", "1.0"]);
        let a = linear_axis(Range::new(0.0, 1.0), 200.0, 0.0, "γ²");
        assert_eq!(a.labels(), ["0.0", "0.2", "0.4", "0.6", "0.8", "1.0"]);
    }

    #[test]
    fn phase_axis_steps() {
        let a = phase_axis(Range::new(-180.0, 180.0), 120.0, 0.0);
        assert_eq!(a.labels(), ["−180", "−90", "0", "90", "180"]);
        let a = phase_axis(Range::new(-180.0, 180.0), 400.0, 0.0);
        assert_eq!(
            a.labels(),
            ["−180", "−135", "−90", "−45", "0", "45", "90", "135", "180"]
        );
        let a = phase_axis(Range::new(-1440.0, 360.0), 200.0, 0.0);
        assert_eq!(a.labels(), ["−1440", "−1080", "−720", "−360", "0", "360"]);
    }

    #[test]
    fn percent_axis_is_log_with_decade_labels() {
        let r = Range::new(0.001, 100.0);
        // Tall: 1-2-5 per decade.
        let a = percent_axis(r, 600.0, 0.0, "%");
        assert_eq!(a.mapping.scale, Scale::Log);
        assert_eq!(a.labels()[..4], ["0.001", "0.002", "0.005", "0.01"]);
        assert_eq!(*a.labels().last().expect("labels"), "100");
        // A 1/4 pane: one label per decade.
        let a = percent_axis(r, 200.0, 0.0, "%");
        assert_eq!(a.labels(), ["0.001", "0.01", "0.1", "1", "10", "100"]);
        // Equal ratios, equal distances: 1 % sits 2/5 of the way down from 100 %.
        let p1 = a.ticks.iter().find(|t| t.value == 1.0).expect("1 %").pos;
        assert!((p1 - 80.0).abs() < 1e-3, "{p1}");
        // Too short for every decade: every second one labelled, the rest minor.
        let a = percent_axis(r, 90.0, 0.0, "%");
        assert_eq!(a.labels(), ["0.01", "1", "100"]);
        assert_eq!(
            a.ticks.iter().filter(|t| t.kind == TickKind::Minor).count(),
            3
        );
        for h in [60.0f32, 90.0, 200.0, 400.0, 800.0] {
            let a = percent_axis(r, h, 0.0, "%");
            let pos: Vec<f32> = a
                .ticks
                .iter()
                .filter(|t| t.label.is_some())
                .map(|t| t.pos)
                .collect();
            assert!(!pos.is_empty(), "{h}");
            for p in pos.windows(2) {
                assert!((p[0] - p[1]).abs() >= MIN_LABEL_PX_VERTICAL - 1e-3, "{h}");
            }
        }
    }

    #[test]
    fn density_sets_the_step_not_the_mapping() {
        let r = Range::new(-30.0, 30.0);
        // Same density as the real length: identical to the plain axes.
        assert_eq!(
            axis_with_density(r, 300.0, 0.0, "dB", Steps::Decimal, 300.0),
            linear_axis(r, 300.0, 0.0, "dB")
        );
        let p = Range::new(-180.0, 180.0);
        assert_eq!(
            axis_with_density(p, 200.0, 0.0, "°", Steps::Degrees, 200.0),
            phase_axis(p, 200.0, 0.0)
        );
        // A taller axis keeps the steps of the shorter one it stands in for, mapped onto its
        // own pixels.
        let tall = axis_with_density(p, 400.0, 0.0, "°", Steps::Degrees, 120.0);
        assert_eq!(tall.labels(), phase_axis(p, 120.0, 0.0).labels());
        assert_eq!(tall.mapping.to_px(180.0), 0.0);
        assert_eq!(tall.mapping.to_px(-180.0), 400.0);
    }
}
