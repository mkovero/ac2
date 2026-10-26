//! Per-kind metadata and the msgpack header.

use serde::{Deserialize, Serialize};

use crate::grid::GridId;
use crate::model::{
    BackendKind, Ballistic, BandFraction, CalStatus, DeviceId, LeqJudgement, LevelScale, Operand,
    Pair, PairSide, PeakWeighting, PhaseBasis, PositionCorrection, PreviewInput, Smoothing,
    SmoothingFraction, SpecLevel, TimeWeighting, TimingState, TimingStatus, Unresolved, Weighting,
    Window,
};
use crate::units::{
    ClientId, DaemonIncarnation, Db, Dbfs, Dbtp, Hz, Lufs, Rev, SampleIndex, Samples, Seconds,
    SessionEpoch, WallNs,
};

use super::{ArrayDesc, FrameKind, ProtectionFlags};

/// TF metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TfMeta {
    /// Delay applied to the reference; for a math channel, the delay its phase is referred
    /// to.
    pub delay: Seconds,
    /// The part of `delay` that `delay.nudge` steps added to the arrival
    /// ([`crate::model::DelayState::nudged`]) when this frame was made; 0 for a math channel.
    /// A view's shared time base refers the curve to `delay − nudged`.
    pub nudged: Seconds,
    /// Live smoothing applied.
    pub smoothing: Option<Smoothing>,
    /// A mic curve was subtracted from `mag` (for a math channel: from every operand).
    pub mic_curve: bool,
    /// What a math channel combined; `None` for a transfer measurement.
    pub math: Option<Box<MathState>>,
    /// Where the columns are finer than the analysis stage serving them resolves; `None`
    /// for a math channel.
    pub unresolved: Option<Unresolved>,
}

/// What a math channel's frame combined.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MathState {
    /// Every operand of the expression, in expression order, with whether it went in.
    pub operands: Vec<OperandState>,
    /// What the result's phase is relative to.
    pub phase: PhaseBasis,
    /// An alignment's fit and its view's curves; `None` for every other expression.
    pub align: Option<Box<crate::model::AlignState>>,
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

/// One operand of a math channel in a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperandState {
    /// The operand.
    pub operand: Operand,
    /// Whether it went in, and why not.
    pub status: OperandStatus,
}

/// Whether an operand went into a math channel's frame. An operand is left out rather than
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
    pub sample_rate: Hz,
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
    /// What a math channel combined; `None` for an RTA measurement.
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
    /// Display smoothing applied to `level`: a smoothed bin is a fractional-octave power
    /// average of tone levels, not the tone level of the bin.
    pub smoothing: Option<SmoothingFraction>,
    /// What a math channel combined; `None` for a spectrum measurement.
    pub math: Option<Box<MathState>>,
    /// The bands behind a multi-resolution spectrum's columns; `None` for one FFT and a
    /// math channel.
    pub bands: Option<Box<SpecBands>>,
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

/// One stereo pair's correlation this frame (docs/design/programme-meter.md §4).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairLevels {
    /// Zero-based device input channel of the left side.
    pub left: u16,
    /// Zero-based device input channel of the right side.
    pub right: u16,
    /// Normalised zero-lag correlation over the rig's window, −1 … 1 (mono +1, wide 0,
    /// reversed −1); `None` while a side is below the gate.
    pub correlation: Option<f32>,
    /// The side below the rig's correlation gate (left checked first), when one is: a dead
    /// channel must not read 0 and look like wide stereo.
    pub quiet: Option<PairSide>,
}

/// What a loudness figure is measured over (docs/design/programme-meter.md §6.1): a
/// declared pair with both inputs captured (stereo, channel weights 1, 1), or one captured
/// input not in a pair (weight 1, so a mono signal reads 3 LU below the same on a pair).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LoudnessSource {
    /// A declared pair; zero-based device input channels, as in [`PairLevels`].
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

/// One source's BS.1770 / EBU Tech 3341 and 3342 loudness this frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceLoudness {
    /// What it is measured over.
    pub source: LoudnessSource,
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
    /// The daemon's clock at the reset, for the "since" line only.
    pub since: WallNs,
}

/// Session input meters metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionLevelsMeta {
    /// Device input channel of each column.
    pub channels: Vec<u16>,
    /// What the bars read (the rig's `levels.ballistic`), so a frame is self-describing:
    /// under a peak detector `ppm` and `max` are its reading; under `momentary` /
    /// `short_term` they are each input's true peak in dBTP and the bars read `loudness`.
    pub ballistic: Ballistic,
    /// Every declared pair whose both inputs are captured, in `session.inputs` order.
    pub pair_levels: Vec<PairLevels>,
    /// Every loudness source: pairs first in `pair_levels` order, then unpaired line and
    /// digital inputs ascending by channel; none for mic inputs.
    pub loudness: Vec<SourceLoudness>,
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
    /// What is on the inputs, as `session.preview` declared them.
    pub inputs: Vec<PreviewInput>,
    /// Pairs among them, as declared.
    pub pairs: Vec<Pair>,
    /// As [`SessionLevelsMeta::ballistic`].
    pub ballistic: Ballistic,
    /// As [`SessionLevelsMeta::pair_levels`], one per declared pair.
    pub pair_levels: Vec<PairLevels>,
    /// As [`SessionLevelsMeta::loudness`].
    pub loudness: Vec<SourceLoudness>,
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
    /// The open session's audio; `None` with no session open.
    pub audio: Option<AudioHealth>,
}

/// The open session's audio callback load and xruns.
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
    /// Mean load since the session opened.
    pub session_load: Option<f64>,
    /// The largest load of a single callback since the session opened.
    pub session_peak: Option<f64>,
    /// Xruns the backend reported since the session opened.
    pub xruns: u64,
}

/// Per-kind metadata on the wire, keyed by kind: `{"tf": {…}}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum FrameMeta {
    /// TF.
    Tf(TfMeta),
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
    /// Session levels.
    SessionLevels(SessionLevelsMeta),
    /// Preview levels.
    PreviewLevels(PreviewLevelsMeta),
    /// Session goniometer.
    SessionGonio(GonioMeta),
    /// Preview goniometer.
    PreviewGonio(GonioMeta),
    /// Timing.
    Timing(TimingMeta),
    /// Keepalive.
    Ka(KaMeta),
}

impl FrameMeta {
    pub(super) fn kind(&self) -> FrameKind {
        match self {
            Self::Tf(_) => FrameKind::Tf,
            Self::Ir(_) => FrameKind::Ir,
            Self::Rta(_) => FrameKind::Rta,
            Self::Spec(_) => FrameKind::Spec,
            Self::Spl(_) => FrameKind::Spl,
            Self::Leq(_) => FrameKind::Leq,
            Self::BandLeq(_) => FrameKind::BandLeq,
            Self::Levels(_) => FrameKind::Levels,
            Self::SessionLevels(_) => FrameKind::SessionLevels,
            Self::PreviewLevels(_) => FrameKind::PreviewLevels,
            Self::SessionGonio(_) => FrameKind::SessionGonio,
            Self::PreviewGonio(_) => FrameKind::PreviewGonio,
            Self::Timing(_) => FrameKind::Timing,
            Self::Ka(_) => FrameKind::Ka,
        }
    }
}

/// The msgpack header part: a msgpack array of these fields in this order (positional, as
/// are the per-kind metadata structs inside it; enum variants keep their names). A frame
/// goes out tens of times a second with a payload often smaller than its field names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameHeader {
    /// Protocol version.
    pub v: u16,
    /// Kind; equals the topic's stream and the `meta` key.
    pub kind: FrameKind,
    /// Per-topic sequence number.
    pub seq: u64,
    /// Session sample index of the newest sample in the frame.
    pub audio_sample: SampleIndex,
    /// Session epoch.
    pub session_epoch: SessionEpoch,
    /// Daemon incarnation.
    pub daemon_incarnation: DaemonIncarnation,
    /// Rev of the config the DSP actually used.
    pub config_rev: Rev,
    /// Sample index from which that config took effect.
    pub config_applied_at: SampleIndex,
    /// Daemon wall clock of the newest sample.
    pub capture_wall_ns: WallNs,
    /// Column grid, for frequency-domain kinds.
    pub grid_id: Option<GridId>,
    /// Protection state.
    pub protection: ProtectionFlags,
    /// Elements per array.
    pub n: u32,
    /// Array parts, in part order.
    pub arrays: Vec<ArrayDesc>,
    /// Per-kind metadata.
    pub meta: FrameMeta,
}
