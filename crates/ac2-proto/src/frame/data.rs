//! Typed frames and data-socket messages.

use crate::event::Event;
use crate::grid::GridId;
use crate::topic::{Stream, Topic};
use crate::units::{DaemonIncarnation, MeasId, Rev, SampleIndex, SessionEpoch, WallNs};

use super::{
    BandLeqMeta, ClipFlags, FrameKind, GonioMeta, GonioPairMeta, IrMeta, KaMeta, LeqFlags, LeqMeta,
    LevelsMeta, PreviewLevelsMeta, ProtectionFlags, RtaMeta, SessionLevelsMeta, SpecMeta, SplMeta,
    TfMeta, TimingMeta, ValidityMask,
};

/// Header fields shared by every kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameStamp {
    /// See [`FrameHeader::seq`](super::FrameHeader::seq).
    pub seq: u64,
    /// See [`FrameHeader::audio_sample`](super::FrameHeader::audio_sample).
    pub audio_sample: SampleIndex,
    /// See [`FrameHeader::session_epoch`](super::FrameHeader::session_epoch).
    pub session_epoch: SessionEpoch,
    /// See [`FrameHeader::daemon_incarnation`](super::FrameHeader::daemon_incarnation).
    pub daemon_incarnation: DaemonIncarnation,
    /// See [`FrameHeader::config_rev`](super::FrameHeader::config_rev).
    pub config_rev: Rev,
    /// See [`FrameHeader::config_applied_at`](super::FrameHeader::config_applied_at).
    pub config_applied_at: SampleIndex,
    /// See [`FrameHeader::capture_wall_ns`](super::FrameHeader::capture_wall_ns).
    pub capture_wall_ns: WallNs,
    /// See [`FrameHeader::grid_id`](super::FrameHeader::grid_id).
    pub grid_id: Option<GridId>,
    /// See [`FrameHeader::protection`](super::FrameHeader::protection).
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
    /// An alignment math channel's view curves; `None` for every other transfer frame.
    pub align: Option<crate::model::AlignCurves>,
    /// An average's range across positions ([`crate::frame::MathState::range`]); `None` for
    /// every other transfer frame and below three positions.
    pub range: Option<crate::model::RangeBand>,
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

/// An SPL meter's band meter, published once a second: one column per window
/// ([`BandLeqMeta`]).
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

/// Meters of every input of the open session, independent of measurements. Beside the
/// sample peak, RMS and clip, the programme meters of docs/design/programme-meter.md §3:
/// the chosen ballistic's reading, the true peak, the daemon's max hold and over count per
/// channel; the pair readings (correlation) are in `meta.pair_levels`.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionLevelsFrame {
    /// Channels, the ballistic in force and the pairs' correlation.
    pub meta: SessionLevelsMeta,
    /// Peak per channel, dBFS (sample peak).
    pub peak: Vec<f32>,
    /// RMS per channel, dBFS.
    pub rms: Vec<f32>,
    /// Clip state per channel.
    pub clip: Vec<ClipFlags>,
    /// The chosen ballistic's reading per channel, dBFS (a peak for the quasi-peak types, the
    /// RMS-scaled mean for VU); where VU is chosen but not offered on the input
    /// ([`crate::cal::offers_vu`]) the true peak, so a rig setting never blanks a bar. Under
    /// a loudness ballistic ([`Ballistic::is_loudness`]) each input's true peak, dBTP: the
    /// loudness is per source, in `meta.loudness`.
    ///
    /// [`Ballistic::is_loudness`]: crate::model::Ballistic::is_loudness
    pub ppm: Vec<f32>,
    /// True peak per channel this frame, dBTP.
    pub true_peak: Vec<f32>,
    /// Highest `ppm` reading of the chosen ballistic since the last `levels.reset` (held per
    /// ballistic, so a ballistic change never splices one type's maximum onto another's);
    /// under a loudness ballistic the true-peak hold.
    pub max: Vec<f32>,
    /// Over events (true peak above the rig's threshold, counted once until it fell 1 dB
    /// below) since the last `levels.reset`.
    pub overs: Vec<u32>,
}

/// Meters of every input of a previewed device, with the same programme meters as
/// [`SessionLevelsFrame`] over the kinds and pairs `session.preview` declared.
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
    /// As [`SessionLevelsFrame::ppm`].
    pub ppm: Vec<f32>,
    /// As [`SessionLevelsFrame::true_peak`].
    pub true_peak: Vec<f32>,
    /// As [`SessionLevelsFrame::max`].
    pub max: Vec<f32>,
    /// As [`SessionLevelsFrame::overs`].
    pub overs: Vec<u32>,
}

/// Meter frames (levels and goniometer) per second at most. A meter read by eye gains
/// nothing above ~30 updates per second, and the 300 ms RMS barely moves between them. A
/// client sizes what it keeps of the frames (the goniometer's embers) from this rate.
pub const METER_FPS: u32 = 30;

/// Points per pair in a goniometer frame (docs/design/programme-meter.md §5): one pair is
/// drawn from a random position in each of this many bins of the frame's samples.
pub const GONIO_POINTS: usize = 256;

/// One pair's goniometer points this frame.
#[derive(Debug, Clone, PartialEq)]
pub struct GonioPair {
    /// Zero-based device input channel of the left side.
    pub left: u16,
    /// Zero-based device input channel of the right side.
    pub right: u16,
    /// The auto gain the points are divided by, dBFS of the pair magnitude `√(l² + r²)`
    /// (a full-scale mono signal reads +3.01).
    pub gain_dbfs: f32,
    /// [`GONIO_POINTS`] points `[x, y]`, rotated 45° so that mono stands upright and left
    /// leans left (`x = (r − l)/√2`, `y = (l + r)/√2`), −127 … 127 of the gain.
    pub points: Vec<[i8; 2]>,
}

/// Goniometer points of every pair (`session/gonio`, `preview/gonio`), at the meter rate.
#[derive(Debug, Clone, PartialEq)]
pub struct GonioFrame {
    /// Pairs, in the declared order.
    pub pairs: Vec<GonioPair>,
}

impl GonioFrame {
    /// The header meta: every pair's channels and gain.
    pub(super) fn meta(&self) -> GonioMeta {
        GonioMeta {
            pairs: self
                .pairs
                .iter()
                .map(|p| GonioPairMeta {
                    left: p.left,
                    right: p.right,
                    gain_dbfs: p.gain_dbfs,
                })
                .collect(),
        }
    }
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
    /// `session/gonio`.
    SessionGonio(GonioFrame),
    /// `preview/gonio`.
    PreviewGonio(GonioFrame),
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
            Self::SessionGonio(_) => FrameKind::SessionGonio,
            Self::PreviewGonio(_) => FrameKind::PreviewGonio,
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
            Self::SessionGonio(_) => Topic::SessionGonio,
            Self::PreviewGonio(_) => Topic::PreviewGonio,
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
