//! View state: what the operator chose to look at (ranges, modes, cursor), as plain data.
//! Builders read it; the UI owns and edits it (zoom, pan, toggles).

use crate::axis::Range;
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

/// Navigation of a vertical dB axis (transfer magnitude, spectrum level, distortion): zoom
/// about a level, pan by dB, frame the shown data. The axis keeps its tick rules
/// ([`crate::axis::linear_ticks`]), which label any range from a tenth of a dB to hundreds.
pub mod level {
    use super::{LEVEL_FIT_MIN_SPAN, LEVEL_LIMIT_HI, LEVEL_LIMIT_LO, LEVEL_MIN_SPAN};
    use crate::axis::Range;

    /// The range kept within the limits, its span between [`LEVEL_MIN_SPAN`] and the
    /// limits' span; an invalid range is returned as it is.
    pub fn clamp(r: Range) -> Range {
        if !r.is_valid() {
            return r;
        }
        let span = r
            .span()
            .clamp(LEVEL_MIN_SPAN, LEVEL_LIMIT_HI - LEVEL_LIMIT_LO);
        let mid = (r.lo + r.hi) / 2.0;
        let lo = (mid - span / 2.0).clamp(LEVEL_LIMIT_LO, LEVEL_LIMIT_HI - span);
        Range::new(lo, lo + span)
    }

    /// Zoom by `factor` (> 1 zooms in) keeping `about` at the same height.
    pub fn zoom(r: Range, about: f64, factor: f64) -> Range {
        if !(factor > 0.0 && factor.is_finite() && about.is_finite() && r.is_valid()) {
            return r;
        }
        let span = (r.span() / factor).clamp(LEVEL_MIN_SPAN, LEVEL_LIMIT_HI - LEVEL_LIMIT_LO);
        let t = ((about - r.lo) / r.span()).clamp(0.0, 1.0);
        let lo = about - t * span;
        clamp(Range::new(lo, lo + span))
    }

    /// Pan by `db` (positive shows higher levels), keeping the span.
    pub fn pan(r: Range, db: f64) -> Range {
        if !(db.is_finite() && r.is_valid()) {
            return r;
        }
        clamp(Range::new(r.lo + db, r.hi + db))
    }

    /// The 1-2-5 step one key press pans by: about a tenth of the span, so a few presses
    /// move a trace across the pane and the grid lines stay on round values.
    pub fn pan_step(r: Range) -> f64 {
        nice(r.span() / 10.0)
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
        }
    }
}

/// Bars or a line for spectrum / RTA traces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpectrumStyle {
    Bars,
    Line,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpectrumView {
    pub style: SpectrumStyle,
    /// Level axis while the pane shows dBFS.
    pub level: Range,
    /// Level axis while the pane shows dB SPL (calibrated): the same zoom and pan keys act on
    /// it, and each scale keeps its own range, so calibrating doesn't push the curves off
    /// a dBFS-sized axis.
    pub level_spl: Range,
    pub peak_hold: bool,
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

    /// The spectrograph is drawn (and its history kept).
    pub fn spectrograph(self) -> bool {
        self != Self::Spectrum
    }
}

/// The history lengths the spectrograph steps through, seconds.
pub const SPECTROGRAPH_SPANS_S: [u32; 4] = [10, 30, 60, 120];

/// The spectrograph: how much history it keeps, and the time of the cursor in it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpectrographView {
    /// History shown, seconds (one of [`SPECTROGRAPH_SPANS_S`]).
    pub span_s: u32,
    /// The cursor's time, seconds before the newest frame (with `cursor_hz`, a point).
    pub cursor_s: Option<f64>,
}

impl Default for SpectrographView {
    fn default() -> Self {
        Self {
            span_s: 30,
            cursor_s: None,
        }
    }
}

impl SpectrographView {
    /// The next history length, wrapping to the shortest.
    pub fn next_span(span_s: u32) -> u32 {
        SPECTROGRAPH_SPANS_S
            .iter()
            .copied()
            .find(|s| *s > span_s)
            .unwrap_or(SPECTROGRAPH_SPANS_S[0])
    }
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
            style: SpectrumStyle::Bars,
            level: Range::new(-100.0, 0.0),
            level_spl: Range::new(20.0, 120.0),
            peak_hold: false,
            mode: SpectrumMode::Spectrum,
            spectrograph: SpectrographView::default(),
        }
    }
}

/// IR display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrMode {
    /// Linear amplitude.
    Linear,
    /// 20·log10 |h|, re the peak.
    Log,
    /// Energy-time curve (published by the daemon), re its peak.
    Etc,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IrView {
    pub mode: IrMode,
    /// Time range in ms re the inserted delay; `None` = the whole published IR.
    pub time_ms: Option<Range>,
    /// Depth of the log / ETC views below the peak, dB.
    pub log_depth_db: f64,
}

impl Default for IrView {
    fn default() -> Self {
        Self {
            mode: IrMode::Linear,
            time_ms: None,
            log_depth_db: 60.0,
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
    /// The sweep's impulse response instead of its distortion.
    pub show_ir: bool,
}

impl Default for DistortionView {
    fn default() -> Self {
        Self {
            unit: DistortionUnit::Db,
            range_db: Range::new(-100.0, 0.0),
            show_ir: false,
        }
    }
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
}

impl SplMode {
    /// G: meter → Leq windows → meter + Leq → meter.
    pub fn next(self) -> Self {
        match self {
            SplMode::Meter => SplMode::Leq,
            SplMode::Leq => SplMode::MeterLeq,
            SplMode::MeterLeq => SplMode::Meter,
        }
    }

    /// The view with the Leq windows on screen: as it is when they are, else the meter
    /// with the windows under it (the meter stays where it was asked for).
    pub fn with_leq(self) -> Self {
        match self {
            SplMode::Meter => SplMode::MeterLeq,
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
    pub mode: SplMode,
    pub layout: LeqLayout,
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
    /// Air temperature for the delay → distance readout (decision A).
    pub temperature_c: f64,
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
            temperature_c: 20.0,
        }
    }
}

#[cfg(test)]
mod tests {

    /// dBFS and dB SPL keep their own level ranges: a calibrated spectrum starts on a
    /// dB SPL-sized axis, and zooming one scale leaves the other as it was.
    #[test]
    fn spectrum_level_range_per_scale() {
        use ac2_proto::model::LevelScale;
        let mut v = SpectrumView::default();
        assert_eq!(v.range(LevelScale::Dbfs), Range::new(-100.0, 0.0));
        assert_eq!(v.range(LevelScale::DbSpl), Range::new(20.0, 120.0));
        *v.range_mut(LevelScale::DbSpl) = Range::new(40.0, 100.0);
        assert_eq!(v.range(LevelScale::Dbfs), Range::new(-100.0, 0.0));
        assert_eq!(v.range(LevelScale::DbSpl), Range::new(40.0, 100.0));
    }

    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * a.abs().max(b.abs())
    }

    #[test]
    fn zoom_keeps_the_anchor() {
        let r = FreqRange::default().zoom(1000.0, 2.0);
        // 1 kHz sits at the same fraction of the log axis before and after.
        let t = |r: FreqRange| (1000f64.ln() - r.lo.ln()) / (r.hi.ln() - r.lo.ln());
        assert!((t(r) - t(FreqRange::default())).abs() < 1e-12);
        assert!(close(r.hi / r.lo, 1000f64.sqrt()));
    }

    #[test]
    fn zoom_and_pan_are_clamped() {
        let r = FreqRange::default().zoom(1000.0, 1e9);
        assert!(close(r.hi / r.lo, FREQ_MIN_RATIO));
        let r = FreqRange::default().zoom(1000.0, 1e-9);
        assert!(close(r.lo, FREQ_LIMIT_LO) && close(r.hi, FREQ_LIMIT_HI));
        let r = FreqRange::default().pan(20.0);
        assert!(close(r.hi, FREQ_LIMIT_HI));
        assert!(close(r.hi / r.lo, 1000.0));
        let r = FreqRange::default().pan(1.0);
        assert!(close(r.lo, 40.0) && close(r.hi, 40_000.0));
    }

    #[test]
    fn level_zoom_keeps_the_anchor_and_limits() {
        let r = Range::new(-100.0, 0.0);
        let z = level::zoom(r, -80.0, 2.0);
        assert!(close(z.span(), 50.0));
        // −80 dB stays a fifth of the way up.
        assert!(close((-80.0 - z.lo) / z.span(), 0.2));
        let back = level::zoom(z, -80.0, 0.5);
        assert!(
            close(back.lo, r.lo) && close(back.hi + 1.0, r.hi + 1.0),
            "{back:?}"
        );
        // The narrowest span and the limits hold however far it goes.
        let z = level::zoom(r, -50.0, 1e9);
        assert!(close(z.span(), LEVEL_MIN_SPAN));
        let z = level::zoom(r, -50.0, 1e-9);
        assert_eq!(z, Range::new(LEVEL_LIMIT_LO, LEVEL_LIMIT_HI));
        // Nonsense leaves it alone.
        assert_eq!(level::zoom(r, f64::NAN, 2.0), r);
        assert_eq!(level::zoom(r, -50.0, 0.0), r);
    }

    #[test]
    fn level_pan_steps_on_round_values() {
        let r = Range::new(-100.0, 0.0);
        assert_eq!(level::pan_step(r), 10.0);
        assert_eq!(level::pan_step(Range::new(-30.0, 30.0)), 10.0);
        assert_eq!(level::pan_step(Range::new(-12.0, 0.0)), 2.0);
        assert!(close(level::pan_step(Range::new(-1.0, 0.0)), 0.1));
        assert_eq!(level::pan(r, -40.0), Range::new(-140.0, -40.0));
        // Against a limit the span stays.
        assert_eq!(level::pan(r, -1000.0), Range::new(-300.0, -200.0));
        assert_eq!(level::pan(r, 1000.0), Range::new(200.0, 300.0));
    }

    #[test]
    fn level_fit_frames_low_signals() {
        // A spectrum of very low levels: noise near −135 dBFS, a tone at −82 dBFS, and a few
        // empty bins far below that a fit must not stretch to.
        let mut v: Vec<f64> = (0..2000).map(|i| -135.0 + f64::from(i % 7)).collect();
        v.push(-82.0);
        v.extend([-300.0, f64::NEG_INFINITY, f64::NAN, -280.0]);
        let r = level::fit(v).expect("finite values");
        assert!(r.lo <= -135.0 && r.lo >= -150.0, "{r:?}");
        assert!(r.hi >= -82.0 && r.hi <= -70.0, "{r:?}");
        // On the grid: whole multiples of the step it picked (5 dB here).
        assert_eq!((r.lo % 5.0, r.hi % 5.0), (0.0, 0.0), "{r:?}");
        // A flat trace still gets a few dB.
        let r = level::fit([3.0; 10]).expect("finite");
        assert!(
            r.span() >= LEVEL_FIT_MIN_SPAN && r.lo < 3.0 && r.hi > 3.0,
            "{r:?}"
        );
        assert_eq!(level::fit([f64::NAN]), None);
        assert_eq!(level::fit(std::iter::empty()), None);
    }

    #[test]
    fn level_ticks_are_readable_at_any_span() {
        use crate::axis::{Steps, axis_with_density};
        let labels = |r: Range| {
            axis_with_density(r, 200.0, 0.0, "dB", Steps::Decimal, 200.0)
                .labels()
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        // A 1 dB span: tenths, with a decimal.
        let l = labels(Range::new(-1.0, 0.0));
        assert!(l.contains(&"\u{2212}0.4".to_owned()), "{l:?}");
        assert!((4..=9).contains(&l.len()), "{l:?}");
        // Very low levels: whole dB at a round step.
        let l = labels(Range::new(-145.0, -75.0));
        assert!(l.contains(&"\u{2212}100".to_owned()), "{l:?}");
        assert!(l.iter().all(|s| !s.contains('.')), "{l:?}");
        // The whole limit span: a few labels, hundreds apart.
        let l = labels(Range::new(LEVEL_LIMIT_LO, LEVEL_LIMIT_HI));
        assert!(l.contains(&"0".to_owned()) && l.len() <= 8, "{l:?}");
    }
}
