//! How a spectrum or RTA measurement's curves and spectrograph are drawn: kept with the
//! measurement on the daemon, so every client draws it the same and a session restores it.

use serde::{Deserialize, Serialize};

/// How spectrum / RTA curves are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpectrumStyle {
    /// The max-per-pixel line.
    Line,
    /// The line over a faint area down to the plot bottom.
    Fill,
    /// Band power as bars (RTA curves); narrowband curves draw as `Line`.
    #[default]
    Bars,
}

impl SpectrumStyle {
    /// The next style in the B cycle: line → fill → bars → line.
    pub fn next(self) -> Self {
        match self {
            Self::Line => Self::Fill,
            Self::Fill => Self::Bars,
            Self::Bars => Self::Line,
        }
    }

    /// What toasts and captions call it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Line => "line",
            Self::Fill => "fill",
            Self::Bars => "bars",
        }
    }
}

/// Colour lookup for spectrograph values. Every map rises monotonically in lightness, so a
/// louder cell always reads brighter, and none relies on telling red from green.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Colormap {
    /// Dark blue through green to yellow: the most hues, so the finest level steps.
    #[default]
    Viridis,
    /// Black through purple and orange to pale yellow: dark for most of its range.
    Magma,
    /// Black through purple and red to pale yellow: like magma, more saturated.
    Inferno,
    /// Dark blue through grey to yellow: the same for normal and red-green-deficient vision.
    Cividis,
    /// Black to light grey; stops short of white, which glares in a dark room.
    Grey,
}

impl Colormap {
    /// In the order ←/→ and the palette step through them.
    pub const ALL: [Colormap; 5] = [
        Colormap::Viridis,
        Colormap::Magma,
        Colormap::Inferno,
        Colormap::Cividis,
        Colormap::Grey,
    ];

    /// What the settings, the palette and the toasts call it.
    pub fn name(self) -> &'static str {
        match self {
            Colormap::Viridis => "viridis",
            Colormap::Magma => "magma",
            Colormap::Inferno => "inferno",
            Colormap::Cividis => "cividis",
            Colormap::Grey => "grey",
        }
    }

    /// `d` places on in [`Colormap::ALL`], wrapping.
    pub fn step(self, d: i32) -> Colormap {
        let n = Self::ALL.len() as i32;
        let i = Self::ALL.iter().position(|m| *m == self).unwrap_or(0) as i32;
        Self::ALL[(i + d).rem_euclid(n) as usize]
    }
}

/// What P draws over a spectrum or RTA measurement's live curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeakMode {
    /// The live curve alone.
    #[default]
    Off,
    /// The highest level each column has reached.
    Hold,
    /// The peak hold, and the highest peaks of the live curve marked and listed.
    Track,
}

impl PeakMode {
    /// P: off → hold → hold + tracking → off.
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Hold,
            Self::Hold => Self::Track,
            Self::Track => Self::Off,
        }
    }

    /// The peak-hold line is drawn.
    pub fn holds(self) -> bool {
        self != Self::Off
    }

    /// Peaks are marked and listed.
    pub fn tracks(self) -> bool {
        self == Self::Track
    }

    /// What toasts and the CLI call it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "peak hold off",
            Self::Hold => "peak hold",
            Self::Track => "peak hold + peak tracking",
        }
    }
}

/// How a spectrum or RTA measurement is drawn (`meas.update`): only what clients draw, so
/// changing it leaves the job, its averages and its frames' config rev as they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpectrumDisplay {
    /// Curve style (B).
    pub style: SpectrumStyle,
    /// What P draws over the live curve; the hold and the tracking are each client's.
    pub peak: PeakMode,
    /// Spectrograph history shown, seconds: one of [`SpectrumDisplay::SPANS_S`].
    pub spectrograph_span_s: u32,
    /// Spectrograph colour map.
    pub spectrograph_colours: Colormap,
}

impl SpectrumDisplay {
    /// Spectrograph spans Shift+B steps through, seconds.
    pub const SPANS_S: [u32; 4] = [10, 30, 60, 120];
    /// Default spectrograph span, seconds.
    pub const DEFAULT_SPAN_S: u32 = 30;

    /// Whether the span is one clients keep history for.
    pub fn span_ok(self) -> bool {
        Self::SPANS_S.contains(&self.spectrograph_span_s)
    }

    /// The span after `span_s` in [`Self::SPANS_S`], wrapping.
    pub fn next_span(span_s: u32) -> u32 {
        Self::SPANS_S
            .iter()
            .copied()
            .find(|s| *s > span_s)
            .unwrap_or(Self::SPANS_S[0])
    }
}

impl Default for SpectrumDisplay {
    fn default() -> Self {
        Self {
            style: SpectrumStyle::default(),
            peak: PeakMode::Off,
            spectrograph_span_s: Self::DEFAULT_SPAN_S,
            spectrograph_colours: Colormap::default(),
        }
    }
}
