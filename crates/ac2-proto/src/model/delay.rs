//! Delay finder: picks, bands, arrivals, outcomes, findings and a transfer measurement's delay state.

use serde::{Deserialize, Serialize};

use crate::units::{Db, Degrees, Hz, Samples, Seconds, WallNs};

/// Which delay-finder result to insert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DelayPick {
    /// The first-arrival rule's pick (decision 1a): the accepted first arrival, or the
    /// pre-selected `ranked[0]` of an ambiguous finding (decision 1c: one key accepts it).
    FirstArrival,
    /// Strongest peak.
    Strongest,
    /// Entry `index` of an ambiguous finding's ranked list (decision 1c: another key picks).
    Ranked {
        /// Index into the `ranked` list of [`DelayOutcome::Ambiguous`].
        index: u8,
    },
}

/// Delay-finder analysis band requested by `delay.find`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum FinderBand {
    /// 2 kHz – 16 kHz: full-range boxes.
    Full,
    /// 300 Hz – 3 kHz.
    Mid,
    /// 20 Hz – 120 Hz (decision 1e).
    Sub,
    /// Operator edges (−6 dB points).
    Custom {
        /// Lower edge.
        lo_hz: Hz,
        /// Upper edge.
        hi_hz: Hz,
    },
    /// Full → mid → sub: the first band that is not refused, from the measured excitation.
    Auto,
}

/// The band a finding was made in (the resolved one for an `auto` request).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DelayBand {
    /// 2 kHz – 16 kHz.
    Full,
    /// 300 Hz – 3 kHz.
    Mid,
    /// 20 Hz – 120 Hz.
    Sub,
    /// Operator edges.
    Custom {
        /// Lower edge.
        lo_hz: Hz,
        /// Upper edge.
        hi_hz: Hz,
    },
}

/// One arrival found by the delay finder.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelayArrival {
    /// Delay of the measurement re the reference.
    pub delay: Seconds,
    /// The same delay in (fractional) samples at the session rate.
    pub delay_samples: f64,
    /// Level relative to the strongest arrival.
    pub level: Db,
    /// Analytic phase: ≈ 0 in phase, ≈ ±180 inverted, otherwise dispersive.
    pub phase: Degrees,
    /// 1-σ timing uncertainty, samples.
    pub uncertainty_samples: f64,
    /// Relative RMS lobe-shape misfit against the pulse model (0 outside refinement).
    pub misfit: f64,
    /// Measured inside a refinement window (otherwise acquisition evidence only).
    pub refined: bool,
}

/// Why the finder gives no estimate. All that apply are reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NoEstimateReason {
    /// Reference input below its floor.
    NoReference,
    /// Measurement input below its floor.
    NoSignal,
    /// Less audio than the band's minimum observation.
    ObservationTooShort,
    /// No lag has reference coverage for at least half the measurement segments.
    InsufficientOverlap,
    /// Too little of the band is excited.
    InsufficientExcitation,
    /// The excitation repeats within the search span plus the tail allowance.
    PeriodicExcitation {
        /// Excitation period.
        period: Samples,
    },
    /// Peak-to-sidelobe ratio too low.
    LowPsr,
    /// The pick's timing uncertainty exceeds the band's tolerance.
    LowPrecision,
    /// The strongest peak lies at the edge of the search range.
    PeakAtSearchEdge,
    /// Coherent-to-incoherent band power too low.
    LowBandSnr,
}

/// Why a finding is ambiguous: the operator resolves it (decision 1c).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmbiguityReason {
    /// A candidate before the clear pick is within the margin of the threshold.
    BorderlineLevel,
    /// Another strong candidate lies within two pulse widths of the pick.
    CloseArrivals,
    /// The pick's lobe does not fit one arrival.
    MergedLobe,
    /// The pick has only acquisition evidence.
    OutsideRefinement,
}

/// Finder outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DelayOutcome {
    /// A clear first arrival.
    Accepted {
        /// First arrival within the threshold of the strongest (decision 1a).
        first: DelayArrival,
        /// Strongest arrival.
        strongest: DelayArrival,
    },
    /// Near-equal or unclear peaks: tracking pauses until the operator picks (decision 1c).
    Ambiguous {
        /// Every reason that applies.
        reasons: Vec<AmbiguityReason>,
        /// At most 3 arrivals, the rule pick first.
        ranked: Vec<DelayArrival>,
        /// Strongest arrival.
        strongest: DelayArrival,
    },
    /// Nothing trustworthy.
    NoEstimate {
        /// Every reason that applies.
        reasons: Vec<NoEstimateReason>,
    },
}

/// Evidence behind a finding; a field the finder did not reach before refusing is nil.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelayConfidence {
    /// Strongest envelope peak over its region's detection floor.
    pub psr_db: Option<Db>,
    /// The same on the acquisition envelope.
    pub psr_acq_db: Option<Db>,
    /// Coherent-to-incoherent band power at the strongest alignment.
    pub band_snr_db: Option<Db>,
    /// Octave fraction of the band that is excited (0…1).
    pub excited_fraction: Option<f64>,
    /// 1-σ timing uncertainty of the rule pick, samples.
    pub uncertainty_samples: Option<f64>,
    /// −6 dB pulse width under this excitation, samples.
    pub pulse_width_samples: Option<f64>,
    /// Excitation period (given or detected).
    pub period: Option<Samples>,
}

/// Most candidates a finding lists.
pub const MAX_FINDING_CANDIDATES: usize = 16;

/// Result of `delay.find`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelayFinding {
    /// Outcome.
    pub outcome: DelayOutcome,
    /// Evidence.
    pub confidence: DelayConfidence,
    /// Band analysed.
    pub band: DelayBand,
    /// Length of the measurement block analysed.
    pub observation: Seconds,
    /// Every candidate by delay (IR panel), also with `no_estimate`; at most
    /// [`MAX_FINDING_CANDIDATES`].
    pub candidates: Vec<DelayArrival>,
    /// When it was found.
    pub found_at: WallNs,
}

impl DelayFinding {
    /// The arrival `pick` selects, if the outcome has it.
    pub fn arrival(&self, pick: DelayPick) -> Option<&DelayArrival> {
        match (&self.outcome, pick) {
            (DelayOutcome::Accepted { first, .. }, DelayPick::FirstArrival) => Some(first),
            (DelayOutcome::Ambiguous { ranked, .. }, DelayPick::FirstArrival) => ranked.first(),
            (
                DelayOutcome::Accepted { strongest, .. }
                | DelayOutcome::Ambiguous { strongest, .. },
                DelayPick::Strongest,
            ) => Some(strongest),
            (DelayOutcome::Ambiguous { ranked, .. }, DelayPick::Ranked { index }) => {
                ranked.get(usize::from(index))
            }
            _ => None,
        }
    }

    /// The reasons of a `no_estimate` outcome.
    pub fn no_estimate(&self) -> Option<&[NoEstimateReason]> {
        match &self.outcome {
            DelayOutcome::NoEstimate { reasons } => Some(reasons),
            _ => None,
        }
    }
}

/// Delay state of a transfer measurement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelayState {
    /// Delay applied to the reference before the estimator.
    pub applied: Seconds,
    /// Applied delay in samples at the session rate (exact value used by DSP, fractions
    /// included: the whole samples shift the reference, the fraction rotates the phase).
    pub applied_samples: f64,
    /// How far `delay.nudge` steps have moved the applied delay from the arrival it was
    /// last set to (an insert, a typed value; tracking moves both and keeps this). The
    /// shared time base refers the live curve to `applied − nudged`, the arrival: a step
    /// then shows as a move of that curve alone, like a trace's display nudge, instead of
    /// being undone by the time base (decision 8a).
    pub nudged: Seconds,
    /// `nudged` in samples at the session rate, fractions included.
    pub nudged_samples: f64,
    /// Tracking enabled.
    pub tracking: bool,
    /// The last finding is ambiguous and the operator has not picked yet (decision 1c):
    /// tracking is paused until `delay.insert`, `delay.set` or a new `delay.find`.
    pub awaiting_pick: bool,
    /// Last finder result.
    pub last_finding: Option<DelayFinding>,
}
