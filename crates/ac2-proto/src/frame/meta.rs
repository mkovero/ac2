//! Per-kind metadata and the msgpack header.

use serde::{Deserialize, Serialize};

use crate::grid::GridId;
use crate::model::{
    BackendKind, Ballistic, BandFraction, BinRange, CalStatus, DeviceId, FreqRange, LeqJudgement,
    LevelScale, Operand, Pair, PairSide, PeakWeighting, PhaseBasis, PositionCorrection,
    PreviewInput, Smoothing, SmoothingFraction, SpecLevel, TimeWeighting, TimingState,
    TimingStatus, Unresolved, Weighting, Window,
};
use crate::units::{
    ClientId, DaemonIncarnation, Db, Dbfs, Dbtp, Hz, Lufs, Rev, SampleIndex, Samples, Seconds,
    StreamEpoch, WallNs,
};

use super::{ArrayDesc, FrameKind, ProtectionFlags};

/// TF metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TfMeta {
    /// Delay applied to the reference; for a math trace, the delay its phase is referred
    /// to.
    pub delay: Seconds,
    /// The part of `delay` that `delay.nudge` steps added to the arrival
    /// ([`crate::model::Delay::nudged`]) when this frame was made; 0 for a math trace.
    /// A view's shared time base refers the curve to `delay − nudged`.
    pub nudged: Seconds,
    /// Live smoothing applied.
    pub smoothing: Option<Smoothing>,
    /// A mic curve was subtracted from `mag` (for a math trace: from every operand).
    pub mic_curve: bool,
    /// The lowest and highest point of that mic curve. Outside them the curve is held at
    /// its end value, so those columns are not corrected for the mic. `None` without a
    /// curve and for a math trace.
    pub mic_range: Option<FreqRange>,
    /// What a math trace combined; `None` for a transfer measurement.
    pub math: Option<Box<MathState>>,
    /// Where the columns are finer than the bins of the analysis stage serving them, with
    /// that width; ascending and disjoint, empty: every column is resolved. A math trace
    /// whose included operands are all live transfers states their ranges, each piece at
    /// the coarsest width among the operands covering it (the limit on what the result
    /// resolves); `None` when it includes a stored trace, which keeps no ranges, or no
    /// operand at all.
    pub unresolved: Option<Vec<BinRange>>,
    /// The analysis stages whose own bins go on `bins/<meas>/<stage>`, for a transfer at
    /// the `bins` resolution; `None` for any other resolution.
    pub bins: Option<Box<super::BinLayout>>,
    /// The frequencies whose averages are still filling: the span served by the analysis
    /// stages that have not yet accumulated their full depth (the longer windows of the
    /// low stages fill last). `None` once every stage is full; for a math trace the span
    /// over its included live operands.
    pub settling: Option<FreqRange>,
    /// How full the averages behind this frame are, 0 to 1: the least full analysis stage's
    /// accumulated blocks over the blocks it needs. Blocks, not elapsed time, so blocks that
    /// protection refused add nothing and a measurement that never averaged never reads 1.
    /// Columns turn valid with a single block, long before their averages settle, so a
    /// client that waits for a settled average (a capture at a position) needs this.
    /// For a math trace: the least of its included live operands (1 for a stored trace).
    pub filled: f32,
}

/// What a math trace's frame combined.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MathState {
    /// Every operand of the expression, in expression order, with whether it went in.
    pub operands: Vec<OperandState>,
    /// What the result's phase is relative to.
    pub phase: PhaseBasis,
    /// An alignment's fit and its view's curves; `None` for every other expression.
    pub align: Option<Box<crate::model::AlignState>>,
    /// An average's range across positions (its band in the transfer frame's range
    /// arrays); `None` for every other expression and for level averages.
    pub range: Option<crate::model::RangeSpread>,
}

impl MathState {
    /// Operands that went into the frame.
    pub fn included(&self) -> usize {
        self.operands
            .iter()
            .filter(|m| m.status == OperandStatus::Included)
            .count()
    }
}

/// One operand of a math trace in a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperandState {
    /// The operand.
    pub operand: Operand,
    /// Whether it went in, and why not.
    pub status: OperandStatus,
}

/// Whether an operand went into a math trace's frame. An operand is left out rather than
/// let it mislead the result (PLAN principle 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum OperandStatus {
    /// Went in.
    Included,
    /// Not running (or deleted).
    Stopped,
    /// Running without a usable result yet: no valid column, or not answering.
    Settling,
    /// Its own frame shows a fault banner (clip, no reference, check routing, no signal).
    Refused {
        /// The operand's protection flags.
        protection: ProtectionFlags,
    },
    /// Its result cannot be combined with the others: another level scale, bin grid or band
    /// layout.
    Mismatch,
}

impl OperandStatus {
    /// Protection flags that leave an operand out: the ones that raise a fault banner on
    /// its own frame. A weak reference or a recent discontinuity only hold or restart the
    /// operand's averaging, which its validity mask already reports per column.
    pub const REFUSING: ProtectionFlags = ProtectionFlags(
        ProtectionFlags::CLIP.0
            | ProtectionFlags::NO_REFERENCE.0
            | ProtectionFlags::CHECK_ROUTING.0
            | ProtectionFlags::NO_SIGNAL.0,
    );
}

/// IR metadata: point `i` is at `t0 + i · dt` relative to the inserted delay.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IrMeta {
    /// Sample rate of the underlying IR.
    pub sample_rate_hz: Hz,
    /// Time of point 0.
    pub t0: Seconds,
    /// Point spacing after decimation.
    pub dt: Seconds,
    /// Delay inserted before the IR was computed (absolute time = t + this).
    pub inserted_delay: Seconds,
}

/// RTA metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RtaMeta {
    /// Band fraction.
    pub fraction: BandFraction,
    /// Frequency weighting.
    pub weighting: Weighting,
    /// Unit of `level` (band power).
    pub scale: LevelScale,
    /// Calibration applied (`uncalibrated` with dBFS).
    pub cal: CalStatus,
    /// A mic curve was subtracted from `level`.
    pub mic_curve: bool,
    /// The lowest and highest point of that mic curve. Outside them the curve is held at
    /// its end value, so those columns are not corrected for the mic. `None` without a
    /// curve and for a math trace.
    pub mic_range: Option<FreqRange>,
    /// What a math trace combined; `None` for an RTA measurement.
    pub math: Option<Box<MathState>>,
}

/// Spectrum metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecMeta {
    /// Window.
    pub window: Window,
    /// Unit of `level` (decision 4b).
    pub scale: LevelScale,
    /// Tone level or density: `level` in dBFS or dBFS/Hz (dB SPL or dB SPL/Hz).
    pub level: SpecLevel,
    /// Calibration applied (`uncalibrated` with dBFS).
    pub cal: CalStatus,
    /// A mic curve was subtracted from `level`.
    pub mic_curve: bool,
    /// The lowest and highest point of that mic curve. Outside them the curve is held at
    /// its end value, so those columns are not corrected for the mic. `None` without a
    /// curve and for a math trace.
    pub mic_range: Option<FreqRange>,
    /// Display smoothing applied to `level`: a smoothed bin is a fractional-octave power
    /// average of tone levels, not the tone level of the bin.
    pub smoothing: Option<SmoothingFraction>,
    /// What a math trace combined; `None` for a spectrum measurement.
    pub math: Option<Box<MathState>>,
    /// The bands behind a multi-resolution spectrum's columns; `None` for one FFT and a
    /// math trace.
    pub bands: Option<Box<SpecBands>>,
    /// The generator's sine read from this measurement's spectrum; `None` unless the
    /// generator is firing a sine, and for a math trace.
    pub harmonics: Option<Box<SpecHarmonics>>,
}

/// The harmonics of the generator's sine in a spectrum, read from the averaged power
/// spectrum at full resolution (before display smoothing), each tone as the power in its
/// window's main lobe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecHarmonics {
    /// The generator's frequency.
    pub f0: Hz,
    /// `f0` is a bin centre of the FFT that was read: the window leaks nothing and the
    /// reading is exact with any window.
    pub on_bin: bool,
    /// What the spectrum says about the sine.
    pub reading: HarmonicReading,
}

/// The lowest ratio a harmonic reading carries, dB. A ratio is a quotient of f64 powers,
/// whose rounding alone lies near −320 dB re the larger, so only a power of exactly zero
/// (digital silence around the sine) is lower: it is sent as this, never as −∞.
pub const HARMONIC_DB_MIN: f64 = -400.0;

/// A spectrum's reading of the sine, or why it has none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HarmonicReading {
    /// The sine was read.
    Read {
        /// Level of the fundamental in the frame's `scale` (dBFS or dB SPL), as the
        /// spectrum shows a tone at `f0` in tone-level mode: calibration and mic curve
        /// applied.
        fundamental: f64,
        /// Harmonics 2, 3, … in order, as far as their lobes lie below fs/2 (at most the
        /// 10th).
        harmonics: Vec<HarmonicLevel>,
        /// Total harmonic distortion: the power of the resolved harmonics re the
        /// fundamental. `None` when no harmonic is resolved; then only `thd_bound` says
        /// anything.
        thd: Option<Db>,
        /// Upper limit of the distortion: every harmonic listed, each at the larger of its
        /// level and its floor, re the fundamental. Like `thd_n`, not below
        /// [`HARMONIC_DB_MIN`].
        thd_bound: Db,
        /// THD+N: everything outside the lobes of DC and the fundamental, re the
        /// fundamental. With a mic curve each bin is corrected as the spectrum's is.
        thd_n: Db,
        /// The lower limit of `thd_n` set by the window: the part of an off-bin sine's own
        /// power outside its lobe. `None` on a bin, where the window leaks nothing.
        leakage: Option<Db>,
    },
    /// The spectrum cannot be read for this sine.
    Unreadable {
        /// Why.
        reason: HarmonicUnreadable,
    },
}

/// Why a spectrum has no reading for the sine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarmonicUnreadable {
    /// A rectangular window reads a sine only on a bin: between bins its leakage covers
    /// every harmonic.
    RectangularOffBin,
    /// The frequency is too low for this FFT length: the lobes of DC, the sine and its
    /// harmonics leave no bins between them.
    TooLow,
    /// The sine's own lobe reaches fs/2.
    OutOfBand,
    /// The sine's lobe holds no power (digital silence): nothing to refer harmonics to.
    NoFundamental,
}

/// One harmonic of the sine, re the fundamental; neither ratio is below
/// [`HARMONIC_DB_MIN`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarmonicLevel {
    /// Order k: the harmonic at k·`f0`.
    pub order: u8,
    /// Its level re the fundamental (dBc). When not `resolved` this is the floor's: the
    /// harmonic is below it, by how much is not known.
    pub level: Db,
    /// What its lobe would hold with no harmonic there (the noise or the fundamental's
    /// skirt around it), re the fundamental.
    pub floor: Db,
    /// `level` stands at least 6 dB above `floor`.
    pub resolved: bool,
}

/// The windows of a multi-resolution spectrum as of this frame
/// (`docs/design/multires-spectrum.md`). Band `b` alone serves the columns at its
/// `sole` frequency; between two bands' `sole` frequencies a column blends both, so the
/// columns below the first follow the longest window and those above the last the
/// shortest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecBands {
    /// Bands, longest window first.
    pub bands: Vec<SpecBand>,
    /// Where the columns are finer than the band serving them resolves.
    pub unresolved: Unresolved,
}

/// One window of a multi-resolution spectrum.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecBand {
    /// Window length.
    pub window: Seconds,
    /// Time between the band's spectra: how often the columns it serves change.
    pub hop: Seconds,
    /// Where this band is the only estimate.
    pub sole: Hz,
    /// From the end of the band's newest spectrum to the end of this frame's
    /// (`audio_sample`); `None` before its first, when its columns have no value.
    pub age: Option<Seconds>,
}

/// SPL meter readings; values in `scale`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplMeta {
    /// Unit.
    pub scale: LevelScale,
    /// Frequency weighting.
    pub weighting: Weighting,
    /// Time weighting.
    pub time_weighting: TimeWeighting,
    /// Peak weighting.
    pub peak_weighting: PeakWeighting,
    /// Current time-weighted level.
    pub level: f64,
    /// Max in interval.
    pub lmax: f64,
    /// Min in interval.
    pub lmin: f64,
    /// Leq over interval.
    pub leq: f64,
    /// Peak over interval.
    pub lpeak: f64,
    /// Interval length.
    pub duration: Seconds,
    /// Calibration applied (`uncalibrated` with dBFS).
    pub cal: CalStatus,
    /// The mic-curve correction filter ran before frequency weighting (never on `lpeak`).
    pub mic_curve: bool,
    /// The measuring-position correction included in every level (`lpeak` takes its
    /// `peak` difference, the others its `level`); only while calibrated.
    pub position: Option<PositionCorrection>,
}

/// Rolling Leq metadata; the arrays hold one column per window of the meter's
/// configuration (`config_rev` in the header says which), in its order.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeqMeta {
    /// Unit of `leq` and `allowed`.
    pub scale: LevelScale,
    /// Calibration applied (`uncalibrated` with dBFS).
    pub cal: CalStatus,
    /// The mic-curve correction filter ran before frequency weighting.
    pub mic_curve: bool,
    /// Headroom horizon.
    pub horizon: Seconds,
    /// Rows the meter's log has logged so far (`spl.log_get`).
    pub logged: u64,
    /// The whole log: its run clock and total; `None` before its first second.
    pub run: Option<LeqRun>,
    /// The LCpeak limit's state, when the meter has one.
    pub lcpeak: Option<LeqPeak>,
    /// The LAFmax limit's state, when the meter has one.
    pub lafmax: Option<LeqPeak>,
    /// The measuring-position correction included in every level of the frame (windows,
    /// headroom, run, peaks); only while calibrated.
    pub position: Option<PositionCorrection>,
}

/// An SPL meter's band meter once a second (`docs/design/band-leq.md`): the shown bands of
/// every band window judged, window by window in the configuration's order (`config_rev` in
/// the header says which), the predicted level at the transfer's place. The arrays hold one
/// column per window and shown band, window-major: column `w · bands.len() + i` is band
/// `bands[i]` of window `w`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BandLeqMeta {
    /// Unit of every level (`db_spl` once calibrated; limits are judged only then).
    pub scale: LevelScale,
    /// Calibration applied.
    pub cal: CalStatus,
    /// The mic-curve correction filter ran before the band filters.
    pub mic_curve: bool,
    /// Headroom horizon (the meter's Leq horizon).
    pub horizon: Seconds,
    /// The §13 correction in force, dB (included in the newest seconds' levels).
    pub correction: Db,
    /// Where the limits at the mic come from.
    pub limits_from: crate::model::BandLimitPlace,
    /// The windows, in the configuration's order.
    pub windows: Vec<BandWindowState>,
    /// The predicted level at the transfer's place; `None` without a transfer or a
    /// predicted window.
    pub predicted: Option<crate::model::PredictedLeq>,
}

/// One band window of a `band_leq` frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BandWindowState {
    /// The window's band, its nominal centre.
    pub band: crate::units::Hz,
    /// Window length.
    pub duration: Seconds,
    /// Weighting of its band levels.
    pub weighting: Weighting,
    /// Seconds of the window elapsed (less than `duration` while it fills).
    pub elapsed: Seconds,
    /// Seconds of those measured.
    pub measured: Seconds,
    /// The limit the window is judged by: night while it holds a night second.
    pub period: crate::model::BandPeriod,
    /// The limit its headroom figures are computed against (once the horizon has passed).
    pub period_after_horizon: crate::model::BandPeriod,
}

/// A peak limit's state: the highest second within the hold, judged against the limit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeqPeak {
    /// The highest LCpeak (LAFmax) of any second within the newest
    /// [`LeqPeak::HOLD_S`] seconds, in the frame's `scale`; NaN before anything was
    /// measured.
    pub level: f64,
    /// Its judgement (`not_calibrated` with dBFS).
    pub judgement: LeqJudgement,
}

impl LeqPeak {
    /// Seconds a peak level is held for judging and display.
    pub const HOLD_S: u32 = 10;
}

/// An SPL meter's log as a whole: from its oldest kept second to its newest, the time
/// measured in between and the energy average over that measured time.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeqRun {
    /// Wall time of the oldest second kept (the log's start, or the oldest second the
    /// retention kept when `trimmed`).
    pub started_at: WallNs,
    /// Wall time of the end of the newest second.
    pub until: WallNs,
    /// Time measured from `started_at` to `until`.
    pub measured: Seconds,
    /// Time between them not measured (capture gaps, the meter or the daemon stopped).
    pub gaps: Seconds,
    /// The log reached its retention ([`crate::model::SplLogPage::RETAINED_ROWS`]): older
    /// seconds were dropped or may have been.
    pub trimmed: bool,
    /// LAeq over the measured time, in the frame's `scale`; NaN when nothing was measured.
    pub laeq: f64,
    /// LCeq over the measured time.
    pub lceq: f64,
    /// LZeq over the measured time.
    pub lzeq: f64,
}

/// Input meters metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LevelsMeta {
    /// Device input channel of each column.
    pub channels: Vec<u16>,
}

/// One stereo pair's correlation this frame (docs/design/programme-meter.md §4); the pair
/// is the one at the same index of its frame's `meta.pairs`. On the wire the arrays
/// `pair_corr` and `pair_quiet`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairLevels {
    /// Normalised zero-lag correlation over the rig's window, −1 … 1 (mono +1, wide 0,
    /// reversed −1); `None` while a side is below the gate.
    pub correlation: Option<f32>,
    /// The side below the rig's correlation gate (left checked first), when one is: a dead
    /// channel must not read 0 and look like wide stereo.
    pub quiet: Option<PairSide>,
}

/// A pair of inputs the meters read together: zero-based device input channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeterPair {
    /// Left side.
    pub left: u16,
    /// Right side.
    pub right: u16,
}

/// What a loudness figure is measured over (docs/design/programme-meter.md §6.1): a
/// declared pair with both inputs captured (stereo, channel weights 1, 1), or one captured
/// input not in a pair (weight 1, so a mono signal reads 3 LU below the same on a pair).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LoudnessSource {
    /// A declared pair; zero-based device input channels, as in [`MeterPair`].
    Pair {
        /// Zero-based device input channel of the left side.
        left: u16,
        /// Zero-based device input channel of the right side.
        right: u16,
    },
    /// One unpaired input.
    Input {
        /// Zero-based device input channel.
        channel: u16,
    },
}

/// A loudness source and when it was last reset: what stands between configuration
/// changes and resets, so it is metadata, not an array.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoudnessSourceMeta {
    /// What it is measured over.
    pub source: LoudnessSource,
    /// The daemon's clock at the reset, for the "since" line only.
    pub since: WallNs,
}

/// One source's BS.1770 / EBU Tech 3341 and 3342 loudness this frame; the source is the one
/// at the same index of its frame's `meta.sources`. On the wire the arrays `lufs_m`,
/// `lufs_s`, `lufs_i`, `lra`, `lufs_m_max`, `lufs_s_max`, `tp_max` and `loud_s`, in f32.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceLoudness {
    /// Momentary loudness (the last 400 ms); −∞ on silence or before a window lies wholly
    /// after the source's start or a discontinuity.
    pub momentary: Lufs,
    /// Short-term loudness (the last 3 s); −∞ as `momentary`.
    pub short_term: Lufs,
    /// Integrated (gated) loudness since the reset; `None` until a block passes the
    /// absolute gate.
    pub integrated: Option<Lufs>,
    /// Loudness range (Tech 3342), LU; `None` until a short-term value passes the gate.
    pub range: Option<Db>,
    /// Highest momentary over windows wholly after the reset; −∞ until the first.
    pub max_momentary: Lufs,
    /// Highest short-term over windows wholly after the reset; −∞ until the first.
    pub max_short_term: Lufs,
    /// Highest true peak of the source's channels since the reset; −∞ until the first
    /// metered block.
    pub max_true_peak: Dbtp,
    /// Metered samples since the reset, as time: the range is settling below 60 s, and it
    /// must not depend on a wall clock that NTP may step.
    pub duration: Seconds,
}

/// Stream input meters metadata: what stands until the configuration changes. Every
/// reading is an array (`StreamLevelsFrame`), so a delta carries no metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamLevelsMeta {
    /// Device input channel of each column.
    pub channels: Vec<u16>,
    /// What the bars read (the rig's `levels.ballistic`), so a frame is self-describing:
    /// under a peak detector `ppm` and `max` are its reading (dBFS); under `momentary` /
    /// `short_term` they are each input's true peak (dBTP) and the bars read the sources'
    /// loudness.
    pub ballistic: Ballistic,
    /// Every declared pair whose both inputs are captured, in `stream.inputs` order: one
    /// element of each pair array per pair.
    pub pairs: Vec<MeterPair>,
    /// Every loudness source: pairs first in `pairs` order, then unpaired line and digital
    /// inputs ascending by channel; none for mic inputs. One element of each source array
    /// per source.
    pub sources: Vec<LoudnessSourceMeta>,
}

/// A rig's own input meters for another rig: [`StreamLevelsMeta`] over its captured inputs
/// only, with its rate and the timing mark the clocks between the rigs are measured from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceLevelsMeta {
    /// Device input channel of each column.
    pub channels: Vec<u16>,
    /// As [`StreamLevelsMeta::ballistic`].
    pub ballistic: Ballistic,
    /// As [`StreamLevelsMeta::pairs`].
    pub pairs: Vec<MeterPair>,
    /// As [`StreamLevelsMeta::sources`].
    pub sources: Vec<LoudnessSourceMeta>,
    /// The stream's nominal sample rate, Hz.
    pub sample_rate_hz: u32,
    /// The newest block's timing; `None` where the rig stamps no capture times or its stamp
    /// clock is not yet mapped onto its system clock. The one field a delta carries.
    pub mark: Option<LevelsMark>,
}

/// A block's sample index with its capture time put on system time by the rig that captured
/// it: two rigs disciplined from one source share system time, so marks of both taken at the
/// same system time relate their sample clocks with no audio exchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LevelsMark {
    /// Stream sample index of the block's first frame.
    pub sample: SampleIndex,
    /// Its capture time on the rig's system clock, ns since the Unix epoch.
    pub system_ns: u64,
    /// Continuity breaks (xruns, estimated gaps) of the capture so far: a count that moved
    /// since the last mark seen says the sample index broke between them, which a counter
    /// tells even when frames between were replaced by newer ones on the way.
    pub breaks: u64,
}

/// Device preview meters metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewLevelsMeta {
    /// Backend previewed.
    pub backend: BackendKind,
    /// Device previewed.
    pub device: DeviceId,
    /// Device input channel of each column.
    pub channels: Vec<u16>,
    /// What is on the inputs, as `stream.preview` declared them.
    pub inputs: Vec<PreviewInput>,
    /// Pairs among them, as declared: one element of each pair array per pair.
    pub pairs: Vec<Pair>,
    /// As [`StreamLevelsMeta::ballistic`].
    pub ballistic: Ballistic,
    /// As [`StreamLevelsMeta::sources`].
    pub sources: Vec<LoudnessSourceMeta>,
}

/// One pair of a goniometer frame: which inputs, and the gain its points are relative to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GonioPairMeta {
    /// Zero-based device input channel of the left side.
    pub left: u16,
    /// Zero-based device input channel of the right side.
    pub right: u16,
    /// The auto gain, dBFS of the pair magnitude.
    pub gain_dbfs: f32,
}

/// Goniometer frame metadata: `n` is the pair count times
/// [`super::GONIO_POINTS`], the `gonio_x` and `gonio_y` arrays holding every pair's points in
/// turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GonioMeta {
    /// Pairs, in the declared order.
    pub pairs: Vec<GonioPairMeta>,
}

/// One timing correlation window.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimingWindow {
    /// Window start in the capture clock.
    pub capture_start: SampleIndex,
    /// Offset found, if confident.
    pub offset: Option<Samples>,
    /// Peak-to-sidelobe ratio of the strongest peak.
    pub psr: Option<Db>,
    /// Loopback level.
    pub loopback: Dbfs,
    /// Stimulus level.
    pub stimulus: Dbfs,
}

/// Timing frame metadata.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimingMeta {
    /// Status.
    pub status: TimingStatus,
    /// Newest window.
    pub window: Option<TimingWindow>,
}

/// Generator summary carried by keepalives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenSummary {
    /// Owner.
    pub owner: Option<ClientId>,
    /// The owner is connected on the rig's own host ([`crate::model::Generator::owner_local`]).
    pub owner_local: bool,
    /// Armed.
    pub armed: bool,
    /// Firing.
    pub firing: bool,
}

/// Keepalive (every 250 ms). Header `capture_wall_ns` equals `daemon_wall_ns`; header
/// `config_rev` equals `rev`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KaMeta {
    /// Current state rev.
    pub rev: Rev,
    /// Daemon wall clock at send (for the clock-offset estimate).
    pub daemon_wall_ns: WallNs,
    /// Timing summary.
    pub timing: TimingState,
    /// Generator owner/state.
    pub generator: GenSummary,
    /// The daemon's host and audio health, for a status meter.
    pub health: DaemonHealth,
}

/// How the daemon's host and its audio are holding up, carried by keepalives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DaemonHealth {
    /// The daemon's host name (a DNS label): a client on another machine names where these
    /// figures come from.
    pub host: String,
    /// Whole-host CPU use over about the last second, every process on every core, as a
    /// share of the host's capacity (1.0 = every core busy). `None` until the daemon has
    /// sampled twice, or where the system states none.
    pub cpu: Option<f64>,
    /// The open stream's audio; `None` with no stream open.
    pub audio: Option<AudioHealth>,
}

/// The open stream's audio callback load and xruns.
///
/// The load is the time the daemon's own audio callback takes as a share of the period
/// its blocks cover (1.0 = the whole period, where the next block is late). It is this
/// process's callback only: the rest of the audio graph (other JACK clients, the driver)
/// is not in it. Each figure is `None` when no callback was timed over its span (a replay
/// has no device callback).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioHealth {
    /// Mean load over about the last second.
    pub load: Option<f64>,
    /// The largest load of a single callback over about the last second.
    pub peak: Option<f64>,
    /// Mean load since the stream opened.
    pub stream_load: Option<f64>,
    /// The largest load of a single callback since the stream opened.
    pub stream_peak: Option<f64>,
    /// Xruns the backend reported since the stream opened.
    pub xruns: u64,
}

/// Per-kind metadata on the wire, keyed by kind: `{"tf": {…}}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum FrameMeta {
    /// TF.
    Tf(TfMeta),
    /// A stage's bins.
    Bins(super::BinsMeta),
    /// IR.
    Ir(IrMeta),
    /// RTA.
    Rta(RtaMeta),
    /// Spectrum.
    Spec(SpecMeta),
    /// SPL.
    Spl(SplMeta),
    /// Leq windows.
    Leq(LeqMeta),
    /// Band Leq.
    BandLeq(BandLeqMeta),
    /// Levels.
    Levels(LevelsMeta),
    /// Stream levels.
    StreamLevels(StreamLevelsMeta),
    /// Preview levels.
    PreviewLevels(PreviewLevelsMeta),
    /// A rig's own levels for another rig.
    SourceLevels(SourceLevelsMeta),
    /// Stream goniometer.
    StreamGonio(GonioMeta),
    /// Preview goniometer.
    PreviewGonio(GonioMeta),
    /// Timing.
    Timing(TimingMeta),
    /// Keepalive.
    Ka(KaMeta),
}

impl FrameMeta {
    /// The elements of an array that has one per `per`: `n` per column, the metadata's
    /// pairs or sources; `None` for a kind without them.
    pub(super) fn count(&self, per: super::ArrayPer, n: u32) -> Option<usize> {
        use super::ArrayPer;
        let (pairs, sources) = match self {
            Self::StreamLevels(m) => (m.pairs.len(), m.sources.len()),
            Self::SourceLevels(m) => (m.pairs.len(), m.sources.len()),
            Self::PreviewLevels(m) => (m.pairs.len(), m.sources.len()),
            _ if per == ArrayPer::Column => return Some(n as usize),
            _ => return None,
        };
        Some(match per {
            ArrayPer::Column => n as usize,
            ArrayPer::Pair => pairs,
            ArrayPer::Source => sources,
        })
    }

    pub(super) fn kind(&self) -> FrameKind {
        match self {
            Self::Tf(_) => FrameKind::Tf,
            Self::Bins(_) => FrameKind::Bins,
            Self::Ir(_) => FrameKind::Ir,
            Self::Rta(_) => FrameKind::Rta,
            Self::Spec(_) => FrameKind::Spec,
            Self::Spl(_) => FrameKind::Spl,
            Self::Leq(_) => FrameKind::Leq,
            Self::BandLeq(_) => FrameKind::BandLeq,
            Self::Levels(_) => FrameKind::Levels,
            Self::StreamLevels(_) => FrameKind::StreamLevels,
            Self::PreviewLevels(_) => FrameKind::PreviewLevels,
            Self::SourceLevels(_) => FrameKind::SourceLevels,
            Self::StreamGonio(_) => FrameKind::StreamGonio,
            Self::PreviewGonio(_) => FrameKind::PreviewGonio,
            Self::Timing(_) => FrameKind::Timing,
            Self::Ka(_) => FrameKind::Ka,
        }
    }
}

/// A keyframe's header: a msgpack array of these fields in this order (positional, as
/// are the per-kind metadata structs inside it; enum variants keep their names), followed in
/// the same part by the deflated intra payload. A frame goes out tens of times a second with
/// a payload often smaller than its field names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameHeader {
    /// Protocol version.
    pub v: u16,
    /// [`Form::Key`](super::Form::Key).
    pub form: super::Form,
    /// Kind; equals the topic's stream and the `meta` key.
    pub kind: FrameKind,
    /// Per-topic sequence number.
    pub seq: u64,
    /// Stream sample index of the newest sample in the frame.
    pub audio_sample: SampleIndex,
    /// Stream epoch.
    pub stream_epoch: StreamEpoch,
    /// Daemon incarnation.
    pub daemon_incarnation: DaemonIncarnation,
    /// What the DSP actually used: the newer of its measurement's `config_rev` and its
    /// delay's `rev` (transfer measurements), both store revs. A frame shows a
    /// measurement's configuration when this is ≥ its `config_rev`, and its delay when this
    /// is ≥ the delay's `rev`.
    pub config_rev: Rev,
    /// Sample index from which that config took effect.
    pub config_applied_at: SampleIndex,
    /// Daemon wall clock of the newest sample.
    pub capture_wall_ns: WallNs,
    /// Column grid, for frequency-domain kinds.
    pub grid_id: Option<GridId>,
    /// Protection state.
    pub protection: ProtectionFlags,
    /// Columns: the length of every column array.
    pub n: u32,
    /// Array parts, in part order.
    pub arrays: Vec<ArrayDesc>,
    /// Per-kind metadata.
    pub meta: FrameMeta,
    /// CRC-32 of the arrays' codes (`crate::packing`), in `arrays` order.
    pub crc: u32,
}
