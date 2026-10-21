//! The alignment of two transfer functions in their overlap (`docs/design/alignment.md`):
//! the band asked for, and what a frame of an alignment math channel found.

use serde::{Deserialize, Serialize};

use crate::units::{Hz, Seconds};

/// The overlap two systems are aligned in: where both play (a crossover region, where a fill
/// meets the main).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlignBand {
    /// Lowest frequency.
    pub lo: Hz,
    /// Highest frequency.
    pub hi: Hz,
}

impl AlignBand {
    /// Whether the band is one: finite, `0 < lo < hi`.
    pub fn is_valid(self) -> bool {
        self.lo.0.is_finite() && self.hi.0.is_finite() && self.lo.0 > 0.0 && self.lo.0 < self.hi.0
    }
}

/// Default largest delay searched: sound travels about 17 m in 50 ms, as far as a fill or
/// a near delay tower is from the main.
pub const ALIGN_MAX_DELAY_DEFAULT: Seconds = Seconds(0.05);
/// Largest delay an alignment searches.
pub const ALIGN_MAX_DELAY_LIMIT: Seconds = Seconds(1.0);

/// Which of the two operands is delayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlignSide {
    /// The first operand, A.
    A,
    /// The second operand, B.
    B,
}

/// One way to align the two: delay `delayed` by `delay`, inverted or not.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlignCandidate {
    /// The delay to add, ≥ 0.
    pub delay: Seconds,
    /// To which of the two.
    pub delayed: AlignSide,
    /// The delayed one's polarity inverted.
    pub inverted: bool,
    /// Weighted RMS of the phase difference left over the band, degrees.
    pub residual_deg: f64,
    /// Weighted mean cosine of that phase difference (1: in phase across the band).
    pub score: f64,
}

/// How fully the two add over the band: `20·lg(|A + B| / (|A| + |B|))`, 0 dB fully, very
/// negative where they cancel.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Summation {
    /// Weighted mean over the band, dB.
    pub mean_db: f64,
    /// Its lowest column, dB.
    pub worst_db: f64,
}

/// The alignment found.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlignFit {
    /// The best alignment: the one the channel's curve applies.
    pub best: AlignCandidate,
    /// The best one of the other polarity.
    pub other_polarity: Option<AlignCandidate>,
    /// Another wrap of the best's polarity that fits nearly as well (the band is too narrow
    /// to tell them apart).
    pub runner_up: Option<AlignCandidate>,
    /// Summation as measured.
    pub before: Summation,
    /// Summation with `best` applied.
    pub after: Summation,
    /// Columns of the band that took part.
    pub columns: u32,
}

/// Why a frame has no alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AlignNoFit {
    /// The two operands are not both in.
    FewOperands,
    /// Too few columns with a value in both within the band.
    FewColumns {
        /// Columns found.
        found: u32,
    },
    /// No column carries weight: no coherence, or one of them silent across the band.
    NoWeight,
}

/// A frame's fit, or why it has none.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AlignOutcome {
    /// Found.
    Fit {
        /// What was found.
        fit: AlignFit,
    },
    /// None.
    NoFit {
        /// Why.
        why: AlignNoFit,
    },
}

/// What an alignment math channel's frame found, beside its curve (the aligned sum). The
/// alignment view's curves travel as the frame's arrays ([`crate::frame::TfFrame::align`]).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlignState {
    /// The fit, or why there is none.
    pub outcome: AlignOutcome,
}

/// An alignment view's curves, per column of the frame's grid (NaN: no value).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AlignCurves {
    /// The phase difference A − B as measured, degrees (−180 … 180).
    pub phase_before: Vec<f32>,
    /// The phase difference with the fit applied, degrees.
    pub phase_after: Vec<f32>,
    /// The summation as measured, dB.
    pub sum_before: Vec<f32>,
    /// The summation with the fit applied, dB.
    pub sum_after: Vec<f32>,
}
