//! The range across positions of an average math channel (`docs/design/spatial-walk.md` §10).

use serde::{Deserialize, Serialize};

/// What an average's range band spans. With few positions a percentile is no more than the
/// extremes it would interpolate between, so below [`RangeSpread::PERCENTILE_FROM`] the band
/// is the plain envelope; from there on one stray seat no longer sets its edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RangeSpread {
    /// Fewer than [`RangeSpread::MIN_POSITIONS`] positions went in: no band.
    TooFew,
    /// Lowest to highest level of the positions per column.
    Envelope {
        /// Positions the band spans.
        positions: u32,
    },
    /// 10th to 90th percentile of the positions' levels per column.
    Percentiles {
        /// Positions the band spans.
        positions: u32,
    },
}

impl RangeSpread {
    /// Fewest positions with a band: two levels are a difference, not a spread.
    pub const MIN_POSITIONS: usize = 3;
    /// Positions from which the band is the 10th–90th percentile.
    pub const PERCENTILE_FROM: usize = 6;
    /// The percentiles of [`RangeSpread::Percentiles`].
    pub const PERCENTILES: (f64, f64) = (0.1, 0.9);

    /// The spread of `positions` operands.
    pub fn of(positions: usize) -> Self {
        let p = u32::try_from(positions).unwrap_or(u32::MAX);
        if positions < Self::MIN_POSITIONS {
            Self::TooFew
        } else if positions < Self::PERCENTILE_FROM {
            Self::Envelope { positions: p }
        } else {
            Self::Percentiles { positions: p }
        }
    }
}

/// A range band per column of the frame's grid (NaN: not in the band).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RangeBand {
    /// Lower edge, dB.
    pub lo: Vec<f32>,
    /// Upper edge, dB.
    pub hi: Vec<f32>,
    /// The lowest γ² among the positions; NaN where a position has no coherence. A display
    /// leaves out a column whose value its coherence mask would blank on a single curve.
    pub coh: Vec<f32>,
}
