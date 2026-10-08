//! Data frames: `[topic][msgpack header][array 0][array 1]…`.
//!
//! Each array part is `n` little-endian 4-byte elements (f32, or u32 for bitmasks), in the
//! order the header's `arrays` list describes. Column order = grid order. Invalid values
//! are NaN; where the reason matters a `validity` bitmask array says why.
//!
//! Decoding validates every size before it parses anything: part count, topic length,
//! header length, total length, then (after the bounded header parse) `n`, the array count
//! and every part's length. A malformed frame is an error value, never a panic.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::PROTO_VERSION;
use crate::event::{Event, EventError, decode_event, encode_event};
use crate::grid::GridId;
use crate::model::{
    BackendKind, BandFraction, CalStatus, DeviceId, LeqJudgement, LevelScale, Operand,
    PeakWeighting, PhaseBasis, PositionCorrection, Smoothing, SmoothingFraction, TimeWeighting,
    TimingState, TimingStatus, Weighting, Window,
};
use crate::topic::{Stream, Topic};
use crate::units::{
    ClientId, DaemonIncarnation, Db, Dbfs, Hz, MeasId, Rev, SampleIndex, Samples, Seconds,
    SessionEpoch, WallNs,
};

/// Largest header part, bytes.
pub const MAX_HEADER_BYTES: usize = 1024;
/// Largest `n` (columns / points / channels).
pub const MAX_N: u32 = 1 << 16;
/// Most array parts in one frame.
pub const MAX_ARRAYS: usize = 8;
/// Largest whole frame (all parts), bytes.
pub const MAX_FRAME_BYTES: usize = 2 << 20;

macro_rules! bitmask {
    ($(#[$doc:meta])* $name:ident { $($(#[$fdoc:meta])* $flag:ident = $bit:expr;)* }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub u32);

        impl $name {
            /// No bit set.
            pub const NONE: Self = Self(0);
            $($(#[$fdoc])* pub const $flag: Self = Self($bit);)*
            /// Every defined bit.
            pub const ALL: Self = Self(0 $(| $bit)*);
            /// `(name, bit)` of every flag, for documentation and other languages.
            pub const NAMED: &'static [(&'static str, u32)] = &[$((stringify!($flag), $bit)),*];

            /// True when all bits of `o` are set.
            pub fn contains(self, o: Self) -> bool {
                self.0 & o.0 == o.0
            }
            /// Union.
            pub fn with(self, o: Self) -> Self {
                Self(self.0 | o.0)
            }
            /// True when only defined bits are set.
            pub fn is_known(self) -> bool {
                self.0 & !Self::ALL.0 == 0
            }
        }
    };
}

bitmask!(
    /// Why a column has no value (0 = valid). Bits combine.
    ValidityMask {
        /// No bin of the serving stage falls in the column.
        THINNED = 1 << 0;
        /// Above the highest served frequency.
        OUT_OF_BAND = 1 << 1;
        /// No averaged block yet.
        SETTLING = 1 << 2;
        /// No reference energy in the column.
        NO_REFERENCE = 1 << 3;
        /// No measurement energy in the column.
        NO_MEASUREMENT = 1 << 4;
        /// Withheld by input protection (clip, missing reference).
        PROTECTED = 1 << 5;
        /// Below the noise / coherence floor.
        BELOW_FLOOR = 1 << 6;
        /// Band narrower than two bins.
        INSUFFICIENT_RESOLUTION = 1 << 7;
        /// Band above Nyquist.
        ABOVE_NYQUIST = 1 << 8;
        /// A math channel without the usable operands it needs: no value.
        FEW_OPERANDS = 1 << 9;
    }
);

bitmask!(
    /// Input protection state of the measurement producing the frame.
    ProtectionFlags {
        /// Reference below its floor: blocks rejected.
        NO_REFERENCE = 1 << 0;
        /// Measurement below its floor.
        NO_SIGNAL = 1 << 1;
        /// Clipping (held).
        CLIP = 1 << 2;
        /// Reference weak relative to the measurement.
        WEAK_REFERENCE = 1 << 3;
        /// Averages restarted after a stream discontinuity.
        DISCONTINUITY = 1 << 4;
        /// Reference and measurement look mis-patched: identical or near-perfectly
        /// correlated at zero lag (the same signal on both inputs; a real acoustic path
        /// always has propagation delay), or the reference silent while the measurement
        /// carries signal (reference and measurement swapped).
        CHECK_ROUTING = 1 << 5;
    }
);

bitmask!(
    /// State of a rolling Leq window. Its judgement ([`LeqFlags::judgement`]) is "no
    /// limit" without `LIMIT`, "not calibrated" with `LIMIT` but without `JUDGED`, else
    /// over, near or ok.
    LeqFlags {
        /// The window has a limit.
        LIMIT = 1 << 0;
        /// The limit is judged (the meter reads dB SPL).
        JUDGED = 1 << 1;
        /// Within the warn margin below the limit, or at it; or, filling, on course
        /// (`ON_COURSE`).
        NEAR = 1 << 2;
        /// Above the limit; while filling, the energy so far has spent the whole window's
        /// budget (it ends over even if the rest is silent).
        OVER = 1 << 3;
        /// No steady level over the horizon brings the window to its limit.
        CANNOT_RECOVER = 1 << 4;
        /// Part of the window was not measured (capture gaps, the meter stopped).
        INCOMPLETE = 1 << 5;
        /// Filling, with the Leq so far above the limit: at the pace so far the full window
        /// ends over it (with `NEAR`).
        ON_COURSE = 1 << 6;
    }
);

impl LeqFlags {
    /// The judgement the flags carry.
    pub fn judgement(self) -> LeqJudgement {
        if !self.contains(Self::LIMIT) {
            LeqJudgement::NoLimit
        } else if !self.contains(Self::JUDGED) {
            LeqJudgement::NotCalibrated
        } else if self.contains(Self::OVER) {
            LeqJudgement::Over
        } else if self.contains(Self::NEAR) {
            LeqJudgement::Near
        } else {
            LeqJudgement::Ok
        }
    }
}

bitmask!(
    /// Per-channel clip state.
    ClipFlags {
        /// Clipped in this frame's interval.
        CLIP = 1 << 0;
        /// Clip indicator held.
        HELD = 1 << 1;
    }
);

/// Frame kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameKind {
    /// Transfer function.
    Tf,
    /// Impulse response view.
    Ir,
    /// RTA.
    Rta,
    /// Spectrum.
    Spec,
    /// SPL meter.
    Spl,
    /// Rolling Leq windows of an SPL meter.
    Leq,
    /// 1/3-octave band Leq of an SPL meter's band meter.
    BandLeq,
    /// Input meters.
    Levels,
    /// Input meters of the open session.
    SessionLevels,
    /// Input meters of a device preview.
    PreviewLevels,
    /// Loopback timing.
    Timing,
    /// Keepalive.
    Ka,
}

/// Array name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArrayName {
    /// TF magnitude.
    Mag,
    /// TF phase.
    Phase,
    /// TF coherence γ².
    Coh,
    /// Validity bitmask.
    Validity,
    /// IR samples, linear.
    IrLinear,
    /// IR energy-time curve.
    IrEtc,
    /// Band / bin level.
    Level,
    /// Input peak.
    Peak,
    /// Input RMS.
    Rms,
    /// Input clip flags.
    Clip,
    /// Leq of each window.
    Leq,
    /// Seconds of each window elapsed.
    Elapsed,
    /// Seconds of each window measured.
    Measured,
    /// Headroom: steady level allowed over the horizon (NaN: none).
    Allowed,
    /// Seconds to recover at the limit (NaN unless the window cannot recover within the
    /// horizon).
    Recover,
    /// Leq window state.
    LeqFlags,
    /// Leq a window ends at if the rest of it is silent (its Leq once full).
    Least,
    /// Seconds until a filling window on course spends its budget (NaN otherwise).
    OverIn,
    /// A band window's limit in force (NaN: none).
    Limit,
}

/// Array unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    /// dB (ratio).
    Db,
    /// dBFS.
    Dbfs,
    /// dB SPL.
    DbSpl,
    /// Degrees.
    Deg,
    /// γ², 0…1.
    Coherence,
    /// Linear, full scale = 1.
    FullScale,
    /// Seconds.
    Seconds,
    /// Bitmask (u32 elements).
    Bitmask,
}

/// Element type of an array part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Elem {
    /// IEEE-754 binary32, little-endian.
    F32,
    /// Unsigned 32-bit, little-endian.
    U32,
}

/// Describes one array part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArrayDesc {
    /// Name.
    pub name: ArrayName,
    /// Unit.
    pub unit: Unit,
    /// Element type.
    pub elem: Elem,
}

// ---------------------------------------------------------------------------------------
// Per-kind metadata

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
    /// Display frozen.
    pub frozen: bool,
    /// Live smoothing applied.
    pub smoothing: Option<Smoothing>,
    /// A mic curve was subtracted from `mag` (for a math channel: from every operand).
    pub mic_curve: bool,
    /// What a math channel combined; `None` for a transfer measurement.
    pub math: Option<Box<MathState>>,
}

/// What a math channel's frame combined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MathState {
    /// Every operand of the expression, in expression order, with whether it went in.
    pub operands: Vec<OperandState>,
    /// What the result's phase is relative to.
    pub phase: PhaseBasis,
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
    /// Unit of `level` (tone level per bin, decision 4b).
    pub scale: LevelScale,
    /// Calibration applied (`uncalibrated` with dBFS).
    pub cal: CalStatus,
    /// A mic curve was subtracted from `level`.
    pub mic_curve: bool,
    /// Display smoothing applied to `level`: a smoothed bin is a fractional-octave power
    /// average of tone levels, not the tone level of the bin.
    pub smoothing: Option<SmoothingFraction>,
    /// What a math channel combined; `None` for a spectrum measurement.
    pub math: Option<Box<MathState>>,
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
    /// The shown bands, indices into [`crate::model::BAND_NOMINAL_HZ`], low to high.
    pub bands: Vec<u8>,
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
    /// Window length.
    pub duration: Seconds,
    /// Weighting of its band levels.
    pub weighting: Weighting,
    /// Seconds of the window elapsed (less than `duration` while it fills).
    pub elapsed: Seconds,
    /// Seconds of those measured.
    pub measured: Seconds,
    /// The limit set the window is judged by: night while it holds a night second.
    pub period: crate::model::BandPeriod,
    /// The set its headroom figures are computed against (once the horizon has passed).
    pub period_after_horizon: crate::model::BandPeriod,
    /// Index into the frame's `bands` of the window's worst band: the most severe
    /// judgement, then the furthest above (or least below) its limit; `None` when nothing
    /// is judged.
    pub worst: Option<u8>,
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
    SessionLevels(LevelsMeta),
    /// Preview levels.
    PreviewLevels(PreviewLevelsMeta),
    /// Timing.
    Timing(TimingMeta),
    /// Keepalive.
    Ka(KaMeta),
}

impl FrameMeta {
    fn kind(&self) -> FrameKind {
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

// ---------------------------------------------------------------------------------------
// Typed frames

/// Header fields shared by every kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameStamp {
    /// See [`FrameHeader::seq`].
    pub seq: u64,
    /// See [`FrameHeader::audio_sample`].
    pub audio_sample: SampleIndex,
    /// See [`FrameHeader::session_epoch`].
    pub session_epoch: SessionEpoch,
    /// See [`FrameHeader::daemon_incarnation`].
    pub daemon_incarnation: DaemonIncarnation,
    /// See [`FrameHeader::config_rev`].
    pub config_rev: Rev,
    /// See [`FrameHeader::config_applied_at`].
    pub config_applied_at: SampleIndex,
    /// See [`FrameHeader::capture_wall_ns`].
    pub capture_wall_ns: WallNs,
    /// See [`FrameHeader::grid_id`].
    pub grid_id: Option<GridId>,
    /// See [`FrameHeader::protection`].
    pub protection: ProtectionFlags,
}

/// TF frame.
#[derive(Debug, Clone, PartialEq)]
pub struct TfFrame {
    /// Measurement.
    pub meas: MeasId,
    /// Metadata.
    pub meta: TfMeta,
    /// Magnitude, dB.
    pub mag: Vec<f32>,
    /// Phase, degrees.
    pub phase: Vec<f32>,
    /// Coherence γ².
    pub coh: Vec<f32>,
    /// Validity per column.
    pub validity: Vec<ValidityMask>,
}

/// IR frame.
#[derive(Debug, Clone, PartialEq)]
pub struct IrFrame {
    /// Measurement.
    pub meas: MeasId,
    /// Metadata.
    pub meta: IrMeta,
    /// Decimated IR, linear full scale.
    pub linear: Vec<f32>,
    /// Energy-time curve, dB, when published.
    pub etc: Option<Vec<f32>>,
}

/// RTA frame.
#[derive(Debug, Clone, PartialEq)]
pub struct RtaFrame {
    /// Measurement.
    pub meas: MeasId,
    /// Metadata.
    pub meta: RtaMeta,
    /// Band power in `meta.scale`.
    pub level: Vec<f32>,
    /// Validity per band.
    pub validity: Vec<ValidityMask>,
}

/// Spectrum frame. Live, its columns are the FFT bins gathered for display
/// ([`crate::grid::GridDef::LogBins`]), each the highest tone level among its bins; a
/// capture keeps every bin ([`crate::grid::GridDef::Linear`]).
#[derive(Debug, Clone, PartialEq)]
pub struct SpecFrame {
    /// Measurement.
    pub meas: MeasId,
    /// Metadata.
    pub meta: SpecMeta,
    /// Tone level per column in `meta.scale`; NaN where there is none (a bin without
    /// power).
    pub level: Vec<f32>,
}

/// SPL frame (no arrays).
#[derive(Debug, Clone, PartialEq)]
pub struct SplFrame {
    /// Measurement.
    pub meas: MeasId,
    /// Readings.
    pub meta: SplMeta,
}

/// Rolling Leq windows of an SPL meter, one column per window, published once a second.
#[derive(Debug, Clone, PartialEq)]
pub struct LeqFrame {
    /// Measurement.
    pub meas: MeasId,
    /// Unit, calibration, horizon.
    pub meta: LeqMeta,
    /// Leq over the window (over the measured time; NaN before anything was measured).
    pub leq: Vec<f32>,
    /// Seconds of the window elapsed (less than its length while it fills).
    pub elapsed: Vec<f32>,
    /// Seconds of those measured.
    pub measured: Vec<f32>,
    /// Steady level allowed over the horizon to stay at the limit; NaN without a judged
    /// limit or when the window cannot recover within the horizon.
    pub allowed: Vec<f32>,
    /// Seconds to recover playing at the limit, when it cannot within the horizon; else
    /// NaN.
    pub recover: Vec<f32>,
    /// The Leq the window ends at if the rest of it is silent: the energy so far over the
    /// measured time plus the seconds left to fill (the Leq itself once full; NaN before
    /// anything was measured).
    pub least: Vec<f32>,
    /// Seconds until a filling window spends its budget at the pace so far, when it is on
    /// course (`ON_COURSE`); else NaN.
    pub over_in: Vec<f32>,
    /// State.
    pub flags: Vec<LeqFlags>,
}

/// An SPL meter's band meter, published once a second: one column per window and shown
/// band ([`BandLeqMeta`]).
#[derive(Debug, Clone, PartialEq)]
pub struct BandLeqFrame {
    /// Measurement.
    pub meas: MeasId,
    /// The windows, the bands, the prediction.
    pub meta: BandLeqMeta,
    /// Weighted band Leq of the window at the mic, §13 correction included; NaN before
    /// anything was measured.
    pub leq: Vec<f32>,
    /// The limit in force at the mic (the place's plus the attenuation with a transfer);
    /// NaN without one.
    pub limit: Vec<f32>,
    /// Steady level allowed over the horizon to stay at the limit in force once the
    /// horizon has passed; NaN without a judged limit or when it cannot recover.
    pub allowed: Vec<f32>,
    /// Seconds to recover playing at the limit, when it cannot within the horizon; else
    /// NaN.
    pub recover: Vec<f32>,
    /// State: the judgement, `ON_COURSE` while filling ([`LeqFlags`]).
    pub flags: Vec<LeqFlags>,
}

impl BandLeqFrame {
    /// Column of band `i` (into `meta.bands`) of window `w`.
    pub fn col(&self, w: usize, i: usize) -> usize {
        w * self.meta.bands.len() + i
    }
}

/// Input meters frame.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelsFrame {
    /// Measurement.
    pub meas: MeasId,
    /// Channels.
    pub meta: LevelsMeta,
    /// Peak per channel, dBFS (sample peak).
    pub peak: Vec<f32>,
    /// RMS per channel, dBFS.
    pub rms: Vec<f32>,
    /// Clip state per channel.
    pub clip: Vec<ClipFlags>,
}

/// Meters of every input of the open session, independent of measurements.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionLevelsFrame {
    /// Channels.
    pub meta: LevelsMeta,
    /// Peak per channel, dBFS (sample peak).
    pub peak: Vec<f32>,
    /// RMS per channel, dBFS.
    pub rms: Vec<f32>,
    /// Clip state per channel.
    pub clip: Vec<ClipFlags>,
}

/// Meters of every input of a previewed device.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewLevelsFrame {
    /// Device and channels.
    pub meta: PreviewLevelsMeta,
    /// Peak per channel, dBFS (sample peak).
    pub peak: Vec<f32>,
    /// RMS per channel, dBFS.
    pub rms: Vec<f32>,
    /// Clip state per channel.
    pub clip: Vec<ClipFlags>,
}

/// Every frame body.
#[derive(Debug, Clone, PartialEq)]
pub enum FrameData {
    /// `d/<meas>/tf`.
    Tf(TfFrame),
    /// `d/<meas>/ir`.
    Ir(IrFrame),
    /// `d/<meas>/rta`.
    Rta(RtaFrame),
    /// `d/<meas>/spec`.
    Spec(SpecFrame),
    /// `d/<meas>/spl`.
    Spl(SplFrame),
    /// `d/<meas>/leq`.
    Leq(Box<LeqFrame>),
    /// `d/<meas>/band_leq`.
    BandLeq(Box<BandLeqFrame>),
    /// `d/<meas>/levels`.
    Levels(LevelsFrame),
    /// `session/levels`.
    SessionLevels(SessionLevelsFrame),
    /// `session/preview`.
    PreviewLevels(PreviewLevelsFrame),
    /// `timing`.
    Timing(TimingMeta),
    /// `ka`.
    Ka(KaMeta),
}

/// A decoded data frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// Common header fields.
    pub stamp: FrameStamp,
    /// Body.
    pub data: FrameData,
}

impl FrameData {
    /// Kind.
    pub fn kind(&self) -> FrameKind {
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
            Self::Timing(_) => FrameKind::Timing,
            Self::Ka(_) => FrameKind::Ka,
        }
    }

    /// Topic the frame is published on.
    pub fn topic(&self) -> Topic {
        let data = |meas, stream| Topic::Data { meas, stream };
        match self {
            Self::Tf(f) => data(f.meas, Stream::Tf),
            Self::Ir(f) => data(f.meas, Stream::Ir),
            Self::Rta(f) => data(f.meas, Stream::Rta),
            Self::Spec(f) => data(f.meas, Stream::Spec),
            Self::Spl(f) => data(f.meas, Stream::Spl),
            Self::Leq(f) => data(f.meas, Stream::Leq),
            Self::BandLeq(f) => data(f.meas, Stream::BandLeq),
            Self::Levels(f) => data(f.meas, Stream::Levels),
            Self::SessionLevels(_) => Topic::SessionLevels,
            Self::PreviewLevels(_) => Topic::PreviewLevels,
            Self::Timing(_) => Topic::Timing,
            Self::Ka(_) => Topic::Ka,
        }
    }
}

impl Frame {
    /// Topic.
    pub fn topic(&self) -> Topic {
        self.data.topic()
    }
}

/// Anything received on the data socket.
// A message is decoded and at once moved on (an event into the mirror, a frame into the
// latest-frame map), never stored as this enum, so the size of its larger variant costs
// one move; boxing either variant would cost an allocation per message instead.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum DataMessage {
    /// A frame (including `ka`).
    Frame(Frame),
    /// A state event (`evt`).
    Event(Event),
}

// ---------------------------------------------------------------------------------------
// Errors

/// Why a frame was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DecodeError {
    /// Wrong number of parts.
    #[error("frame has {0} parts")]
    Parts(usize),
    /// Total size above [`MAX_FRAME_BYTES`].
    #[error("frame of {0} bytes exceeds the limit")]
    TooLarge(usize),
    /// Topic invalid.
    #[error(transparent)]
    Topic(#[from] crate::topic::TopicError),
    /// Header part above [`MAX_HEADER_BYTES`].
    #[error("header of {0} bytes exceeds the limit")]
    HeaderTooLarge(usize),
    /// Header not valid msgpack of the schema.
    #[error("malformed header: {0}")]
    Header(String),
    /// Another protocol version.
    #[error("frame at protocol version {theirs}, this build speaks {ours}")]
    VersionMismatch {
        /// This build.
        ours: u16,
        /// The frame.
        theirs: u16,
    },
    /// `n` above [`MAX_N`].
    #[error("n = {0} exceeds the limit")]
    TooManyElements(u32),
    /// Array descriptors and parts disagree in count.
    #[error("header lists {listed} arrays, frame has {parts}")]
    ArrayCount {
        /// In the header.
        listed: usize,
        /// Parts present.
        parts: usize,
    },
    /// A part's length is not `n × 4`.
    #[error("array {index}: {got} bytes, expected {expected}")]
    ArrayLength {
        /// Array index.
        index: usize,
        /// Expected.
        expected: usize,
        /// Got.
        got: usize,
    },
    /// Topic, `kind` and `meta` disagree.
    #[error("topic, kind and meta disagree")]
    KindMismatch,
    /// Arrays do not match the kind's schema.
    #[error("array schema: {0}")]
    Schema(String),
    /// A bitmask has undefined bits.
    #[error("undefined bits in {0:?}")]
    UnknownBits(ArrayName),
    /// Event body malformed.
    #[error(transparent)]
    Event(#[from] EventError),
}

/// Why a frame could not be encoded (it would be refused by the decoder).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EncodeError {
    /// Arrays of one frame have different lengths.
    #[error("array {name:?} has {got} elements, expected {expected}")]
    Length {
        /// Array.
        name: ArrayName,
        /// Expected (`n`).
        expected: usize,
        /// Got.
        got: usize,
    },
    /// `n` above [`MAX_N`].
    #[error("n = {0} exceeds the limit")]
    TooManyElements(usize),
    /// Header above [`MAX_HEADER_BYTES`].
    #[error("header of {0} bytes exceeds the limit")]
    HeaderTooLarge(usize),
    /// Serialization failed.
    #[error("encode: {0}")]
    Msgpack(String),
    /// Event encoding failed.
    #[error(transparent)]
    Event(#[from] EventError),
}

// ---------------------------------------------------------------------------------------
// Encode

enum Col<'a> {
    F(&'a [f32]),
    U(Vec<u32>),
}

fn desc(name: ArrayName, unit: Unit) -> ArrayDesc {
    let elem = if unit == Unit::Bitmask {
        Elem::U32
    } else {
        Elem::F32
    };
    ArrayDesc { name, unit, elem }
}

fn level_unit(scale: LevelScale) -> Unit {
    match scale {
        LevelScale::Dbfs => Unit::Dbfs,
        LevelScale::DbSpl => Unit::DbSpl,
    }
}

fn mask_slice<T: MaskBits>(v: &[T]) -> Vec<u32> {
    v.iter().map(T::to_u32).collect()
}

/// Bitmask newtypes stored in u32 array parts.
trait MaskBits: Sized {
    fn to_u32(&self) -> u32;
    fn from_u32(x: u32) -> Self;
    fn known(&self) -> bool;
}

macro_rules! mask_bits {
    ($t:ident) => {
        impl MaskBits for $t {
            fn to_u32(&self) -> u32 {
                self.0
            }
            fn from_u32(x: u32) -> Self {
                Self(x)
            }
            fn known(&self) -> bool {
                self.is_known()
            }
        }
    };
}

fn put_f32(out: &mut Vec<u8>, v: &[f32]) {
    if cfg!(target_endian = "little") {
        out.extend_from_slice(bytemuck::cast_slice(v));
    } else {
        for x in v {
            out.extend_from_slice(&x.to_le_bytes());
        }
    }
}

fn put_u32(out: &mut Vec<u8>, v: &[u32]) {
    if cfg!(target_endian = "little") {
        out.extend_from_slice(bytemuck::cast_slice(v));
    } else {
        for x in v {
            out.extend_from_slice(&x.to_le_bytes());
        }
    }
}

/// Encode a frame into its parts `[topic, header, arrays…]`.
pub fn encode_frame(frame: &Frame) -> Result<Vec<Vec<u8>>, EncodeError> {
    let mut cols: Vec<(ArrayDesc, Col<'_>)> = Vec::new();
    let meta = match &frame.data {
        FrameData::Tf(f) => {
            cols.push((desc(ArrayName::Mag, Unit::Db), Col::F(&f.mag)));
            cols.push((desc(ArrayName::Phase, Unit::Deg), Col::F(&f.phase)));
            cols.push((desc(ArrayName::Coh, Unit::Coherence), Col::F(&f.coh)));
            cols.push((
                desc(ArrayName::Validity, Unit::Bitmask),
                Col::U(mask_slice(&f.validity)),
            ));
            FrameMeta::Tf(f.meta.clone())
        }
        FrameData::Ir(f) => {
            cols.push((
                desc(ArrayName::IrLinear, Unit::FullScale),
                Col::F(&f.linear),
            ));
            if let Some(e) = &f.etc {
                cols.push((desc(ArrayName::IrEtc, Unit::Db), Col::F(e)));
            }
            FrameMeta::Ir(f.meta)
        }
        FrameData::Rta(f) => {
            cols.push((
                desc(ArrayName::Level, level_unit(f.meta.scale)),
                Col::F(&f.level),
            ));
            cols.push((
                desc(ArrayName::Validity, Unit::Bitmask),
                Col::U(mask_slice(&f.validity)),
            ));
            FrameMeta::Rta(f.meta.clone())
        }
        FrameData::Spec(f) => {
            cols.push((
                desc(ArrayName::Level, level_unit(f.meta.scale)),
                Col::F(&f.level),
            ));
            FrameMeta::Spec(f.meta.clone())
        }
        FrameData::Spl(f) => FrameMeta::Spl(f.meta),
        FrameData::BandLeq(f) => {
            let unit = level_unit(f.meta.scale);
            cols.push((desc(ArrayName::Leq, unit), Col::F(&f.leq)));
            cols.push((desc(ArrayName::Limit, unit), Col::F(&f.limit)));
            cols.push((desc(ArrayName::Allowed, unit), Col::F(&f.allowed)));
            cols.push((desc(ArrayName::Recover, Unit::Seconds), Col::F(&f.recover)));
            cols.push((
                desc(ArrayName::LeqFlags, Unit::Bitmask),
                Col::U(mask_slice(&f.flags)),
            ));
            FrameMeta::BandLeq(f.meta.clone())
        }
        FrameData::Leq(f) => {
            let unit = level_unit(f.meta.scale);
            cols.push((desc(ArrayName::Leq, unit), Col::F(&f.leq)));
            cols.push((desc(ArrayName::Elapsed, Unit::Seconds), Col::F(&f.elapsed)));
            cols.push((
                desc(ArrayName::Measured, Unit::Seconds),
                Col::F(&f.measured),
            ));
            cols.push((desc(ArrayName::Allowed, unit), Col::F(&f.allowed)));
            cols.push((desc(ArrayName::Recover, Unit::Seconds), Col::F(&f.recover)));
            cols.push((desc(ArrayName::Least, unit), Col::F(&f.least)));
            cols.push((desc(ArrayName::OverIn, Unit::Seconds), Col::F(&f.over_in)));
            cols.push((
                desc(ArrayName::LeqFlags, Unit::Bitmask),
                Col::U(mask_slice(&f.flags)),
            ));
            FrameMeta::Leq(f.meta)
        }
        FrameData::Levels(f) => {
            cols.push((desc(ArrayName::Peak, Unit::Dbfs), Col::F(&f.peak)));
            cols.push((desc(ArrayName::Rms, Unit::Dbfs), Col::F(&f.rms)));
            cols.push((
                desc(ArrayName::Clip, Unit::Bitmask),
                Col::U(mask_slice(&f.clip)),
            ));
            FrameMeta::Levels(f.meta.clone())
        }
        FrameData::SessionLevels(f) => {
            cols.push((desc(ArrayName::Peak, Unit::Dbfs), Col::F(&f.peak)));
            cols.push((desc(ArrayName::Rms, Unit::Dbfs), Col::F(&f.rms)));
            cols.push((
                desc(ArrayName::Clip, Unit::Bitmask),
                Col::U(mask_slice(&f.clip)),
            ));
            FrameMeta::SessionLevels(f.meta.clone())
        }
        FrameData::PreviewLevels(f) => {
            cols.push((desc(ArrayName::Peak, Unit::Dbfs), Col::F(&f.peak)));
            cols.push((desc(ArrayName::Rms, Unit::Dbfs), Col::F(&f.rms)));
            cols.push((
                desc(ArrayName::Clip, Unit::Bitmask),
                Col::U(mask_slice(&f.clip)),
            ));
            FrameMeta::PreviewLevels(f.meta.clone())
        }
        FrameData::Timing(m) => FrameMeta::Timing(*m),
        FrameData::Ka(m) => FrameMeta::Ka(m.clone()),
    };

    let n = match cols.first() {
        None => 0,
        Some((_, Col::F(v))) => v.len(),
        Some((_, Col::U(v))) => v.len(),
    };
    if n > MAX_N as usize {
        return Err(EncodeError::TooManyElements(n));
    }
    for (d, c) in &cols {
        let got = match c {
            Col::F(v) => v.len(),
            Col::U(v) => v.len(),
        };
        if got != n {
            return Err(EncodeError::Length {
                name: d.name,
                expected: n,
                got,
            });
        }
    }

    let s = &frame.stamp;
    let header = FrameHeader {
        v: PROTO_VERSION,
        kind: frame.data.kind(),
        seq: s.seq,
        audio_sample: s.audio_sample,
        session_epoch: s.session_epoch,
        daemon_incarnation: s.daemon_incarnation,
        config_rev: s.config_rev,
        config_applied_at: s.config_applied_at,
        capture_wall_ns: s.capture_wall_ns,
        grid_id: s.grid_id,
        protection: s.protection,
        n: n as u32,
        arrays: cols.iter().map(|(d, _)| *d).collect(),
        meta,
    };
    let hb = rmp_serde::to_vec(&header).map_err(|e| EncodeError::Msgpack(e.to_string()))?;
    if hb.len() > MAX_HEADER_BYTES {
        return Err(EncodeError::HeaderTooLarge(hb.len()));
    }

    let mut parts = Vec::with_capacity(2 + cols.len());
    parts.push(frame.topic().to_bytes());
    parts.push(hb);
    for (_, c) in &cols {
        let mut b = Vec::with_capacity(n * 4);
        match c {
            Col::F(v) => put_f32(&mut b, v),
            Col::U(v) => put_u32(&mut b, v),
        }
        parts.push(b);
    }
    Ok(parts)
}

/// Encode an event as data-socket parts `[evt, body]`.
pub fn encode_event_message(e: &Event) -> Result<Vec<Vec<u8>>, EncodeError> {
    Ok(vec![Topic::Evt.to_bytes(), encode_event(e)?])
}

// ---------------------------------------------------------------------------------------
// Decode

/// Little-endian f32 from bytes: zero-copy cast when aligned, per-element copy otherwise.
fn get_f32(b: &[u8]) -> Vec<f32> {
    if cfg!(target_endian = "little")
        && let Ok(v) = bytemuck::try_cast_slice::<u8, f32>(b)
    {
        return v.to_vec();
    }
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn get_u32(b: &[u8]) -> Vec<u32> {
    if cfg!(target_endian = "little")
        && let Ok(v) = bytemuck::try_cast_slice::<u8, u32>(b)
    {
        return v.to_vec();
    }
    b.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

struct Arrays<'a> {
    descs: &'a [ArrayDesc],
    parts: &'a [&'a [u8]],
    used: Vec<bool>,
}

impl<'a> Arrays<'a> {
    fn find(&mut self, name: ArrayName, unit: Unit) -> Result<Option<&'a [u8]>, DecodeError> {
        let mut hit = None;
        for (i, d) in self.descs.iter().enumerate() {
            if d.name == name {
                if hit.is_some() {
                    return Err(DecodeError::Schema(format!("duplicate {name:?}")));
                }
                let elem = desc(name, unit).elem;
                if d.unit != unit || d.elem != elem {
                    return Err(DecodeError::Schema(format!(
                        "{name:?} must be {unit:?}/{elem:?}, got {:?}/{:?}",
                        d.unit, d.elem
                    )));
                }
                hit = Some(i);
            }
        }
        Ok(hit.map(|i| {
            self.used[i] = true;
            self.parts[i]
        }))
    }

    fn f32(&mut self, name: ArrayName, unit: Unit) -> Result<Vec<f32>, DecodeError> {
        self.opt_f32(name, unit)?
            .ok_or_else(|| DecodeError::Schema(format!("missing {name:?}")))
    }

    fn opt_f32(&mut self, name: ArrayName, unit: Unit) -> Result<Option<Vec<f32>>, DecodeError> {
        Ok(self.find(name, unit)?.map(get_f32))
    }

    fn mask<T: MaskBits>(&mut self, name: ArrayName) -> Result<Vec<T>, DecodeError> {
        let b = self
            .find(name, Unit::Bitmask)?
            .ok_or_else(|| DecodeError::Schema(format!("missing {name:?}")))?;
        let v: Vec<T> = get_u32(b).into_iter().map(T::from_u32).collect();
        if v.iter().all(T::known) {
            Ok(v)
        } else {
            Err(DecodeError::UnknownBits(name))
        }
    }

    fn finish(self) -> Result<(), DecodeError> {
        match self.used.iter().position(|u| !u) {
            None => Ok(()),
            Some(i) => Err(DecodeError::Schema(format!(
                "unexpected array {:?}",
                self.descs[i].name
            ))),
        }
    }
}

fn stream_kind(s: Stream) -> FrameKind {
    match s {
        Stream::Tf => FrameKind::Tf,
        Stream::Ir => FrameKind::Ir,
        Stream::Rta => FrameKind::Rta,
        Stream::Spec => FrameKind::Spec,
        Stream::Spl => FrameKind::Spl,
        Stream::Leq => FrameKind::Leq,
        Stream::BandLeq => FrameKind::BandLeq,
        Stream::Levels => FrameKind::Levels,
    }
}

/// `v`, the first element of a header array: after the array marker (fixarray, array 16 or
/// array 32), a positive fixint, uint 8 or uint 16. `None` if the bytes are not that.
fn header_version(h: &[u8]) -> Option<u16> {
    let skip = match *h.first()? {
        0x90..=0x9f => 1,
        0xdc => 3,
        0xdd => 5,
        _ => return None,
    };
    let v = h.get(skip..)?;
    match *v.first()? {
        x @ 0x00..=0x7f => Some(u16::from(x)),
        0xcc => v.get(1).map(|x| u16::from(*x)),
        0xcd => Some(u16::from_be_bytes([*v.get(1)?, *v.get(2)?])),
        _ => None,
    }
}

/// Decode a data frame from its parts. Sizes are validated before anything is parsed.
pub fn decode_frame(parts: &[&[u8]]) -> Result<Frame, DecodeError> {
    if parts.len() < 2 || parts.len() > 2 + MAX_ARRAYS {
        return Err(DecodeError::Parts(parts.len()));
    }
    let total = parts.iter().fold(0usize, |a, p| a.saturating_add(p.len()));
    if total > MAX_FRAME_BYTES {
        return Err(DecodeError::TooLarge(total));
    }
    let topic = Topic::parse(parts[0])?;
    if parts[1].len() > MAX_HEADER_BYTES {
        return Err(DecodeError::HeaderTooLarge(parts[1].len()));
    }
    // The version comes first and is read on its own: another version may lay the rest of
    // the header out differently, and must be refused as such, not as malformed.
    if let Some(theirs) = header_version(parts[1])
        && theirs != PROTO_VERSION
    {
        return Err(DecodeError::VersionMismatch {
            ours: PROTO_VERSION,
            theirs,
        });
    }
    let h: FrameHeader =
        rmp_serde::from_slice(parts[1]).map_err(|e| DecodeError::Header(e.to_string()))?;
    if h.v != PROTO_VERSION {
        return Err(DecodeError::VersionMismatch {
            ours: PROTO_VERSION,
            theirs: h.v,
        });
    }
    if h.n > MAX_N {
        return Err(DecodeError::TooManyElements(h.n));
    }
    let arrays = &parts[2..];
    if h.arrays.len() != arrays.len() {
        return Err(DecodeError::ArrayCount {
            listed: h.arrays.len(),
            parts: arrays.len(),
        });
    }
    let expected = h.n as usize * 4;
    for (index, p) in arrays.iter().enumerate() {
        if p.len() != expected {
            return Err(DecodeError::ArrayLength {
                index,
                expected,
                got: p.len(),
            });
        }
    }
    let topic_kind = match topic {
        Topic::Data { stream, .. } => stream_kind(stream),
        Topic::SessionLevels => FrameKind::SessionLevels,
        Topic::PreviewLevels => FrameKind::PreviewLevels,
        Topic::Timing => FrameKind::Timing,
        Topic::Ka => FrameKind::Ka,
        Topic::Evt => return Err(DecodeError::KindMismatch),
    };
    if h.kind != topic_kind || h.meta.kind() != h.kind {
        return Err(DecodeError::KindMismatch);
    }
    if !h.protection.is_known() {
        return Err(DecodeError::Schema("undefined protection bits".into()));
    }

    let meas = match topic {
        Topic::Data { meas, .. } => meas,
        _ => MeasId(0),
    };
    let mut a = Arrays {
        descs: &h.arrays,
        parts: arrays,
        used: vec![false; arrays.len()],
    };
    let data = match h.meta {
        FrameMeta::Tf(meta) => FrameData::Tf(TfFrame {
            meas,
            meta,
            mag: a.f32(ArrayName::Mag, Unit::Db)?,
            phase: a.f32(ArrayName::Phase, Unit::Deg)?,
            coh: a.f32(ArrayName::Coh, Unit::Coherence)?,
            validity: a.mask(ArrayName::Validity)?,
        }),
        FrameMeta::Ir(meta) => FrameData::Ir(IrFrame {
            meas,
            meta,
            linear: a.f32(ArrayName::IrLinear, Unit::FullScale)?,
            etc: a.opt_f32(ArrayName::IrEtc, Unit::Db)?,
        }),
        FrameMeta::Rta(meta) => FrameData::Rta(RtaFrame {
            meas,
            level: a.f32(ArrayName::Level, level_unit(meta.scale))?,
            validity: a.mask(ArrayName::Validity)?,
            meta,
        }),
        FrameMeta::Spec(meta) => FrameData::Spec(SpecFrame {
            meas,
            level: a.f32(ArrayName::Level, level_unit(meta.scale))?,
            meta,
        }),
        FrameMeta::Spl(meta) => FrameData::Spl(SplFrame { meas, meta }),
        FrameMeta::BandLeq(meta) => {
            if meta.windows.len() * meta.bands.len() != h.n as usize {
                return Err(DecodeError::Schema(
                    "band_leq: windows.len() × bands.len() != n".into(),
                ));
            }
            let unit = level_unit(meta.scale);
            FrameData::BandLeq(Box::new(BandLeqFrame {
                meas,
                leq: a.f32(ArrayName::Leq, unit)?,
                limit: a.f32(ArrayName::Limit, unit)?,
                allowed: a.f32(ArrayName::Allowed, unit)?,
                recover: a.f32(ArrayName::Recover, Unit::Seconds)?,
                flags: a.mask(ArrayName::LeqFlags)?,
                meta,
            }))
        }
        FrameMeta::Leq(meta) => {
            let unit = level_unit(meta.scale);
            FrameData::Leq(Box::new(LeqFrame {
                meas,
                meta,
                leq: a.f32(ArrayName::Leq, unit)?,
                elapsed: a.f32(ArrayName::Elapsed, Unit::Seconds)?,
                measured: a.f32(ArrayName::Measured, Unit::Seconds)?,
                allowed: a.f32(ArrayName::Allowed, unit)?,
                recover: a.f32(ArrayName::Recover, Unit::Seconds)?,
                least: a.f32(ArrayName::Least, unit)?,
                over_in: a.f32(ArrayName::OverIn, Unit::Seconds)?,
                flags: a.mask(ArrayName::LeqFlags)?,
            }))
        }
        FrameMeta::Levels(meta) => {
            if meta.channels.len() != h.n as usize {
                return Err(DecodeError::Schema("levels: channels.len() != n".into()));
            }
            FrameData::Levels(LevelsFrame {
                meas,
                meta,
                peak: a.f32(ArrayName::Peak, Unit::Dbfs)?,
                rms: a.f32(ArrayName::Rms, Unit::Dbfs)?,
                clip: a.mask(ArrayName::Clip)?,
            })
        }
        FrameMeta::SessionLevels(meta) => {
            if meta.channels.len() != h.n as usize {
                return Err(DecodeError::Schema(
                    "session levels: channels.len() != n".into(),
                ));
            }
            FrameData::SessionLevels(SessionLevelsFrame {
                meta,
                peak: a.f32(ArrayName::Peak, Unit::Dbfs)?,
                rms: a.f32(ArrayName::Rms, Unit::Dbfs)?,
                clip: a.mask(ArrayName::Clip)?,
            })
        }
        FrameMeta::PreviewLevels(meta) => {
            if meta.channels.len() != h.n as usize {
                return Err(DecodeError::Schema(
                    "preview levels: channels.len() != n".into(),
                ));
            }
            FrameData::PreviewLevels(PreviewLevelsFrame {
                meta,
                peak: a.f32(ArrayName::Peak, Unit::Dbfs)?,
                rms: a.f32(ArrayName::Rms, Unit::Dbfs)?,
                clip: a.mask(ArrayName::Clip)?,
            })
        }
        FrameMeta::Timing(m) => FrameData::Timing(m),
        FrameMeta::Ka(m) => FrameData::Ka(m),
    };
    a.finish()?;
    Ok(Frame {
        stamp: FrameStamp {
            seq: h.seq,
            audio_sample: h.audio_sample,
            session_epoch: h.session_epoch,
            daemon_incarnation: h.daemon_incarnation,
            config_rev: h.config_rev,
            config_applied_at: h.config_applied_at,
            capture_wall_ns: h.capture_wall_ns,
            grid_id: h.grid_id,
            protection: h.protection,
        },
        data,
    })
}

/// Decode anything received on the data socket: `evt` → event, otherwise a frame.
pub fn decode_data_message(parts: &[&[u8]]) -> Result<DataMessage, DecodeError> {
    match parts.first().map(|t| Topic::parse(t)) {
        Some(Ok(Topic::Evt)) => {
            if parts.len() != 2 {
                return Err(DecodeError::Parts(parts.len()));
            }
            Ok(DataMessage::Event(decode_event(parts[1])?))
        }
        _ => decode_frame(parts).map(DataMessage::Frame),
    }
}

mask_bits!(ValidityMask);
mask_bits!(ClipFlags);
mask_bits!(LeqFlags);
