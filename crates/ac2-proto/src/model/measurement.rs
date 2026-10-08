//! Measurement configuration: kinds, transfer/spectrum/RTA/SPL/Leq settings, trace ownership, math, and the measurement entity.

use serde::{Deserialize, Serialize};

use super::{
    AverageMethod, BandFraction, DelayState, PeakWeighting, Smoothing, SmoothingFraction,
    SpecAveraging, SweepConfig, TfAveraging, TimeWeighting, Weighting, Window,
};
use crate::grid::GridId;
use crate::topic::Stream;
use crate::units::{Db, DbSpl, Hz, MeasId, Rev, Seconds, TraceId};

/// Log-spaced column grid request (`k` indices on the base-2 grid around 1 kHz).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogGridSpec {
    /// Points per octave.
    pub ppo: u32,
    /// First index.
    pub k_min: i32,
    /// Last index (inclusive).
    pub k_max: i32,
}

/// Dual-channel transfer function measurement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferConfig {
    /// Reference input channel (zero-based device channel).
    pub reference_input: u16,
    /// Measurement input channel.
    pub measurement_input: u16,
    /// Averaging.
    pub averaging: TfAveraging,
    /// Output grid.
    pub grid: LogGridSpec,
    /// Live smoothing, if any.
    pub smoothing: Option<Smoothing>,
    /// How deep the decimated stages average (decision M1).
    pub depth: DepthPolicy,
}

/// Averaging depth of the MTW ladder's decimated stages (decision M1).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DepthPolicy {
    /// Every stage reaches the same effective-average count, so the coherence floor is the
    /// same at every frequency; low-frequency stages settle more slowly.
    EqualConfidence,
    /// No decimated stage averages over a longer span than `max_settle_s`; those stages
    /// hold fewer averages and show a higher coherence floor.
    FastLf {
        /// Longest averaging span of a decimated stage (> 0).
        max_settle_s: Seconds,
    },
}

/// Narrowband spectrum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpectrumConfig {
    /// Input channel.
    pub input: u16,
    /// FFT length, samples.
    pub fft_len: u32,
    /// Window.
    pub window: Window,
    /// Averaging.
    pub averaging: SpecAveraging,
    /// Display smoothing (power, on a log-frequency kernel over the bins), if any. A
    /// smoothed bin no longer reads as tone level; frames say so (`SpecMeta.smoothing`).
    pub smoothing: Option<SmoothingFraction>,
}

/// Fractional-octave RTA.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RtaConfig {
    /// Input channel.
    pub input: u16,
    /// Band fraction.
    pub fraction: BandFraction,
    /// Lowest band centre to include.
    pub f_lo: Hz,
    /// Highest band centre to include.
    pub f_hi: Hz,
    /// Frequency weighting.
    pub weighting: Weighting,
    /// Averaging.
    pub averaging: SpecAveraging,
}

/// Sound level meter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplConfig {
    /// Input channel.
    pub input: u16,
    /// Frequency weighting.
    pub weighting: Weighting,
    /// Time weighting.
    pub time_weighting: TimeWeighting,
    /// Peak weighting.
    pub peak_weighting: PeakWeighting,
    /// Rolling Leq windows (`docs/design/leq.md`).
    pub leq: LeqConfig,
    /// Measuring-position correction: added to every level the meter reports once
    /// calibrated (the log keeps what was measured, with the correction in force). Not
    /// applied to the band meter, whose transfer is measured from the mic's own position.
    pub position: Option<PositionCorrection>,
    /// 1/3-octave band Leq against dwelling limits (`docs/design/band-leq.md`); `None`:
    /// off.
    pub bands: Option<Box<super::BandLeqConfig>>,
}

/// The level difference from where the mic is to where a limit applies (the loudest
/// audience position, read from a mic at FOH), added to what the meter measures. The
/// energy levels (Leq, LAF…, LAFmax) and the peak levels (LCpeak, LZpeak) take separate
/// differences, as DIN 15905-5 has K1 and K2: a peak travels differently from the energy.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PositionCorrection {
    /// Added to the energy levels (dB).
    pub level: Db,
    /// Added to the peak levels (dB).
    pub peak: Db,
}

impl PositionCorrection {
    /// Largest correction either way, dB: a measuring position more than this far from
    /// where the limit applies is not that position.
    pub const MAX_DB: f64 = 30.0;

    /// Both differences the same.
    pub fn both(db: f64) -> Self {
        Self {
            level: Db(db),
            peak: Db(db),
        }
    }

    /// Whether both are finite and within ±[`Self::MAX_DB`].
    pub fn is_valid(&self) -> bool {
        [self.level.0, self.peak.0]
            .iter()
            .all(|v| v.is_finite() && v.abs() <= Self::MAX_DB)
    }
}

/// What a peak limit is set on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PeakQuantity {
    /// C-weighted peak (LCpeak).
    #[serde(rename = "lcpeak")]
    LcPeak,
    /// Highest A-weighted Fast level (LAFmax).
    #[serde(rename = "lafmax")]
    LafMax,
}

impl PeakQuantity {
    /// Both, in display order.
    pub const ALL: [PeakQuantity; 2] = [PeakQuantity::LcPeak, PeakQuantity::LafMax];
}

/// A limit on the highest LCpeak or LAFmax of any second: over as soon as one second
/// exceeds it (`docs/design/leq.md`, *Peak limits*).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeakLimit {
    /// Limit; judged only while the meter reads dB SPL.
    pub limit: DbSpl,
    /// "Near" within this much below the limit (≥ 0).
    pub warn_margin: Db,
}

/// The peak limits of an SPL meter.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeakLimits {
    /// On LCpeak.
    pub lcpeak: Option<PeakLimit>,
    /// On LAFmax.
    pub lafmax: Option<PeakLimit>,
}

impl PeakLimits {
    /// The limit on `q`.
    pub fn get(&self, q: PeakQuantity) -> Option<PeakLimit> {
        match q {
            PeakQuantity::LcPeak => self.lcpeak,
            PeakQuantity::LafMax => self.lafmax,
        }
    }

    /// The limit on `q`, to change.
    pub fn get_mut(&mut self, q: PeakQuantity) -> &mut Option<PeakLimit> {
        match q {
            PeakQuantity::LcPeak => &mut self.lcpeak,
            PeakQuantity::LafMax => &mut self.lafmax,
        }
    }
}

/// One rolling Leq window of an SPL meter.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeqWindow {
    /// Length: whole seconds, 1 s … [`LeqWindow::MAX_SECONDS`].
    pub duration: Seconds,
    /// Frequency weighting.
    pub weighting: Weighting,
    /// Limit; judged only while the meter reads dB SPL (calibrated).
    pub limit: Option<DbSpl>,
    /// The window is "near" within this much below the limit (≥ 0).
    pub warn_margin: Db,
}

/// The Leq windows of an SPL meter and the horizon of their headroom figure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeqConfig {
    /// Windows, in display order (at most [`LeqConfig::MAX_WINDOWS`]).
    pub windows: Vec<LeqWindow>,
    /// Headroom horizon: the steady level allowed over this much of the future
    /// (whole seconds, 1 s … 1 h).
    pub horizon: Seconds,
    /// Limits on the highest LCpeak and LAFmax.
    pub peaks: PeakLimits,
}

/// Judgement of a rolling Leq window against its limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeqJudgement {
    /// The window has no limit.
    NoLimit,
    /// A limit is set but the meter reads dBFS: nothing is judged.
    NotCalibrated,
    /// More than the warn margin below the limit (or nothing measured yet).
    Ok,
    /// Within the warn margin below the limit, or at it.
    Near,
    /// Above the limit.
    Over,
}

/// Informational presets of published limits, each on one or more windows and the peak
/// limits the rule sets. Not legal advice: each rule has more to it (measuring position,
/// duties);
/// `docs/design/leq.md` lists the sources and what is not covered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LeqPreset {
    /// DIN 15905-5: LAeq 30 min ≤ 99 dB, LCpeak ≤ 135 dB.
    Din15905,
    /// Swiss V-NISSG, first category: LAeq 60 min ≤ 93 dB, LAFmax ≤ 125 dB.
    Swiss93,
    /// Swiss V-NISSG, second category: LAeq 60 min ≤ 96 dB, LAFmax ≤ 125 dB.
    Swiss96,
    /// Swiss V-NISSG, third category: LAeq 60 min ≤ 100 dB, LAFmax ≤ 125 dB.
    Swiss100,
    /// WHO safe listening venues and events (2022): LAeq 15 min ≤ 100 dB.
    Who,
    /// France, Code de la santé publique art. R1336-1: LAeq 15 min ≤ 102 dB and
    /// LCeq 15 min ≤ 118 dB.
    France,
    /// France, art. R1336-1, events for children up to six: LAeq 15 min ≤ 94 dB and
    /// LCeq 15 min ≤ 104 dB.
    FranceChildren,
    /// Flanders, VLAREM II art. 6.7.3: LAeq 15 min ≤ 85 dB.
    Flanders85,
    /// Flanders, VLAREM II art. 5.32.2.2bis § 1: LAeq 15 min ≤ 95 dB.
    Flanders95,
    /// Flanders, VLAREM II art. 5.32.2.2bis § 2: LAeq 60 min ≤ 100 dB, LAeq 15 min shown.
    Flanders100,
    /// Brussels-Capital, son amplifié art. 3: LAeq 15 min ≤ 85 dB.
    Brussels85,
    /// Brussels-Capital, son amplifié art. 4: LAeq 15 min ≤ 95 dB and LCeq 15 min ≤ 110 dB.
    Brussels95,
    /// Brussels-Capital, son amplifié art. 5: LAeq 60 min ≤ 100 dB and LCeq 60 min ≤ 115 dB.
    Brussels100,
    /// Netherlands, fourth covenant (voluntary), art. 3.1.2: LAeq 15 min ≤ 103 dB.
    NetherlandsCovenant,
    /// Netherlands covenant art. 3.1.3 c, audiences of 16 and 17: LAeq 15 min ≤ 100 dB.
    NetherlandsCovenant16To17,
    /// Netherlands covenant art. 3.1.3 b, audiences of 14 and 15: LAeq 15 min ≤ 96 dB.
    NetherlandsCovenant14To15,
    /// Netherlands covenant art. 3.1.3 a, audiences up to 13: LAeq 15 min ≤ 91 dB.
    NetherlandsCovenantTo13,
    /// Finland, STM 545/2015 §12, against hearing damage: LAeq 4 h ≤ 100 dB, LAFmax ≤ 115
    /// dB, LCpeak ≤ 140 dB.
    Finland545,
}

/// What a measurement computes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MeasKind {
    /// Transfer function (publishes `tf`, `ir`, `levels`).
    Transfer {
        /// Configuration.
        config: TransferConfig,
    },
    /// Spectrum (publishes `spec`, `levels`).
    Spectrum {
        /// Configuration.
        config: SpectrumConfig,
    },
    /// RTA (publishes `rta`, `levels`).
    Rta {
        /// Configuration.
        config: RtaConfig,
    },
    /// SPL meter (publishes `spl`, `levels`).
    Spl {
        /// Configuration.
        config: SplConfig,
    },
    /// Math channel: a result the daemon computes from named live measurements and stored
    /// traces, published as `tf`, `spec` or `rta` by its domain
    /// (`docs/design/math-channels.md`).
    Math {
        /// Configuration.
        config: MathConfig,
    },
    /// Sweep measurement: settings only. Nothing plays until `sweep.run` fires it (under the
    /// stimulus lease, armed, like firing); each run is stored as a sweep trace it owns
    /// (`docs/design/measurement-tree.md`).
    Sweep {
        /// Configuration.
        config: SweepConfig,
    },
}

impl MeasKind {
    /// The stream that carries the measurement's curve or reading; `None` for a sweep
    /// measurement, whose results are stored traces, never a stream.
    pub fn stream(&self) -> Option<Stream> {
        match self {
            MeasKind::Transfer { .. } => Some(Stream::Tf),
            MeasKind::Spectrum { .. } => Some(Stream::Spec),
            MeasKind::Rta { .. } => Some(Stream::Rta),
            MeasKind::Spl { .. } => Some(Stream::Spl),
            MeasKind::Math { config } => Some(config.domain.stream()),
            MeasKind::Sweep { .. } => None,
        }
    }

    /// Whether the measurement publishes a `tf` stream (a transfer function or transfer
    /// math), so it is drawn, captured and compared as a transfer function.
    pub fn publishes_tf(&self) -> bool {
        self.stream() == Some(Stream::Tf)
    }

    /// Whether the measurement is drawn on the spectrum pane: a spectrum, an RTA, or math
    /// on either.
    pub fn publishes_levels(&self) -> bool {
        matches!(self.stream(), Some(Stream::Spec | Stream::Rta))
    }

    /// Whether it runs as a job (`meas.start`): every kind but a sweep measurement, which
    /// plays only when `sweep.run` fires it.
    pub fn is_job(&self) -> bool {
        !matches!(self, MeasKind::Sweep { .. })
    }
}

/// Who a stored trace or a math channel belongs to: the measurement it is listed under, or
/// the imported group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TraceOwner {
    /// A measurement (never a math channel: math lives under a measurement).
    Meas {
        /// The measurement.
        meas: MeasId,
    },
    /// No measurement: imports, and the traces of a measurement deleted with them kept.
    Imported,
}

impl TraceOwner {
    /// The owning measurement, if any.
    pub fn meas(self) -> Option<MeasId> {
        match self {
            TraceOwner::Meas { meas } => Some(meas),
            TraceOwner::Imported => None,
        }
    }
}

/// What `meas.delete` does with the stored traces and math channels the measurement owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnedTraces {
    /// They stay, moved to [`TraceOwner::Imported`].
    Keep,
    /// They are deleted with it.
    Delete,
}

/// A math channel: an expression over operands of one domain, evaluated by the daemon
/// whenever it publishes, so every client shows the same result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MathConfig {
    /// The measurement it is listed under (the one selected when it was made); its
    /// captures are filed there too.
    pub owner: TraceOwner,
    /// What the operands are, and so what the result is and where it is drawn.
    pub domain: MathDomain,
    /// The expression.
    pub expr: MathExpr,
    /// Delay the phase of a transfer sum, difference or average is referred to.
    pub reference: MathReference,
    /// Display smoothing of the result (transfer and spectrum domains); the operands are
    /// combined unsmoothed.
    pub smoothing: Option<Smoothing>,
}

impl MathConfig {
    /// Fewest operands of an average, and fewest usable ones for it to show a value.
    pub const MIN_AVERAGE: usize = 2;
    /// Most operands of an average.
    pub const MAX_AVERAGE: usize = 16;

    /// `expr` in `domain` under `owner`, phase referred to the first operand, unsmoothed.
    pub fn of(owner: TraceOwner, domain: MathDomain, expr: MathExpr) -> Self {
        let first = expr
            .operands()
            .first()
            .copied()
            .unwrap_or(Operand::Meas { meas: MeasId(0) });
        Self {
            owner,
            domain,
            expr,
            reference: MathReference::Operand { operand: first },
            smoothing: None,
        }
    }

    /// The power average of `of` under `owner`: a spatial average of mic positions.
    pub fn power_average(owner: TraceOwner, domain: MathDomain, of: Vec<Operand>) -> Self {
        Self::of(
            owner,
            domain,
            MathExpr::Average {
                of,
                method: AverageMethod::Power,
            },
        )
    }
}

/// What a math channel's operands are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MathDomain {
    /// Transfer functions (and sweep traces, their fundamental): magnitude and phase
    /// combine as complex values. Publishes `tf`.
    Transfer,
    /// Narrowband spectra on one bin grid: tone levels. Publishes `spec`.
    Spectrum,
    /// RTA bands on one band layout: band power. Publishes `rta`.
    Rta,
}

impl MathDomain {
    /// The stream a math channel of this domain publishes.
    pub fn stream(self) -> Stream {
        match self {
            MathDomain::Transfer => Stream::Tf,
            MathDomain::Spectrum => Stream::Spec,
            MathDomain::Rta => Stream::Rta,
        }
    }
}

/// A math channel's expression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MathExpr {
    /// `a op b`.
    Binary {
        /// Left operand.
        a: Operand,
        /// Operator.
        op: MathOp,
        /// Right operand.
        b: Operand,
    },
    /// Average of [`MathConfig::MIN_AVERAGE`] … [`MathConfig::MAX_AVERAGE`] distinct
    /// operands.
    Average {
        /// Operands, in display order.
        of: Vec<Operand>,
        /// How they are combined (spectrum and RTA: power only).
        method: AverageMethod,
    },
}

impl MathExpr {
    /// Every operand, in expression order.
    pub fn operands(&self) -> Vec<Operand> {
        match self {
            MathExpr::Binary { a, b, .. } => vec![*a, *b],
            MathExpr::Average { of, .. } => of.clone(),
        }
    }

    /// Whether `o` is one of the operands.
    pub fn names(&self, o: Operand) -> bool {
        self.operands().contains(&o)
    }
}

/// A math operator. Transfer functions combine as complex values (magnitude and phase
/// together); spectra and RTA bands as levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MathOp {
    /// `A ÷ B`: A relative to B (transfer only).
    Divide,
    /// `A × B`: A cascaded with B (transfer only).
    Multiply,
    /// `A + B`: transfer: the complex sum (what A and B sum to acoustically); levels: the
    /// power sum.
    Add,
    /// `A − B`: transfer: the complex difference; levels: the level difference in dB.
    Subtract,
}

/// A math channel's operand: a live measurement or a stored trace, named by id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operand {
    /// A live measurement's current result.
    Meas {
        /// The measurement.
        meas: MeasId,
    },
    /// A stored trace.
    Trace {
        /// The trace.
        trace: TraceId,
    },
}

/// Delay the phase of a transfer sum, difference or average is referred to.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MathReference {
    /// The delay one operand was measured with (a live operand's newest).
    Operand {
        /// That operand.
        operand: Operand,
    },
    /// An explicit delay.
    Fixed {
        /// Delay.
        delay: Seconds,
    },
}

/// What a math result's phase is relative to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseBasis {
    /// Every operand shares one time base (one session epoch): the phase keeps their
    /// relative arrival, referred to the delay the result states.
    SharedTimeBase,
    /// The operands share no time base (an import, another epoch): the phase combines each
    /// operand as aligned by its own delay; their relative arrival is unknown.
    OwnAlignments,
    /// The result has no phase (levels, an operand without phase, a power average across
    /// time bases).
    NoPhase,
}

/// Arguments of `meas.create` / `meas.update`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasConfig {
    /// Display name.
    pub name: String,
    /// Analysis.
    pub kind: MeasKind,
}

/// Measurement entity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Measurement {
    /// Id.
    pub id: MeasId,
    /// Configuration.
    pub config: MeasConfig,
    /// Rev at which the config was committed; frames with `config_rev` ≥ this show it.
    pub config_rev: Rev,
    /// Job running.
    pub running: bool,
    /// Display frozen (averaging continues off-screen only if running).
    pub frozen: bool,
    /// Delay (transfer measurements only).
    pub delay: Option<DelayState>,
    /// Grid of the measurement's main stream.
    pub grid_id: Option<GridId>,
}
