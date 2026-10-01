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
    pub level: Range,
    pub peak_hold: bool,
}

impl Default for SpectrumView {
    fn default() -> Self {
        Self {
            style: SpectrumStyle::Bars,
            level: Range::new(-100.0, 0.0),
            peak_hold: false,
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

/// Everything the operator chose about the view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewState {
    /// Shared log-frequency axis (TF and spectrum).
    pub freq: FreqRange,
    pub tf: TfView,
    pub spectrum: SpectrumView,
    pub ir: IrView,
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
            cursor_hz: None,
            temperature_c: 20.0,
        }
    }
}

#[cfg(test)]
mod tests {
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
}
