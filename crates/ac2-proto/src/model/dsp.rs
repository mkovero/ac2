//! DSP configuration enums: band and smoothing fractions, weightings, windows, averaging (mirrors of ac2-core).

use serde::{Deserialize, Serialize};

use crate::units::{Hz, Seconds, TraceId};

/// Fractional-octave band designator for RTA bands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BandFraction {
    /// 1/1 octave.
    Octave,
    /// 1/3 octave.
    Third,
    /// 1/6 octave.
    Sixth,
    /// 1/12 octave.
    Twelfth,
    /// 1/24 octave.
    TwentyFourth,
}

impl BandFraction {
    /// The designator b of 1/b octave.
    pub fn b(self) -> u32 {
        match self {
            Self::Octave => 1,
            Self::Third => 3,
            Self::Sixth => 6,
            Self::Twelfth => 12,
            Self::TwentyFourth => 24,
        }
    }
}

/// Fractional-octave smoothing bandwidth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmoothingFraction {
    /// 1/3 octave.
    Third,
    /// 1/6 octave.
    Sixth,
    /// 1/12 octave.
    Twelfth,
    /// 1/24 octave.
    TwentyFourth,
    /// 1/48 octave.
    FortyEighth,
}

impl SmoothingFraction {
    /// Every bandwidth, narrowest first.
    pub const ALL: [SmoothingFraction; 5] = [
        Self::FortyEighth,
        Self::TwentyFourth,
        Self::Twelfth,
        Self::Sixth,
        Self::Third,
    ];

    /// The designator b of 1/b octave.
    pub fn b(self) -> u32 {
        match self {
            Self::Third => 3,
            Self::Sixth => 6,
            Self::Twelfth => 12,
            Self::TwentyFourth => 24,
            Self::FortyEighth => 48,
        }
    }
}

/// Points per octave a transfer function or sweep stores: its column grid. Not smoothing,
/// which is a display edit over these columns; a finer resolution only adds columns where
/// the analysis resolves them ([`Unresolved`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    /// 12 points per octave.
    Twelfth,
    /// 24 points per octave.
    TwentyFourth,
    /// 48 points per octave.
    #[default]
    FortyEighth,
    /// 96 points per octave.
    NinetySixth,
}

impl Resolution {
    /// Every resolution, coarsest first.
    pub const ALL: [Resolution; 4] = [
        Self::Twelfth,
        Self::TwentyFourth,
        Self::FortyEighth,
        Self::NinetySixth,
    ];

    /// Points per octave.
    pub fn ppo(self) -> u32 {
        match self {
            Self::Twelfth => 12,
            Self::TwentyFourth => 24,
            Self::FortyEighth => 48,
            Self::NinetySixth => 96,
        }
    }

    /// The resolution of `ppo` points per octave, if it is one.
    pub fn from_ppo(ppo: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.ppo() == ppo)
    }
}

/// A frequency range, `lo < hi`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreqRange {
    /// Lower edge.
    pub lo: Hz,
    /// Upper edge.
    pub hi: Hz,
}

/// Where a curve stored at `resolution` has columns closer together than its analysis
/// resolves: there one estimate's resolution cell (the bandwidth its window sets) spans
/// more than one column, so the extra columns are interpolation, not detail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unresolved {
    /// The grid the ranges were found on.
    pub resolution: Resolution,
    /// Column-edge ranges, ascending and disjoint; empty: every column is resolved.
    pub ranges: Vec<FreqRange>,
}

/// What smoothing averages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmoothingMode {
    /// Power-average magnitude; phase as measured.
    Magnitude,
    /// Power-average magnitude and average the phase, unwrapped within each run of valid
    /// columns. What every front end sets.
    MagnitudePhase,
}

impl Smoothing {
    /// `fraction` in the mode front ends set: magnitude and phase.
    pub fn of(fraction: SmoothingFraction) -> Self {
        Self {
            fraction,
            mode: SmoothingMode::MagnitudePhase,
        }
    }
}

/// Smoothing applied to a transfer function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Smoothing {
    /// Bandwidth.
    pub fraction: SmoothingFraction,
    /// Mode.
    pub mode: SmoothingMode,
}

/// IEC 61672-1 frequency weighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Weighting {
    /// A.
    A,
    /// C.
    C,
    /// Z (flat).
    Z,
}

/// Exponential time weighting of a sound level meter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeWeighting {
    /// 125 ms.
    Fast,
    /// 1 s.
    Slow,
    /// 35 ms rise, 1.5 s fall.
    Impulse,
}

/// Weighting of the peak detector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeakWeighting {
    /// C-weighted peak (LCpeak).
    C,
    /// Unweighted peak.
    Z,
}

/// Unit of level values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LevelScale {
    /// dBFS.
    Dbfs,
    /// dB SPL (calibrated).
    DbSpl,
}

/// FFT window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Window {
    /// Hann.
    Hann,
    /// 4-term Blackman-Harris.
    BlackmanHarris4,
    /// HFT95 flat-top.
    FlatTop,
    /// No window.
    Rectangular,
}

/// Transfer-function averaging of the MTW ladder.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TfAveraging {
    /// Mean of the most recent blocks of the full-rate stage.
    Fifo {
        /// Blocks held by the full-rate stage.
        blocks: u32,
    },
    /// Exponential mean.
    Exponential {
        /// Time constant of the full-rate stage.
        time_constant: Seconds,
    },
}

/// Spectrum / RTA averaging (on power).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SpecAveraging {
    /// Each frame replaces the previous one.
    Off,
    /// Mean of the last `frames` frames (a spectrum computes one per hop: `n / 8` for long
    /// FFTs, a 1024-sample hop at 48 kHz for short ones; never more than `n / 2`. An RTA's
    /// frame is one result interval, about 1/60 s, each weighted by its duration). At most
    /// [`SpecAveraging::MAX_SPECTRUM_FIFO_VALUES`] stored values for a spectrum
    /// (`frames · (n/2 + 1)`), [`SpecAveraging::MAX_RTA_FIFO_FRAMES`] frames for an RTA.
    Fifo {
        /// Frames.
        frames: u32,
    },
    /// Exponential mean.
    Exponential {
        /// Time constant.
        time_constant: Seconds,
    },
}

impl SpecAveraging {
    /// A spectrum FIFO stores every frame's bins: `frames · (n/2 + 1)` at most this many
    /// (128 MiB of f64), e.g. 511 frames of a 65536-point FFT, 8188 of a 4096-point one.
    pub const MAX_SPECTRUM_FIFO_VALUES: u64 = 1 << 24;
    /// An RTA FIFO's frames at most: about 18 minutes of result intervals at 60 per second.
    pub const MAX_RTA_FIFO_FRAMES: u32 = 1 << 16;
}

/// How traces are combined by `trace.average`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AverageMethod {
    /// RMS magnitude; phase of the complex mean.
    Power,
    /// Complex mean.
    Complex,
    /// Coherence-weighted complex mean.
    CoherenceWeighted,
}

/// Delay the averaged phase is referred to.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DelayReference {
    /// The measured delay of one input trace.
    Trace {
        /// That trace.
        trace: TraceId,
    },
    /// An explicit delay.
    Fixed {
        /// Delay.
        delay: Seconds,
    },
}
