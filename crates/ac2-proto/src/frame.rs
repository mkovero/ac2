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
    BandFraction, CalStatus, LevelScale, PeakWeighting, Smoothing, TimeWeighting, TimingState,
    TimingStatus, Weighting, Window,
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
    /// Input meters.
    Levels,
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
    /// TF model effective averages.
    EffAvg,
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
    /// Count (effective averages).
    Count,
    /// Linear, full scale = 1.
    FullScale,
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
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TfMeta {
    /// Delay applied to the reference.
    pub delay: Seconds,
    /// Display frozen.
    pub frozen: bool,
    /// Live smoothing applied.
    pub smoothing: Option<Smoothing>,
    /// A mic curve was subtracted from `mag`.
    pub mic_curve: bool,
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
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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
}

/// Spectrum metadata.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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
}

/// Input meters metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LevelsMeta {
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
    /// Levels.
    Levels(LevelsMeta),
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
            Self::Levels(_) => FrameKind::Levels,
            Self::Timing(_) => FrameKind::Timing,
            Self::Ka(_) => FrameKind::Ka,
        }
    }
}

/// The msgpack header part.
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
    /// Model effective averages, when published.
    pub eff_avg: Option<Vec<f32>>,
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

/// Spectrum frame.
#[derive(Debug, Clone, PartialEq)]
pub struct SpecFrame {
    /// Measurement.
    pub meas: MeasId,
    /// Metadata.
    pub meta: SpecMeta,
    /// Tone level per bin in `meta.scale`.
    pub level: Vec<f32>,
    /// Validity per bin.
    pub validity: Vec<ValidityMask>,
}

/// SPL frame (no arrays).
#[derive(Debug, Clone, PartialEq)]
pub struct SplFrame {
    /// Measurement.
    pub meas: MeasId,
    /// Readings.
    pub meta: SplMeta,
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
    /// `d/<meas>/levels`.
    Levels(LevelsFrame),
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
            Self::Levels(_) => FrameKind::Levels,
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
            Self::Levels(f) => data(f.meas, Stream::Levels),
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
            if let Some(e) = &f.eff_avg {
                cols.push((desc(ArrayName::EffAvg, Unit::Count), Col::F(e)));
            }
            cols.push((
                desc(ArrayName::Validity, Unit::Bitmask),
                Col::U(mask_slice(&f.validity)),
            ));
            FrameMeta::Tf(f.meta)
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
            FrameMeta::Rta(f.meta)
        }
        FrameData::Spec(f) => {
            cols.push((
                desc(ArrayName::Level, level_unit(f.meta.scale)),
                Col::F(&f.level),
            ));
            cols.push((
                desc(ArrayName::Validity, Unit::Bitmask),
                Col::U(mask_slice(&f.validity)),
            ));
            FrameMeta::Spec(f.meta)
        }
        FrameData::Spl(f) => FrameMeta::Spl(f.meta),
        FrameData::Levels(f) => {
            cols.push((desc(ArrayName::Peak, Unit::Dbfs), Col::F(&f.peak)));
            cols.push((desc(ArrayName::Rms, Unit::Dbfs), Col::F(&f.rms)));
            cols.push((
                desc(ArrayName::Clip, Unit::Bitmask),
                Col::U(mask_slice(&f.clip)),
            ));
            FrameMeta::Levels(f.meta.clone())
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
    let hb = rmp_serde::to_vec_named(&header).map_err(|e| EncodeError::Msgpack(e.to_string()))?;
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
        Stream::Levels => FrameKind::Levels,
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
            eff_avg: a.opt_f32(ArrayName::EffAvg, Unit::Count)?,
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
            meta,
            level: a.f32(ArrayName::Level, level_unit(meta.scale))?,
            validity: a.mask(ArrayName::Validity)?,
        }),
        FrameMeta::Spec(meta) => FrameData::Spec(SpecFrame {
            meas,
            meta,
            level: a.f32(ArrayName::Level, level_unit(meta.scale))?,
            validity: a.mask(ArrayName::Validity)?,
        }),
        FrameMeta::Spl(meta) => FrameData::Spl(SplFrame { meas, meta }),
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
