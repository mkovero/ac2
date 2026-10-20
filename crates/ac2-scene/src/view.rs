//! View state: what the operator chose to look at (ranges, modes, cursor), as plain data.
//! Builders read it; the UI owns and edits it (zoom, pan, toggles).

use crate::axis::Range;
use crate::legend::LegendView;
use crate::trace::TraceKey;

/// Lowest frequency the view can zoom out to.
pub const FREQ_LIMIT_LO: f64 = 1.0;
/// Highest frequency the view can zoom out to.
pub const FREQ_LIMIT_HI: f64 = 100_000.0;
/// Narrowest frequency span (hi / lo): about 1/7 octave.
pub const FREQ_MIN_RATIO: f64 = 1.1;

/// Frequency range of the log axis shared by the TF and spectrum panes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FreqRange {
    pub lo: f64,
    pub hi: f64,
}

impl Default for FreqRange {
    /// 20 Hz – 20 kHz.
    fn default() -> Self {
        Self {
            lo: 20.0,
            hi: 20_000.0,
        }
    }
}

impl FreqRange {
    pub fn range(&self) -> Range {
        Range::new(self.lo, self.hi)
    }

    /// Zoom by `factor` (> 1 zooms in) keeping `about` at the same screen position; clamped
    /// to the limits and the minimum span.
    pub fn zoom(&self, about: f64, factor: f64) -> Self {
        if !(factor > 0.0 && about > 0.0) {
            return *self;
        }
        let (l, h, a) = (self.lo.ln(), self.hi.ln(), about.ln());
        let span = ((h - l) / factor).max(FREQ_MIN_RATIO.ln());
        let t = ((a - l) / (h - l)).clamp(0.0, 1.0);
        let lo = a - t * span;
        Self::from_log(lo, lo + span)
    }

    /// Pan by `octaves` (positive = towards higher frequencies), keeping the span.
    pub fn pan(&self, octaves: f64) -> Self {
        let d = octaves * 2f64.ln();
        Self::from_log(self.lo.ln() + d, self.hi.ln() + d)
    }

    fn from_log(lo: f64, hi: f64) -> Self {
        let (ll, lh) = (FREQ_LIMIT_LO.ln(), FREQ_LIMIT_HI.ln());
        let span = (hi - lo).min(lh - ll);
        let lo = lo.clamp(ll, lh - span);
        Self {
            lo: lo.exp(),
            hi: (lo + span).exp(),
        }
    }
}

/// Lowest level a dB axis can pan to.
pub const LEVEL_LIMIT_LO: f64 = -300.0;
/// Highest level a dB axis can pan to.
pub const LEVEL_LIMIT_HI: f64 = 300.0;
/// Narrowest level span, dB: a tenth of a dB still gets its own label.
pub const LEVEL_MIN_SPAN: f64 = 1.0;
/// Narrowest span a fit frames: a flat trace still shows its ripple against a few dB.
pub const LEVEL_FIT_MIN_SPAN: f64 = 6.0;

/// The limits a linear axis is navigated within: its range stays inside `limits` and at
/// least `min_span` wide. Every zoom and pan of a linear axis (level, IR time, IR amplitude)
/// goes through one of these, so each keeps the value under the pointer where it was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AxisBounds {
    pub limits: Range,
    pub min_span: f64,
}

impl AxisBounds {
    /// The range kept within the limits, its span between `min_span` and the limits' span;
    /// an invalid range is returned as it is.
    pub fn clamp(&self, r: Range) -> Range {
        if !r.is_valid() || !self.limits.is_valid() {
            return r;
        }
        let max = self.limits.span();
        let span = r.span().clamp(self.min_span.min(max), max);
        let mid = (r.lo + r.hi) / 2.0;
        let lo = (mid - span / 2.0).clamp(self.limits.lo, self.limits.hi - span);
        Range::new(lo, lo + span)
    }

    /// Zoom by `factor` (> 1 zooms in) keeping `about` at the same place on the axis.
    pub fn zoom(&self, r: Range, about: f64, factor: f64) -> Range {
        if !(factor > 0.0 && factor.is_finite() && about.is_finite() && r.is_valid()) {
            return r;
        }
        let max = self.limits.span();
        let span = (r.span() / factor).clamp(self.min_span.min(max), max);
        let t = ((about - r.lo) / r.span()).clamp(0.0, 1.0);
        let lo = about - t * span;
        self.clamp(Range::new(lo, lo + span))
    }

    /// Pan by `by` (positive shows higher values), keeping the span.
    pub fn pan(&self, r: Range, by: f64) -> Range {
        if !(by.is_finite() && r.is_valid()) {
            return r;
        }
        self.clamp(Range::new(r.lo + by, r.hi + by))
    }
}

/// The smallest 1, 2, 5 × 10ⁿ at least `x` (`x` > 0).
fn nice(x: f64) -> f64 {
    if !(x > 0.0 && x.is_finite()) {
        return 1.0;
    }
    let p = 10f64.powf(x.log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|m| m * p)
        .find(|s| *s >= x * (1.0 - 1e-9))
        .unwrap_or(10.0 * p)
}

/// The 1-2-5 step one key press pans an axis showing `r` by: about a tenth of the span, so
/// a few presses move a curve across the pane and the grid lines stay on round values.
pub fn pan_step(r: Range) -> f64 {
    nice(r.span() / 10.0)
}

/// Navigation of a vertical dB axis (transfer magnitude, spectrum level, distortion, the IR
/// views' log and ETC levels): zoom about a level, pan by dB, frame the shown data. The axis
/// keeps its tick rules ([`crate::axis::linear_ticks`]), which label any range from a tenth
/// of a dB to hundreds.
pub mod level {
    use super::{
        AxisBounds, LEVEL_FIT_MIN_SPAN, LEVEL_LIMIT_HI, LEVEL_LIMIT_LO, LEVEL_MIN_SPAN, nice,
    };
    use crate::axis::Range;

    const BOUNDS: AxisBounds = AxisBounds {
        limits: Range::new(LEVEL_LIMIT_LO, LEVEL_LIMIT_HI),
        min_span: LEVEL_MIN_SPAN,
    };

    /// The range kept within the limits, its span between [`LEVEL_MIN_SPAN`] and the
    /// limits' span; an invalid range is returned as it is.
    pub fn clamp(r: Range) -> Range {
        BOUNDS.clamp(r)
    }

    /// Zoom by `factor` (> 1 zooms in) keeping `about` at the same height.
    pub fn zoom(r: Range, about: f64, factor: f64) -> Range {
        BOUNDS.zoom(r, about, factor)
    }

    /// Pan by `db` (positive shows higher levels), keeping the span.
    pub fn pan(r: Range, db: f64) -> Range {
        BOUNDS.pan(r, db)
    }

    /// The 1-2-5 step one key press pans by ([`super::pan_step`]).
    pub fn pan_step(r: Range) -> f64 {
        super::pan_step(r)
    }

    /// A range that frames `values` (dB; NaN and ±∞ ignored): from the lowest percent of
    /// them — a few empty bins or a deep null do not stretch it — to the highest, padded
    /// and rounded out to the grid step, at least [`LEVEL_FIT_MIN_SPAN`] wide. `None`
    /// without a finite value.
    pub fn fit(values: impl IntoIterator<Item = f64>) -> Option<Range> {
        let mut v: Vec<f64> = values.into_iter().filter(|x| x.is_finite()).collect();
        if v.is_empty() {
            return None;
        }
        v.sort_by(f64::total_cmp);
        let lo = v[v.len() / 100];
        let hi = v[v.len() - 1];
        let mid = (lo + hi) / 2.0;
        let span = (hi - lo).max(LEVEL_FIT_MIN_SPAN);
        let pad = (span * 0.08).max(1.0);
        let (lo, hi) = (mid - span / 2.0 - pad, mid + span / 2.0 + pad);
        let step = nice((hi - lo) / 8.0);
        let r = Range::new((lo / step).floor() * step, (hi / step).ceil() * step);
        Some(clamp(r))
    }
}

/// Narrowest time span of an IR view, in sample spacings: a few samples still show the
/// shape between them, where one spacing alone would be a single segment.
pub const IR_MIN_SPAN_SAMPLES: f64 = 4.0;
/// Largest amplitude a linear IR view can show, FS: a transfer function with 40 dB of gain.
pub const IR_AMPLITUDE_LIMIT: f64 = 100.0;
/// Narrowest amplitude span of a linear IR view, FS (−120 dB re full scale).
pub const IR_AMPLITUDE_MIN_SPAN: f64 = 1e-6;

/// Navigation of a linear IR view's amplitude axis, FS.
pub const IR_AMPLITUDE_BOUNDS: AxisBounds = AxisBounds {
    limits: Range::new(-IR_AMPLITUDE_LIMIT, IR_AMPLITUDE_LIMIT),
    min_span: IR_AMPLITUDE_MIN_SPAN,
};

/// Where an impulse response lies in time, ms re its t = 0: its first and last sample and
/// their spacing. Its time axis is navigated within it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IrExtent {
    /// From the first to the last sample; ±1 ms when there is no span.
    pub full: Range,
    /// The first sample's time.
    pub t0_ms: f64,
    pub dt_ms: f64,
    /// Samples.
    pub n: usize,
}

impl IrExtent {
    /// `n` samples `dt_s` apart from `t0_s`.
    pub fn of(t0_s: f64, dt_s: f64, n: usize) -> Self {
        let (lo, dt) = (t0_s * 1000.0, dt_s * 1000.0);
        let hi = lo + n.saturating_sub(1) as f64 * dt;
        let full = if lo.is_finite() && hi.is_finite() && hi > lo {
            Range::new(lo, hi)
        } else {
            Range::new(-1.0, 1.0)
        };
        Self {
            full,
            t0_ms: if lo.is_finite() { lo } else { 0.0 },
            dt_ms: if dt > 0.0 && dt.is_finite() { dt } else { 0.0 },
            n,
        }
    }

    /// Time of sample `i`.
    pub fn time_of(&self, i: usize) -> f64 {
        self.t0_ms + i as f64 * self.dt_ms
    }

    /// `t_ms` moved to the nearest sample.
    pub fn snap(&self, t_ms: f64) -> Option<f64> {
        self.nearest(t_ms).map(|i| self.time_of(i))
    }

    /// The time axis's bounds: never narrower than [`IR_MIN_SPAN_SAMPLES`] samples, never
    /// further outside the IR than its own length (beyond that there is nothing to see).
    pub fn bounds(&self) -> AxisBounds {
        let len = self.full.span();
        AxisBounds {
            limits: Range::new(self.full.lo - len, self.full.hi + len),
            min_span: IR_MIN_SPAN_SAMPLES * self.dt_ms,
        }
    }

    /// Index of the sample nearest `t_ms`.
    pub fn nearest(&self, t_ms: f64) -> Option<usize> {
        if self.n == 0 || !t_ms.is_finite() || self.dt_ms <= 0.0 {
            return None;
        }
        let i = ((t_ms - self.t0_ms) / self.dt_ms).round();
        Some(i.clamp(0.0, (self.n - 1) as f64) as usize)
    }

    /// One press of the cursor keys: a sample when zoomed in that far, else a hundredth of
    /// the shown span in whole samples, so a few presses cross a reflection at any zoom.
    pub fn cursor_step(&self, shown: Range) -> f64 {
        if self.dt_ms <= 0.0 {
            return shown.span() / 100.0;
        }
        (shown.span() / 100.0 / self.dt_ms).round().max(1.0) * self.dt_ms
    }
}

/// How the phase pane shows phase.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PhaseView {
    /// −180…+180°.
    Wrapped,
    /// Unwrapped along frequency, over `range` degrees.
    Unwrapped { range: Range },
    /// Group delay over `range_ms` milliseconds.
    GroupDelay { range_ms: Range },
}

impl PhaseView {
    /// Shift+P: wrapped → unwrapped → group delay → wrapped, each view at its default range.
    pub fn next(self) -> Self {
        match self {
            Self::Wrapped => Self::Unwrapped {
                range: Range::new(-720.0, 180.0),
            },
            Self::Unwrapped { .. } => Self::GroupDelay {
                range_ms: Range::new(-2.0, 10.0),
            },
            Self::GroupDelay { .. } => Self::Wrapped,
        }
    }

    /// What the operator reads when stepping (toast).
    pub fn name(self) -> &'static str {
        match self {
            Self::Wrapped => "phase: wrapped",
            Self::Unwrapped { .. } => "phase: unwrapped",
            Self::GroupDelay { .. } => "group delay",
        }
    }
}

/// Coherence-driven display of TF traces (magnitude and phase panes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoherenceStyle {
    /// Columns with γ² below this are not drawn (a gap). `None` = no blanking.
    pub blank_below: Option<f32>,
    /// Fade traces where coherence is low.
    pub alpha: bool,
    /// Opacity at γ² = 0 (or at the blanking threshold when blanking is on).
    pub alpha_floor: f32,
    /// γ² at and above which a trace is fully opaque.
    pub alpha_full_at: f32,
}

impl Default for CoherenceStyle {
    fn default() -> Self {
        Self {
            blank_below: None,
            alpha: true,
            alpha_floor: 0.15,
            alpha_full_at: 0.9,
        }
    }
}

/// Where the coherence trace is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CoherencePlacement {
    /// Its own pane under phase; magnitude : phase : coherence heights 3 : 2 : 1.
    #[default]
    Pane,
    /// Scaled into the top of the magnitude pane with its own 0–1 axis on the right;
    /// magnitude : phase heights 3 : 2. Needs the magnitude pane: with magnitude hidden,
    /// coherence keeps its own pane.
    OverlayOnMagnitude,
}

/// Transfer-function panes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TfView {
    pub show_magnitude: bool,
    pub show_phase: bool,
    pub show_coherence: bool,
    pub magnitude_db: Range,
    pub phase: PhaseView,
    pub coherence: CoherenceStyle,
    pub coherence_placement: CoherencePlacement,
    /// Trace whose measured delay is the phase reference (decision 8b). `None`: the first
    /// trace with a shared time base.
    pub phase_reference: Option<TraceKey>,
    /// Where the legend sits over the magnitude pane, how large it may be.
    pub legend: LegendView,
    /// Min / max tracking (P): the highest peaks and deepest dips of the magnitude marked
    /// and listed.
    pub extremes: bool,
}

impl Default for TfView {
    fn default() -> Self {
        Self {
            show_magnitude: true,
            show_phase: true,
            show_coherence: true,
            magnitude_db: Range::new(-30.0, 30.0),
            phase: PhaseView::Wrapped,
            coherence: CoherenceStyle::default(),
            coherence_placement: CoherencePlacement::Pane,
            phase_reference: None,
            legend: LegendView::default(),
            extremes: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpectrumView {
    /// Level axis while the pane shows dBFS.
    pub level: Range,
    /// Level axis while the pane shows dB SPL (calibrated): the same zoom and pan keys act on
    /// it, and each scale keeps its own range, so calibrating doesn't push the curves off
    /// a dBFS-sized axis.
    pub level_spl: Range,
    /// What the pane shows: the spectrum, the spectrograph or both.
    pub mode: SpectrumMode,
    pub spectrograph: SpectrographView,
}

/// What the spectrum pane shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SpectrumMode {
    /// The spectrum (or RTA) alone.
    #[default]
    Spectrum,
    /// The spectrum on top, the spectrograph of the pane's measurement under it, on the same
    /// frequency pixels.
    Split,
    /// The spectrograph alone, the whole pane (full screen with W / F11).
    Spectrograph,
}

impl SpectrumMode {
    /// G: spectrum → spectrum + spectrograph → spectrograph → spectrum. The spectrograph
    /// first comes in beside the curve it is made of, so its history (kept only while it is
    /// shown) builds while the spectrum is still in view; it then takes the whole pane
    /// with its history, and the last step back hides it.
    pub fn next(self) -> Self {
        match self {
            Self::Spectrum => Self::Split,
            Self::Split => Self::Spectrograph,
            Self::Spectrograph => Self::Spectrum,
        }
    }

    /// Shift+G: [`Self::next`] backwards.
    pub fn prev(self) -> Self {
        match self {
            Self::Spectrum => Self::Spectrograph,
            Self::Split => Self::Spectrum,
            Self::Spectrograph => Self::Split,
        }
    }

    /// The spectrograph is drawn (and its history kept).
    pub fn spectrograph(self) -> bool {
        self != Self::Spectrum
    }
}

/// The spectrograph's cursor. How much history it keeps and its colours are its
/// measurement's ([`ac2_proto::model::SpectrumDisplay`]).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct SpectrographView {
    /// The cursor's time, seconds before the newest frame (with `cursor_hz`, a point).
    pub cursor_s: Option<f64>,
}

impl SpectrumView {
    /// The level axis for curves shown in `scale`.
    pub fn range(&self, scale: ac2_proto::model::LevelScale) -> Range {
        match scale {
            ac2_proto::model::LevelScale::Dbfs => self.level,
            ac2_proto::model::LevelScale::DbSpl => self.level_spl,
        }
    }

    pub fn range_mut(&mut self, scale: ac2_proto::model::LevelScale) -> &mut Range {
        match scale {
            ac2_proto::model::LevelScale::Dbfs => &mut self.level,
            ac2_proto::model::LevelScale::DbSpl => &mut self.level_spl,
        }
    }
}

impl Default for SpectrumView {
    fn default() -> Self {
        Self {
            level: Range::new(-100.0, 0.0),
            level_spl: Range::new(20.0, 120.0),
            mode: SpectrumMode::Spectrum,
            spectrograph: SpectrographView::default(),
        }
    }
}

/// IR display: each is a view of its own in the pane's view chain (G), in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum IrMode {
    /// Linear amplitude.
    #[default]
    Linear,
    /// 20·log10 |h|, re the peak.
    Log,
    /// Energy-time curve (published by the daemon), re its peak.
    Etc,
}

/// Which impulse-response picture a set of IR axes belongs to: each keeps its own zoom
/// and cursor, since the live IR (a few ms around the arrival) and a sweep's (hundreds of
/// ms of room decay, harmonics before t = 0) are read at very different scales.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrPane {
    /// The IR pane: the focused transfer measurement's IR.
    Live,
    /// The sweep pane's impulse-response view.
    Sweep,
}

/// The axes and the cursor of one impulse-response picture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IrAxes {
    /// Time range in ms re t = 0; `None` = the whole IR.
    pub time_ms: Option<Range>,
    /// Amplitude range of the linear view, FS; `None` = ±110 % of the IR's peak.
    pub amplitude: Option<Range>,
    /// Level range of the log and ETC views, dB re the peak.
    pub level_db: Range,
    /// The cursor's time, ms re t = 0.
    pub cursor_ms: Option<f64>,
}

impl IrAxes {
    /// 60 dB under the peak: a room's decay and the noise floor under it, the peak a little
    /// below the top.
    pub const DEFAULT_LEVEL_DB: Range = Range::new(-60.0, 3.0);
}

impl Default for IrAxes {
    fn default() -> Self {
        Self {
            time_ms: None,
            amplitude: None,
            level_db: Self::DEFAULT_LEVEL_DB,
            cursor_ms: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IrView {
    /// Shared by the IR pane and the sweep's IR view: one key steps both.
    pub mode: IrMode,
    /// The IR pane's axes and cursor.
    pub axes: IrAxes,
}

impl Default for IrView {
    fn default() -> Self {
        Self {
            mode: IrMode::Linear,
            axes: IrAxes::default(),
        }
    }
}

/// How distortion is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DistortionUnit {
    /// dB re the fundamental.
    Db,
    /// Percent of the fundamental.
    Percent,
}

/// The sweep (distortion) pane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DistortionView {
    pub unit: DistortionUnit,
    /// y range of the dB view.
    pub range_db: Range,
    /// The reference input's own orders drawn behind the measurement's, as context.
    pub reference: bool,
    /// The axes and cursor of its impulse-response view.
    pub ir: IrAxes,
}

impl Default for DistortionView {
    fn default() -> Self {
        Self {
            unit: DistortionUnit::Db,
            range_db: Range::new(-100.0, 0.0),
            reference: true,
            ir: IrAxes::default(),
        }
    }
}

/// The views of a sweep result, one at a time in the sweep pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SweepMode {
    /// The fundamental's response above, the harmonic distortion below.
    #[default]
    Response,
    /// The sweep's impulse response, drawn as this.
    Ir(IrMode),
    /// The ISO 3382-1 room parameters, the whole pane: read at a distance.
    Room,
}

impl SweepMode {
    /// G steps through these: the order a sweep is read in, from what the speaker does
    /// (response, then its impulse response linear, in dB, as energy) to what the room does.
    pub const ALL: [SweepMode; 5] = [
        SweepMode::Response,
        SweepMode::Ir(IrMode::Linear),
        SweepMode::Ir(IrMode::Log),
        SweepMode::Ir(IrMode::Etc),
        SweepMode::Room,
    ];

    /// G: the next view, the last back to the first.
    pub fn next(self) -> Self {
        step(&Self::ALL, self, 1)
    }

    /// Shift+G: the previous view.
    pub fn prev(self) -> Self {
        step(&Self::ALL, self, -1)
    }

    /// The impulse-response display, in an IR view.
    pub fn ir(self) -> Option<IrMode> {
        match self {
            SweepMode::Ir(m) => Some(m),
            _ => None,
        }
    }
}

/// `by` steps from `v` in the view chain `all`, wrapping at both ends.
pub fn step<T: Copy + PartialEq>(all: &[T], v: T, by: isize) -> T {
    let n = all.len() as isize;
    let i = all.iter().position(|x| *x == v).unwrap_or(0) as isize;
    all[(i + by).rem_euclid(n) as usize]
}

/// How the Leq windows are laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LeqStyle {
    /// One full-height bar per window, shortest left: read from a distance.
    #[default]
    Columns,
    /// A grid of tiles with every figure written out.
    Tiles,
}

/// The Leq view's layout: the windows' style and whether the history strip is under them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct LeqLayout {
    pub style: LeqStyle,
    pub history: bool,
}

/// What the SPL pane shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SplMode {
    /// The meter alone: its number, bar, statistics and calibration.
    Meter,
    /// The rolling Leq windows alone.
    Leq,
    /// The meter's number on top, the Leq windows below: the level now and the limits'
    /// state on one screen.
    #[default]
    MeterLeq,
    /// The band meter: eleven bars 20 … 200 Hz against their limits, the worst band named,
    /// the predicted LAeq at the place.
    Bands,
}

impl SplMode {
    /// G: meter → Leq windows → meter + Leq → (bands, when the meter has a band meter) →
    /// meter.
    pub fn next(self, bands: bool) -> Self {
        match self {
            SplMode::Meter => SplMode::Leq,
            SplMode::Leq => SplMode::MeterLeq,
            SplMode::MeterLeq if bands => SplMode::Bands,
            SplMode::MeterLeq | SplMode::Bands => SplMode::Meter,
        }
    }

    /// Shift+G: [`Self::next`] backwards.
    pub fn prev(self, bands: bool) -> Self {
        match self {
            SplMode::Meter if bands => SplMode::Bands,
            SplMode::Meter | SplMode::Bands => SplMode::MeterLeq,
            SplMode::Leq => SplMode::Meter,
            SplMode::MeterLeq => SplMode::Leq,
        }
    }

    /// The view with the Leq windows on screen: as it is when they are, else the meter
    /// with the windows under it (the meter stays where it was asked for).
    pub fn with_leq(self) -> Self {
        match self {
            SplMode::Meter | SplMode::Bands => SplMode::MeterLeq,
            m => m,
        }
    }

    /// Whether the Leq windows are on screen.
    pub fn shows_leq(self) -> bool {
        matches!(self, SplMode::Leq | SplMode::MeterLeq)
    }
}

/// The SPL pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SplView {
    pub layout: LeqLayout,
}

/// How a pane draws: its plot furniture and the colours and sizes it draws in.
#[derive(Clone, Copy, Debug)]
pub struct Style<'a> {
    pub chrome: PlotChrome,
    pub theme: &'a crate::theme::Theme,
}

/// How much of a plot's furniture a pane draws around its traces. Fewer lines let a
/// screenshot or a crowded overlay read as curves alone; the axes stay as they are, so
/// nothing about the picture's scale changes between the steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PlotChrome {
    /// Grid, axis labels and the cursor.
    #[default]
    Full,
    /// Axis labels and the cursor; no grid lines, resolution marker or window changes.
    NoGrid,
    /// The traces alone: no grid, no axis labels, no cursor.
    Bare,
}

impl PlotChrome {
    /// T: full → no grid → traces only → full.
    pub fn next(self) -> Self {
        match self {
            Self::Full => Self::NoGrid,
            Self::NoGrid => Self::Bare,
            Self::Bare => Self::Full,
        }
    }

    /// What the operator reads when stepping (toast, help).
    pub fn name(self) -> &'static str {
        match self {
            Self::Full => "grid, labels and cursor",
            Self::NoGrid => "no grid or marks",
            Self::Bare => "traces only",
        }
    }

    pub fn grid(self) -> bool {
        self == Self::Full
    }

    /// Tick labels and the axis title.
    pub fn labels(self) -> bool {
        self != Self::Bare
    }

    /// The cursor line and its readout.
    pub fn cursor(self) -> bool {
        self != Self::Bare
    }

    /// The spectrum's peak list and the rank beside each mark; the marks themselves are on
    /// the curve and stay with it, like the traces.
    pub fn peak_list(self) -> bool {
        self != Self::Bare
    }

    /// The SPL views' state words (`OK`, `NEAR`, `OVER`) and limit texts (`stay ≤ … dB`,
    /// `limit … dB`). Read across a room, a bar's colour and its limit line already say
    /// the state, so the words go with the grid; every other word (on course, cooling
    /// down, not calibrated, the values and window names) stays.
    pub fn limit_text(self) -> bool {
        self == Self::Full
    }

    /// [`Self::name`] on an SPL pane: the grid is the bars' 10 dB lines, the labels their
    /// scale's numbers, and the step past full takes the limit text too.
    pub fn spl_name(self) -> &'static str {
        match self {
            Self::Full => "grid, scale and limit text",
            Self::NoGrid => "no grid or limit text",
            Self::Bare => "no grid, scale or limit text",
        }
    }
}

/// Everything the operator chose about the view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewState {
    /// Shared log-frequency axis (TF and spectrum).
    pub freq: FreqRange,
    pub tf: TfView,
    pub spectrum: SpectrumView,
    pub ir: IrView,
    pub distortion: DistortionView,
    pub spl: SplView,
    /// Comparison cursor, Hz; synchronised across traces and panes.
    pub cursor_hz: Option<f64>,
    /// The cursor's mark, Hz: while set, the cursor reads how it differs from there.
    pub cursor_mark_hz: Option<f64>,
    /// How far a peak or notch must stand out for the cursor to jump to it, dB
    /// ([`crate::cursor_jump`]).
    pub jump_prominence_db: f64,
    /// Air temperature for the delay → distance readout (decision A).
    pub temperature_c: f64,
    /// Grid, labels and cursor of the pane being drawn.
    pub chrome: PlotChrome,
    /// Mark the frequency ranges a curve's analysis does not resolve at its grid
    /// ([`crate::unresolved`]).
    pub unresolved: bool,
    /// Least right margin of a frequency plot, logical pixels: set by the pane layout (not
    /// the operator) to the widest margin among the panes stacked in this pane's column, so
    /// they all put one frequency at one x ([`crate::freq_column`]). 0 keeps the plot's own.
    pub freq_right: f32,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            freq: FreqRange::default(),
            tf: TfView::default(),
            spectrum: SpectrumView::default(),
            ir: IrView::default(),
            distortion: DistortionView::default(),
            spl: SplView::default(),
            cursor_hz: None,
            cursor_mark_hz: None,
            jump_prominence_db: crate::cursor_jump::DEFAULT_PROMINENCE_DB,
            temperature_c: 20.0,
            chrome: PlotChrome::Full,
            unresolved: true,
            freq_right: 0.0,
        }
    }
}

impl ViewState {
    /// Whether the resolution marker and a multi-resolution spectrum's window changes are
    /// drawn: they read as grid lines, so T takes them away with the grid.
    pub fn marks(&self) -> bool {
        self.unresolved && self.chrome.grid()
    }

    /// The axes of IR picture `p`.
    pub fn ir_axes(&self, p: IrPane) -> &IrAxes {
        match p {
            IrPane::Live => &self.ir.axes,
            IrPane::Sweep => &self.distortion.ir,
        }
    }

    pub fn ir_axes_mut(&mut self, p: IrPane) -> &mut IrAxes {
        match p {
            IrPane::Live => &mut self.ir.axes,
            IrPane::Sweep => &mut self.distortion.ir,
        }
    }
}

#[cfg(test)]
mod tests;
