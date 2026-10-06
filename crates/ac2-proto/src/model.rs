//! State entities mirrored by clients, and the configuration enums they are made of.
//!
//! These mirror `ac2-core` / `ac2-audio` types on the wire. They are separate types (not
//! re-exports) so the protocol has no dependency on the DSP or audio crates and so a change
//! there cannot silently change the wire; the daemon converts at its boundary.

use serde::{Deserialize, Serialize};

use crate::grid::GridId;
use crate::units::{
    ClientId, Db, DbSpl, Dbfs, Degrees, Hz, MeasId, MvPerPa, Rev, SampleIndex, Samples, Seconds,
    SessionEpoch, SweepId, TraceId, Volts, WallNs,
};

// ---------------------------------------------------------------------------------------
// DSP configuration enums (mirrors of ac2-core)

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
    /// FFTs, a 1024-sample hop at 48 kHz for short ones; never more than `n / 2`).
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

// ---------------------------------------------------------------------------------------
// Devices and session (mirrors of ac2-audio)

/// Audio backend implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// JACK.
    Jack,
    /// cpal's OS host.
    Cpal,
    /// Simulated device.
    Fake,
    /// A raw capture file played back (`session.replay`); never listed by
    /// `session.devices`.
    Replay,
}

/// How capture and playback of one device relate in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockRelation {
    /// One callback, one clock.
    SingleCallback,
    /// One device, separate callbacks.
    SameDeviceSeparateCallbacks,
    /// Unknown; may drift.
    Unknown,
}

/// How the capture index is obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexExactness {
    /// Exact hardware/server counter.
    Exact,
    /// Running count; gaps estimated.
    Estimated,
}

/// Opaque device identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(pub String);

/// Inclusive range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RangeU32 {
    /// Smallest.
    pub min: u32,
    /// Largest.
    pub max: u32,
}

/// One direction of a device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectionInfo {
    /// Most channels.
    pub max_channels: u16,
    /// Supported sample-rate ranges, Hz.
    pub rates_hz: Vec<RangeU32>,
    /// Buffer sizes in frames, when the host can say.
    pub buffer_frames: Option<RangeU32>,
    /// Default rate, Hz.
    pub default_rate_hz: Option<u32>,
    /// Callback size the device runs at unless asked otherwise, when the host states one.
    pub default_buffer_frames: Option<u32>,
    /// One name per channel (`max_channels` of them) where the backend names its channels
    /// (JACK ports: alias or short name); `None` where it does not (cpal).
    pub channel_names: Option<Vec<String>>,
}

/// A device as listed by `session.devices`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceInfo {
    /// Backend.
    pub backend: BackendKind,
    /// Host name (`jack`, `alsa`, …).
    pub host: String,
    /// Identifier.
    pub id: DeviceId,
    /// Human-readable name.
    pub name: String,
    /// Capture side.
    pub input: Option<DirectionInfo>,
    /// Playback side.
    pub output: Option<DirectionInfo>,
    /// Duplex clock relation.
    pub duplex_clock: ClockRelation,
    /// Capture index exactness.
    pub index: IndexExactness,
    /// Non-fatal problems.
    pub notes: Vec<String>,
}

/// Whether a backend can be used now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Availability {
    /// Devices listed.
    Available,
    /// Nothing can be opened on it now.
    Unavailable {
        /// Why, for the operator (`JACK server not running`).
        reason: String,
    },
}

/// One backend the daemon offers, as listed by `session.devices`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendInfo {
    /// Backend.
    pub kind: BackendKind,
    /// What it is, for the operator.
    pub description: String,
    /// Usable now, or why not.
    pub availability: Availability,
    /// Its devices; empty while unavailable.
    pub devices: Vec<DeviceInfo>,
}

/// Which device to open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DeviceSelector {
    /// The host default.
    Default,
    /// A device by id.
    Id {
        /// Device.
        id: DeviceId,
    },
}

/// Generator output and its loopback: stimulus and reference leave the same converter and
/// the reference returns on an input, so all chain latency cancels (§4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopbackRoute {
    /// Output channel carrying the reference copy (zero-based).
    pub output: u16,
    /// Input channel it returns on (zero-based device channel).
    pub input: u16,
}

/// Arguments of `session.open`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionConfig {
    /// Backend the devices belong to; `None` = the daemon's default backend (the one it was
    /// started on).
    pub backend: Option<BackendKind>,
    /// Capture device.
    pub input_device: DeviceSelector,
    /// Playback device.
    pub output_device: DeviceSelector,
    /// Device input channels captured (zero-based).
    pub input_channels: Vec<u16>,
    /// Output channels of the stream.
    pub output_channels: u16,
    /// Requested sample rate; `None` = device default.
    pub sample_rate_hz: Option<u32>,
    /// Requested buffer size; `None` = device default.
    pub buffer_frames: Option<u32>,
    /// Reference loopback; `None` = no internal reference (decision 3b).
    pub loopback: Option<LoopbackRoute>,
}

/// An open session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenSession {
    /// Requested configuration.
    pub config: SessionConfig,
    /// Backend actually used.
    pub backend: BackendKind,
    /// Device actually opened for capture.
    pub input_device: DeviceId,
    /// Device actually opened for playback.
    pub output_device: DeviceId,
    /// Actual rate.
    pub sample_rate_hz: u32,
    /// Actual buffer size.
    pub buffer_frames: u32,
    /// Clock relation of the opened pair.
    pub clock: ClockRelation,
    /// When it opened.
    pub opened_at: WallNs,
    /// The recording it plays, for a replay session (`backend` is then `replay`, the
    /// devices are the ones that recorded it); `None` for a live device.
    pub replay: Option<ReplayInfo>,
}

/// Session entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    /// Current epoch.
    pub epoch: SessionEpoch,
    /// `None` while closed. Kept through an audio outage: the session stays open, with its
    /// configuration, until a client closes it.
    pub open: Option<OpenSession>,
    /// The open session's audio has stopped and the daemon is reopening it
    /// (`docs/design/audio-recovery.md`); `None` while audio runs or no session is open.
    pub stopped: Option<AudioStopped>,
}

/// An open session whose audio stopped: since when, why, and how reopening it goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioStopped {
    /// Wall time of the last audio received (the open, if none ever came).
    pub since: WallNs,
    /// Why the stream is considered stopped.
    pub cause: StopCause,
    /// Where reopening the same configuration stands.
    pub recovery: Recovery,
}

/// Why a session's audio stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum StopCause {
    /// No audio for `after` while the backend reported nothing (a hung audio server, a
    /// device reset under it).
    NotDelivering {
        /// The silence that counts as stopped for this stream.
        after_ms: u32,
    },
    /// The audio host ended the stream (its server shut down, the device was removed).
    HostEnded,
    /// The device or its configuration changed, and opening it again failed.
    DeviceChanged,
}

/// Reopening a stopped session's configuration, attempt by attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Recovery {
    /// Attempt `attempt` (1-based) is opening the stream since `started`; an audio server
    /// that hangs may keep it there.
    Opening {
        /// Attempt number.
        attempt: u32,
        /// When it began.
        started: WallNs,
    },
    /// Attempt `attempt` failed with `error`; the next begins at `next_at`.
    Waiting {
        /// Attempt number that failed.
        attempt: u32,
        /// What the backend said, which names what it waits for (a server to start, a
        /// device to return).
        error: String,
        /// When the next attempt begins.
        next_at: WallNs,
    },
}

/// Reply of `session.preview`: capture-only meters of a device before a session opens on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preview {
    /// Backend.
    pub backend: BackendKind,
    /// Device metered.
    pub device: DeviceId,
    /// Inputs metered: every input of the device, `0 .. channels`.
    pub channels: u16,
    /// Rate it runs at.
    pub sample_rate_hz: u32,
    /// The preview closes unless `session.preview` names this device again within this.
    pub expires_in_ms: u32,
}

/// How well one input matched the loopback-detection burst.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopbackCandidate {
    /// Zero-based device input.
    pub input: u16,
    /// Arrival after the burst left the output (exact on a single-callback clock; with
    /// separate callbacks it includes an offset common to every input).
    pub delay: Seconds,
    /// The same in samples.
    pub delay_samples: Samples,
    /// Normalised cross-correlation at that delay, −1 … 1 (negative: polarity inverted).
    pub correlation: f64,
    /// Level of the return relative to the burst; `None` for a silent input.
    pub gain: Option<Db>,
}

/// Reply of `session.detect_loopback`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopbackDetection {
    /// Backend.
    pub backend: BackendKind,
    /// Device.
    pub device: DeviceId,
    /// Output the burst played on (zero-based).
    pub output: u16,
    /// RMS level of the burst.
    pub level: Dbfs,
    /// Every input, best match first.
    pub ranked: Vec<LoopbackCandidate>,
    /// The best-ranked input when it correlates as a loopback cable does; `None` when no
    /// input does.
    pub loopback: Option<u16>,
    /// Clock relation of the stream the burst played on.
    pub clock: ClockRelation,
}

// ---------------------------------------------------------------------------------------
// Measurements

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
    /// calibrated (the log keeps what was measured, with the correction in force).
    pub position: Option<PositionCorrection>,
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
    /// Live spatial average of transfer measurements (publishes `tf` only;
    /// `docs/design/spatial-average.md`).
    SpatialAverage {
        /// Configuration.
        config: SpatialAverageConfig,
    },
}

impl MeasKind {
    /// Whether the measurement publishes a `tf` stream (a transfer function or a spatial
    /// average of them), so it is drawn, captured and compared as a transfer function.
    pub fn publishes_tf(&self) -> bool {
        matches!(
            self,
            MeasKind::Transfer { .. } | MeasKind::SpatialAverage { .. }
        )
    }
}

/// Live spatial average of transfer measurements: the daemon combines the members' current
/// results whenever it publishes, with the same mathematics as `trace.average`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpatialAverageConfig {
    /// Member transfer measurements, in display order: at least
    /// [`SpatialAverageConfig::MIN_MEMBERS`], at most [`SpatialAverageConfig::MAX_MEMBERS`],
    /// distinct, all on one grid.
    pub members: Vec<MeasId>,
    /// How the members are combined.
    pub method: AverageMethod,
    /// Delay the averaged phase is referred to.
    pub reference: AverageReference,
    /// Live smoothing of the average, if any (members are averaged unsmoothed).
    pub smoothing: Option<Smoothing>,
}

impl SpatialAverageConfig {
    /// Fewest members of an average, and fewest usable members for it to show a value.
    pub const MIN_MEMBERS: usize = 2;
    /// Most members of an average.
    pub const MAX_MEMBERS: usize = 16;

    /// `members` averaged by power, phase referred to the first member's delay, unsmoothed.
    pub fn power_of(members: Vec<MeasId>) -> Self {
        let reference = AverageReference::Member {
            meas: members.first().copied().unwrap_or(MeasId(0)),
        };
        Self {
            members,
            method: AverageMethod::Power,
            reference,
            smoothing: None,
        }
    }
}

/// Delay a live spatial average's phase is referred to.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AverageReference {
    /// The inserted delay of one member (the delay its newest result was measured with).
    Member {
        /// That member.
        meas: MeasId,
    },
    /// An explicit delay.
    Fixed {
        /// Delay.
        delay: Seconds,
    },
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

// Defaults every front end shares (`ac2 meas new`, the app's dialogs), so a measurement
// made from either is the same measurement.

impl LogGridSpec {
    /// Ten octaves around 1 kHz (≈ 31 Hz … 32 kHz) at `ppo` points per octave.
    pub fn ten_octaves(ppo: u32) -> Self {
        let p = i64::from(ppo);
        let k = |v: i64| i32::try_from(v).unwrap_or(if v < 0 { i32::MIN } else { i32::MAX });
        Self {
            ppo,
            k_min: k(-5 * p),
            k_max: k(5 * p - 1),
        }
    }
}

impl TransferConfig {
    /// Default points per octave of the grid.
    pub const DEFAULT_PPO: u32 = 48;
    /// Default FIFO blocks of the full-rate stage.
    pub const DEFAULT_BLOCKS: u32 = 8;

    /// `reference_input` → `measurement_input` with the default grid and averaging, no
    /// smoothing and equal confidence at every frequency.
    pub fn with_inputs(reference_input: u16, measurement_input: u16) -> Self {
        Self {
            reference_input,
            measurement_input,
            averaging: TfAveraging::Fifo {
                blocks: Self::DEFAULT_BLOCKS,
            },
            grid: LogGridSpec::ten_octaves(Self::DEFAULT_PPO),
            smoothing: None,
            depth: DepthPolicy::EqualConfidence,
        }
    }
}

impl DepthPolicy {
    /// Settle cap of [`DepthPolicy::FastLf`] when none is given, seconds.
    pub const DEFAULT_FAST_LF_S: f64 = 1.0;
}

impl SpectrumConfig {
    /// Default FFT length, samples.
    pub const DEFAULT_FFT_LEN: u32 = 65_536;

    /// `input` with the default FFT length, a Hann window and no averaging.
    pub fn on_input(input: u16) -> Self {
        Self {
            input,
            fft_len: Self::DEFAULT_FFT_LEN,
            window: Window::Hann,
            averaging: SpecAveraging::Off,
            smoothing: None,
        }
    }
}

impl RtaConfig {
    /// Default lowest band, Hz.
    pub const DEFAULT_F_LO_HZ: f64 = 20.0;
    /// Default highest band, Hz.
    pub const DEFAULT_F_HI_HZ: f64 = 20_000.0;

    /// `input` in `fraction` bands over 20 Hz … 20 kHz, Z-weighted, no averaging.
    pub fn on_input(input: u16, fraction: BandFraction) -> Self {
        Self {
            input,
            fraction,
            f_lo: Hz(Self::DEFAULT_F_LO_HZ),
            f_hi: Hz(Self::DEFAULT_F_HI_HZ),
            weighting: Weighting::Z,
            averaging: SpecAveraging::Off,
        }
    }
}

impl SplConfig {
    /// `input` with the given weightings, a C-weighted peak and the default Leq windows.
    pub fn on_input(input: u16, weighting: Weighting, time_weighting: TimeWeighting) -> Self {
        Self {
            input,
            weighting,
            time_weighting,
            peak_weighting: PeakWeighting::C,
            leq: LeqConfig::default_windows(),
            position: None,
        }
    }

    /// Why the configuration cannot run, if it cannot.
    pub fn check(&self) -> Result<(), String> {
        self.leq.check()?;
        if self.position.is_some_and(|p| !p.is_valid()) {
            return Err(format!(
                "a position correction is at most ±{} dB",
                PositionCorrection::MAX_DB
            ));
        }
        Ok(())
    }
}

impl LeqWindow {
    /// Longest window, s (one day).
    pub const MAX_SECONDS: u32 = 86_400;
    /// Default warn margin, dB.
    pub const DEFAULT_WARN_MARGIN_DB: f64 = 3.0;

    /// A-weighted, `minutes` long, no limit.
    pub fn minutes(minutes: u32) -> Self {
        Self {
            duration: Seconds(f64::from(minutes) * 60.0),
            weighting: Weighting::A,
            limit: None,
            warn_margin: Db(Self::DEFAULT_WARN_MARGIN_DB),
        }
    }

    /// Length in whole seconds, when it is one in range.
    pub fn seconds(&self) -> Option<u32> {
        let s = self.duration.0;
        (s.is_finite() && s >= 1.0 && s <= f64::from(Self::MAX_SECONDS) && s.fract() == 0.0)
            .then_some(s as u32)
    }

    /// Puts `windows` in display order: shorter first, equal lengths A, C, Z.
    pub fn sort(windows: &mut [LeqWindow]) {
        let rank = |w: Weighting| match w {
            Weighting::A => 0,
            Weighting::C => 1,
            Weighting::Z => 2,
        };
        windows.sort_by(|a, b| {
            a.duration
                .0
                .total_cmp(&b.duration.0)
                .then(rank(a.weighting).cmp(&rank(b.weighting)))
        });
    }

    /// Whether the window is well formed: whole seconds in range, a finite limit and a
    /// finite margin ≥ 0.
    pub fn is_valid(&self) -> bool {
        self.seconds().is_some()
            && self.limit.is_none_or(|l| l.0.is_finite())
            && self.warn_margin.0.is_finite()
            && self.warn_margin.0 >= 0.0
    }
}

impl LeqConfig {
    /// Most windows per meter.
    pub const MAX_WINDOWS: usize = 8;
    /// Default headroom horizon, s.
    pub const DEFAULT_HORIZON_S: f64 = 60.0;
    /// Longest headroom horizon, s.
    pub const MAX_HORIZON_S: f64 = 3600.0;

    /// LAeq over 1, 5, 10, 30 and 60 min, no limits, a one-minute horizon.
    pub fn default_windows() -> Self {
        Self {
            windows: [1, 5, 10, 30, 60].map(LeqWindow::minutes).to_vec(),
            horizon: Seconds(Self::DEFAULT_HORIZON_S),
            peaks: PeakLimits::default(),
        }
    }

    /// Horizon in whole seconds, when it is one in range.
    pub fn horizon_seconds(&self) -> Option<u32> {
        let h = self.horizon.0;
        (h.is_finite() && (1.0..=Self::MAX_HORIZON_S).contains(&h) && h.fract() == 0.0)
            .then_some(h as u32)
    }

    /// Why the configuration cannot run, if it cannot.
    pub fn check(&self) -> Result<(), String> {
        if self.windows.len() > Self::MAX_WINDOWS {
            return Err(format!(
                "at most {} Leq windows per meter",
                Self::MAX_WINDOWS
            ));
        }
        if let Some(w) = self.windows.iter().find(|w| !w.is_valid()) {
            return Err(format!(
                "Leq window of {} s: a window is 1 s … 24 h in whole seconds, with a finite \
                 limit and a warn margin of 0 dB or more",
                w.duration.0
            ));
        }
        if self.horizon_seconds().is_none() {
            return Err("the headroom horizon is 1 s … 1 h in whole seconds".into());
        }
        let bad = |p: &PeakLimit| {
            !(p.limit.0.is_finite() && p.warn_margin.0.is_finite() && p.warn_margin.0 >= 0.0)
        };
        if [self.peaks.lcpeak, self.peaks.lafmax]
            .iter()
            .flatten()
            .any(bad)
        {
            return Err(
                "a peak limit needs a finite limit and a warn margin of 0 dB or more".into(),
            );
        }
        Ok(())
    }
}

impl LeqPreset {
    /// Every preset, in the order the app offers them.
    pub const ALL: [LeqPreset; 17] = [
        LeqPreset::Din15905,
        LeqPreset::Swiss93,
        LeqPreset::Swiss96,
        LeqPreset::Swiss100,
        LeqPreset::Who,
        LeqPreset::France,
        LeqPreset::FranceChildren,
        LeqPreset::Flanders85,
        LeqPreset::Flanders95,
        LeqPreset::Flanders100,
        LeqPreset::Brussels85,
        LeqPreset::Brussels95,
        LeqPreset::Brussels100,
        LeqPreset::NetherlandsCovenant,
        LeqPreset::NetherlandsCovenant16To17,
        LeqPreset::NetherlandsCovenant14To15,
        LeqPreset::NetherlandsCovenantTo13,
    ];

    /// Display name.
    pub fn name(self) -> &'static str {
        match self {
            LeqPreset::Din15905 => "DIN 15905-5",
            LeqPreset::Swiss93 => "Swiss V-NISSG 93 dB",
            LeqPreset::Swiss96 => "Swiss V-NISSG 96 dB",
            LeqPreset::Swiss100 => "Swiss V-NISSG 100 dB",
            LeqPreset::Who => "WHO safe listening",
            LeqPreset::France => "France R1336-1",
            LeqPreset::FranceChildren => "France R1336-1, children up to 6",
            LeqPreset::Flanders85 => "Flanders VLAREM 85 dB",
            LeqPreset::Flanders95 => "Flanders VLAREM 95 dB",
            LeqPreset::Flanders100 => "Flanders VLAREM 100 dB",
            LeqPreset::Brussels85 => "Brussels 85 dB",
            LeqPreset::Brussels95 => "Brussels 95 dB",
            LeqPreset::Brussels100 => "Brussels 100 dB",
            LeqPreset::NetherlandsCovenant => "NL covenant 103 dB",
            LeqPreset::NetherlandsCovenant16To17 => "NL covenant, ages 16–17",
            LeqPreset::NetherlandsCovenant14To15 => "NL covenant, ages 14–15",
            LeqPreset::NetherlandsCovenantTo13 => "NL covenant, ages up to 13",
        }
    }

    /// Where the figures come from.
    pub fn source(self) -> &'static str {
        match self {
            LeqPreset::Din15905 => "DIN 15905-5:2007, loudest audience position",
            LeqPreset::Swiss93 | LeqPreset::Swiss96 | LeqPreset::Swiss100 => {
                "V-NISSG (SR 814.711), by event category"
            }
            LeqPreset::Who => "WHO Global standard for safe listening venues and events (2022)",
            LeqPreset::France => {
                "Code de la santé publique art. R1336-1 II 1° (décret n° 2017-1244), anywhere \
                 the public can be"
            }
            LeqPreset::FranceChildren => {
                "Code de la santé publique art. R1336-1 II 1° (décret n° 2017-1244), events \
                 aimed at children up to six"
            }
            LeqPreset::Flanders85 => {
                "VLAREM II art. 6.7.3 § 1: music in tents, open air and other public places"
            }
            LeqPreset::Flanders95 => {
                "VLAREM II art. 5.32.2.2bis § 1 (and 5.32.3.10): at the measuring position"
            }
            LeqPreset::Flanders100 => {
                "VLAREM II art. 5.32.2.2bis § 2: at the measuring position, LAeq 15 min shown"
            }
            LeqPreset::Brussels85 => {
                "Brussels-Capital arrêté du 26 janvier 2017 (son amplifié) art. 3"
            }
            LeqPreset::Brussels95 => {
                "Brussels-Capital arrêté du 26 janvier 2017 (son amplifié) art. 4"
            }
            LeqPreset::Brussels100 => {
                "Brussels-Capital arrêté du 26 janvier 2017 (son amplifié) art. 5"
            }
            LeqPreset::NetherlandsCovenant => {
                "Vierde convenant preventie gehoorschade versterkte muziek (Stcrt. 2024, 3787) \
                 art. 3.1.2, voluntary"
            }
            LeqPreset::NetherlandsCovenant16To17
            | LeqPreset::NetherlandsCovenant14To15
            | LeqPreset::NetherlandsCovenantTo13 => {
                "Vierde convenant preventie gehoorschade versterkte muziek (Stcrt. 2024, 3787) \
                 art. 3.1.3, voluntary"
            }
        }
    }

    /// The windows the preset sets, shortest first (equal lengths A, C, Z), each with the
    /// rule's limit; one without a limit is a window the rule wants shown.
    pub fn windows(self) -> Vec<LeqWindow> {
        use Weighting::{A, C};
        let w = |minutes: u32, weighting: Weighting, limit: Option<f64>| LeqWindow {
            weighting,
            limit: limit.map(DbSpl),
            ..LeqWindow::minutes(minutes)
        };
        match self {
            LeqPreset::Din15905 => vec![w(30, A, Some(99.0))],
            LeqPreset::Swiss93 => vec![w(60, A, Some(93.0))],
            LeqPreset::Swiss96 => vec![w(60, A, Some(96.0))],
            LeqPreset::Swiss100 => vec![w(60, A, Some(100.0))],
            LeqPreset::Who => vec![w(15, A, Some(100.0))],
            LeqPreset::France => vec![w(15, A, Some(102.0)), w(15, C, Some(118.0))],
            LeqPreset::FranceChildren => vec![w(15, A, Some(94.0)), w(15, C, Some(104.0))],
            LeqPreset::Flanders85 => vec![w(15, A, Some(85.0))],
            LeqPreset::Flanders95 => vec![w(15, A, Some(95.0))],
            LeqPreset::Flanders100 => vec![w(15, A, None), w(60, A, Some(100.0))],
            LeqPreset::Brussels85 => vec![w(15, A, Some(85.0))],
            LeqPreset::Brussels95 => vec![w(15, A, Some(95.0)), w(15, C, Some(110.0))],
            LeqPreset::Brussels100 => vec![w(60, A, Some(100.0)), w(60, C, Some(115.0))],
            LeqPreset::NetherlandsCovenant => vec![w(15, A, Some(103.0))],
            LeqPreset::NetherlandsCovenant16To17 => vec![w(15, A, Some(100.0))],
            LeqPreset::NetherlandsCovenant14To15 => vec![w(15, A, Some(96.0))],
            LeqPreset::NetherlandsCovenantTo13 => vec![w(15, A, Some(91.0))],
        }
    }

    /// The peak limits the rule sets (none for most: their texts limit Leq windows only).
    pub fn peaks(self) -> PeakLimits {
        let p = |l: f64| {
            Some(PeakLimit {
                limit: DbSpl(l),
                warn_margin: Db(LeqWindow::DEFAULT_WARN_MARGIN_DB),
            })
        };
        match self {
            LeqPreset::Din15905 => PeakLimits {
                lcpeak: p(135.0),
                lafmax: None,
            },
            LeqPreset::Swiss93 | LeqPreset::Swiss96 | LeqPreset::Swiss100 => PeakLimits {
                lcpeak: None,
                lafmax: p(125.0),
            },
            _ => PeakLimits::default(),
        }
    }

    /// The peak limits a meter has once `presets` are applied: exactly theirs, the lower
    /// where two set the same quantity (both rules met).
    pub fn peaks_of(presets: &[LeqPreset]) -> PeakLimits {
        let mut out = PeakLimits::default();
        for p in presets.iter().map(|p| p.peaks()) {
            for q in PeakQuantity::ALL {
                let slot = out.get_mut(q);
                *slot = match (*slot, p.get(q)) {
                    (Some(a), Some(b)) => Some(if b.limit.0 < a.limit.0 { b } else { a }),
                    (a, b) => a.or(b),
                };
            }
        }
        out
    }

    /// The windows a meter has once `presets` are applied: exactly theirs, shortest first
    /// (equal lengths A, C, Z), whatever it had before — a rule's limits on windows it does
    /// not define would read as part of it. A window two presets share gets the lower of
    /// their limits, so both rules are met, and a limit wins over a window shown without
    /// one. At most five distinct windows occur across all presets, well within
    /// [`LeqConfig::MAX_WINDOWS`].
    pub fn windows_of(presets: &[LeqPreset]) -> Vec<LeqWindow> {
        let mut out: Vec<LeqWindow> = Vec::new();
        for p in presets.iter().flat_map(|p| p.windows()) {
            match out
                .iter_mut()
                .find(|w| w.duration == p.duration && w.weighting == p.weighting)
            {
                Some(w) => {
                    w.limit = match (w.limit, p.limit) {
                        (Some(a), Some(b)) => Some(if b.0 < a.0 { b } else { a }),
                        (a, b) => a.or(b),
                    }
                }
                None => out.push(p),
            }
        }
        LeqWindow::sort(&mut out);
        out
    }
}

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
    /// Tracking enabled.
    pub tracking: bool,
    /// The last finding is ambiguous and the operator has not picked yet (decision 1c):
    /// tracking is paused until `delay.insert`, `delay.set` or a new `delay.find`.
    pub awaiting_pick: bool,
    /// Last finder result.
    pub last_finding: Option<DelayFinding>,
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

// ---------------------------------------------------------------------------------------
// Generator

/// Butterworth band-limit slope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOrder {
    /// 12 dB/oct.
    Second,
    /// 24 dB/oct.
    Fourth,
}

/// Optional band limits on noise.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BandLimit {
    /// High-pass corner.
    pub highpass: Option<Hz>,
    /// Low-pass corner.
    pub lowpass: Option<Hz>,
    /// Slope.
    pub order: FilterOrder,
}

/// Exponential sine sweep.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EssSpec {
    /// Start frequency.
    pub start: Hz,
    /// End frequency.
    pub end: Hz,
    /// Requested duration.
    pub duration: Seconds,
    /// Fade-in.
    pub fade_in: Seconds,
    /// Fade-out.
    pub fade_out: Seconds,
}

impl EssSpec {
    /// Default start frequency of a sweep measurement, Hz.
    pub const DEFAULT_START_HZ: f64 = 20.0;
    /// Default end frequency, Hz.
    pub const DEFAULT_END_HZ: f64 = 20_000.0;
    /// Default requested duration, s.
    pub const DEFAULT_DURATION_S: f64 = 3.0;

    /// A sweep from `start` to `end` in about `duration`, fading in over its first 1/6
    /// octave and out over its last 1/24 octave: it starts and stops without a step, and the
    /// fades stay short enough to leave the band's ends measured.
    pub fn with_fades(start: Hz, end: Hz, duration: Seconds) -> Self {
        let rate = duration.0 / (end.0 / start.0).ln();
        let fade = |octaves: f64| {
            let s = rate * std::f64::consts::LN_2 * octaves;
            Seconds(if s.is_finite() && s > 0.0 { s } else { 0.0 })
        };
        Self {
            start,
            end,
            duration,
            fade_in: fade(1.0 / 6.0),
            fade_out: fade(1.0 / 24.0),
        }
    }
}

/// Generator signal.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Signal {
    /// White noise.
    White,
    /// Pink noise.
    Pink,
    /// Periodic pink noise.
    PeriodicPink {
        /// Period (power of two).
        period: Samples,
    },
    /// Sine.
    Sine {
        /// Frequency.
        freq: Hz,
    },
    /// One exponential sine sweep.
    Ess {
        /// Sweep.
        sweep: EssSpec,
    },
}

/// Generator settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratorSettings {
    /// Signal.
    pub signal: Signal,
    /// RMS level.
    pub level: Dbfs,
    /// Band limits (noise only).
    pub band: Option<BandLimit>,
    /// Output channels carrying the stimulus (zero-based).
    pub outputs: Vec<u16>,
}

/// Full desired generator state for `gen.set`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratorDesired {
    /// Settings.
    pub settings: GeneratorSettings,
    /// Armed (does not emit).
    pub armed: bool,
    /// Firing; requires `armed`.
    pub firing: bool,
}

/// Audited generator action (Q6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenAction {
    /// Lease acquired.
    Acquire,
    /// Lease taken over with force.
    Force,
    /// Armed.
    Arm,
    /// Fired.
    Fire,
    /// Settings changed.
    Set,
    /// Stopped (universal).
    Stop,
    /// Lease released.
    Release,
    /// Lease expired.
    Expiry,
}

/// Last audited action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenAudit {
    /// Action.
    pub action: GenAction,
    /// Client that caused it; for `expiry`, the owner whose lease expired.
    pub client: Option<ClientId>,
    /// When.
    pub at: WallNs,
}

/// Generator entity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Generator {
    /// Lease holder.
    pub owner: Option<ClientId>,
    /// Armed.
    pub armed: bool,
    /// Emitting.
    pub firing: bool,
    /// Current settings, if ever set.
    pub settings: Option<GeneratorSettings>,
    /// Global maximum level; requests above it are refused.
    pub ceiling: Dbfs,
    /// Last audited action.
    pub last_action: Option<GenAudit>,
}

/// Lease grant (reply to `gen.acquire` / `gen.refresh`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lease {
    /// Token to carry in stimulus commands.
    pub lease_token: crate::units::LeaseToken,
    /// Time until expiry without refresh.
    pub expires_in_ms: u32,
}

// ---------------------------------------------------------------------------------------
// Traces (PLAN §3.5)

/// Polarity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Polarity {
    /// As measured.
    Normal,
    /// Inverted.
    Inverted,
}

/// Trace math operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MathOp {
    /// dB subtraction of magnitudes (A − B); phase dropped.
    MagnitudeDifference,
    /// Complex division A / B.
    ComplexDivision,
}

/// Imported file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportFormat {
    /// ac2 CSV (what `trace.export` writes; its header is checked).
    Ac2Csv,
    /// Analyzer text export: columns `freq mag [phase] [coherence]`, separated by commas,
    /// semicolons, tabs or spaces, with optional comment and header lines.
    AnalyzerText,
    /// ac2 CSV when the file starts with the ac2 header, analyzer text otherwise.
    Auto,
}

/// What an imported file becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportRole {
    /// A measured trace (independent time base, decision 8a).
    Trace,
    /// A target curve: magnitude only, drawn on the transfer pane.
    Target,
}

/// What a trace holds, and so where it is drawn and which operations apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TraceKind {
    /// Transfer function: magnitude dB, phase, coherence.
    Transfer,
    /// Target curve: magnitude dB only.
    Target,
    /// Narrowband spectrum (tone level).
    Spectrum {
        /// Level unit.
        scale: LevelScale,
    },
    /// Fractional-octave RTA (band power).
    Rta {
        /// Level unit.
        scale: LevelScale,
    },
    /// Sweep measurement: the fundamental's magnitude dB and phase (drawn like a transfer
    /// function), harmonic distortion per order and the impulse response
    /// ([`TraceData::sweep`]).
    Sweep,
}

/// Export format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// ac2 CSV.
    Ac2Csv,
}

/// Where a trace came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TraceSource {
    /// Captured live.
    Captured {
        /// Source measurement.
        meas: MeasId,
        /// Its name at capture.
        meas_name: String,
        /// Epoch it was captured in (shared time reference within it).
        epoch: SessionEpoch,
        /// Capture sample index.
        at_sample: SampleIndex,
    },
    /// Imported from a file; independent time reference (decision 8a).
    Imported {
        /// Original file name.
        file_name: String,
        /// Format actually parsed (never `auto`).
        format: ImportFormat,
        /// What the file held that the trace does not keep.
        notes: Vec<ImportNote>,
    },
    /// Captured from a live spatial average (shared time reference within `epoch`, like a
    /// capture).
    SpatialAverage {
        /// The average measurement.
        meas: MeasId,
        /// Its name at capture.
        meas_name: String,
        /// Epoch it was captured in.
        epoch: SessionEpoch,
        /// Capture sample index.
        at_sample: SampleIndex,
        /// Method.
        method: AverageMethod,
        /// The members averaged into the capture (members excluded at that moment are not
        /// listed).
        members: Vec<AverageMember>,
    },
    /// Average of other traces.
    Average {
        /// Inputs.
        traces: Vec<TraceId>,
        /// Method.
        method: AverageMethod,
        /// Phase reference.
        reference: DelayReference,
    },
    /// A − B.
    Math {
        /// A.
        a: TraceId,
        /// B.
        b: TraceId,
        /// Operation.
        op: MathOp,
    },
    /// Sweep measurement (`ir.capture`).
    IrCapture {
        /// The run that made it.
        run: SweepId,
        /// Epoch (shared time reference within it, like a capture).
        epoch: SessionEpoch,
        /// Sweep played.
        sweep: EssSpec,
        /// Level played.
        level: Dbfs,
        /// Sweeps averaged.
        repeats: u8,
        /// Reference input.
        reference_input: u16,
        /// Measurement input.
        measurement_input: u16,
    },
}

impl TraceSource {
    /// The session epoch whose time base the trace's phase is in: captures (live, spatial
    /// average or sweep) share their epoch's; every other source is independent
    /// (decision 8a).
    pub fn shared_epoch(&self) -> Option<SessionEpoch> {
        match self {
            TraceSource::Captured { epoch, .. }
            | TraceSource::SpatialAverage { epoch, .. }
            | TraceSource::IrCapture { epoch, .. } => Some(*epoch),
            TraceSource::Imported { .. }
            | TraceSource::Average { .. }
            | TraceSource::Math { .. } => None,
        }
    }
}

/// A member of a captured spatial average.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AverageMember {
    /// The member measurement.
    pub meas: MeasId,
    /// Its name at capture.
    pub name: String,
}

/// Something an imported file held that the trace does not keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportNote {
    /// A sweep export without its analysis facts and impulse response (written before
    /// they were exported): imported as its transfer function, the distortion dropped.
    SweepWithoutAnalysis,
    /// A sweep export whose rows are off the grid in its header: the response was
    /// resampled, and distortion is never resampled, so it was dropped.
    SweepOffGrid,
    /// The export showed a mic curve applied afterwards as a display edit; its columns are
    /// as measured, without the curve (apply it again with `trace.mic_curve`).
    MicCurveNotApplied,
}

/// Display colour, 8-bit sRGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rgb {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

/// Calibration state of a trace at capture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CalState {
    /// Uncalibrated (dBFS).
    Uncalibrated,
    /// Calibrated with this entry.
    Calibrated {
        /// The calibration used.
        key: CalKey,
        /// Its sensitivity.
        sensitivity: Db,
        /// When the calibration was taken.
        calibrated_at: WallNs,
    },
}

/// Mic in use for a trace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MicState {
    /// Mic name.
    pub name: String,
    /// The curve the live measurement had subtracted from the captured columns, `None`
    /// when none applied.
    pub curve: Option<MicCurveRef>,
}

/// A mic curve applied to a stored trace after capture (`trace.mic_curve`): a display
/// edit. The stored columns stay as measured; the daemon subtracts the curve, normalised to
/// 0 dB at `f_norm`, when it serves the trace's data (after the display smoothing), and
/// keeps the curve's points with the trace, so it survives a later change to the
/// calibration store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceMicCurve {
    /// Mic name of the calibration entry the curve came from.
    pub mic: String,
    /// The curve as the calibration store had it.
    pub curve: MicCurveRef,
    /// Normalisation frequency: the correction is 0 dB here.
    pub f_norm: Hz,
}

/// Operator-editable trace properties (`trace.update` replaces all of them).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceEdit {
    /// Name.
    pub name: String,
    /// Colour.
    pub color: Rgb,
    /// Shown.
    pub visible: bool,
    /// Protected from edits and deletion.
    pub locked: bool,
    /// Display order (lower first).
    pub order: u32,
    /// Magnitude offset.
    pub offset: Db,
    /// Polarity.
    pub polarity: Polarity,
    /// Per-trace delay nudge on top of the measured delay (decision 8a).
    pub delay_nudge: Seconds,
    /// Slot 1…9 the trace occupies (Ctrl+1…9 in the UI); a slot holds at most one trace.
    pub slot: Option<u8>,
    /// Display smoothing (transfer and spectrum traces): applied when the daemon serves the
    /// trace's data. The stored columns stay unsmoothed, so it can be changed at any time; a
    /// capture starts with the smoothing its measurement had. A spectrum has no phase: its
    /// power is smoothed in either mode.
    pub smoothing: Option<Smoothing>,
}

/// Trace metadata entity (mandatory metadata of PLAN §3.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceMeta {
    /// Id.
    pub id: TraceId,
    /// Editable properties.
    pub edit: TraceEdit,
    /// Content.
    pub kind: TraceKind,
    /// Origin.
    pub source: TraceSource,
    /// Grid of the stored data.
    pub grid_id: GridId,
    /// Delay the phase is referred to: the measured delay at capture; for an average the
    /// common reference delay; for an import the delay its ac2 CSV header states (0 for
    /// other files).
    pub delay: Seconds,
    /// Averaging depth policy at capture (transfer captures).
    pub depth: Option<DepthPolicy>,
    /// Calibration at capture.
    pub cal: CalState,
    /// Mic at capture.
    pub mic: Option<MicState>,
    /// A mic curve applied after capture (display edit, [`TraceMicCurve`]); never set
    /// while `mic.curve` is (the columns carry that curve already).
    pub mic_curve: Option<Box<TraceMicCurve>>,
    /// When captured / created.
    pub created_at: WallNs,
}

/// A session saved by `file.save` (reply to `file.save` / `file.load`, rows of `file.list`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionFile {
    /// Name (the directory name).
    pub name: String,
    /// Directory on the daemon host.
    pub path: String,
    /// When it was saved.
    pub saved_at: WallNs,
    /// Measurements in it.
    pub measurements: u32,
    /// Traces in it.
    pub traces: u32,
}

/// Which session directory `file.save` / `file.load` use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionRef {
    /// A name in the daemon's session directory (letters, digits, ` `, `-`, `_`, `.`).
    Name {
        /// Name.
        name: String,
    },
    /// A directory path on the daemon host (local transports only).
    Path {
        /// Path.
        path: String,
    },
}

/// Stored trace data (reply to `trace.get`): the stored columns with the trace's display
/// smoothing (`meta.edit.smoothing`) and then its mic curve (`meta.mic_curve`) applied; the
/// curve also corrects a sweep's distortion levels and floors (each order re the
/// fundamental at its own frequency). Column order = grid order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceData {
    /// Metadata.
    pub meta: TraceMeta,
    /// Magnitude, dB.
    pub mag_db: Vec<f32>,
    /// Phase, degrees; `None` when the trace has no phase.
    pub phase_deg: Option<Vec<f32>>,
    /// Coherence γ²; `None` when not available.
    pub coherence: Option<Vec<f32>>,
    /// Distortion and impulse response of a [`TraceKind::Sweep`] trace.
    pub sweep: Option<SweepData>,
}

// ---------------------------------------------------------------------------------------
// Sweep measurement (`ir.capture`, docs/design/sweep-distortion.md)

/// Which inputs a sweep records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SweepInputs {
    /// The reference and measurement inputs of a transfer measurement.
    Measurement {
        /// The measurement.
        meas: MeasId,
    },
    /// Inputs by number (zero-based device inputs).
    Channels {
        /// Reference (loopback).
        reference: u16,
        /// Measurement (mic).
        measurement: u16,
    },
}

/// Arguments of `ir.capture`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SweepRequest {
    /// Inputs recorded.
    pub inputs: SweepInputs,
    /// Output channels carrying the sweep (zero-based): the speaker's and the loopback's.
    pub outputs: Vec<u16>,
    /// RMS level of the sweep's constant-envelope part; refused when absent (there is no
    /// default level).
    pub level: Option<Dbfs>,
    /// The sweep.
    pub sweep: EssSpec,
    /// Sweeps played and averaged, 1 … [`SweepRequest::MAX_REPEATS`].
    pub repeats: u8,
    /// Linear-response gate after the arrival; `None` = the whole response up to the noise
    /// window.
    pub gate: Option<Seconds>,
    /// Silence recorded after each sweep (the room's decay and its noise; room parameters
    /// are computed up to its end), at most [`SweepRequest::MAX_TAIL`]; `None` or anything
    /// shorter = the shortest the analysis needs (1 s or more).
    pub tail: Option<Seconds>,
}

impl SweepRequest {
    /// Longest silence after each sweep.
    pub const MAX_TAIL: Seconds = Seconds(20.0);

    /// Most repeats.
    pub const MAX_REPEATS: u8 = 8;
}

/// Why a sweep run ended without a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SweepFailure {
    /// `gen.stop`, `gen.release` or a forced takeover stopped it.
    Stopped,
    /// The stimulus lease expired.
    LeaseExpired,
    /// The session closed or reopened.
    SessionClosed,
    /// Audio was lost while recording (an xrun or overflow).
    Dropout,
    /// The reference input carries no sweep.
    NoReference,
    /// The analysis refused the recording.
    Analysis,
}

/// Where a sweep run is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SweepStatus {
    /// Playing and recording sweep `repeat` (1-based).
    Playing {
        /// Repeat.
        repeat: u8,
    },
    /// Recorded; the analysis runs.
    Analysing,
    /// Stored as a sweep trace.
    Done {
        /// The trace.
        trace: TraceId,
    },
    /// Ended without a result; its audio is discarded.
    Failed {
        /// Why.
        reason: SweepFailure,
        /// Detail for the operator.
        msg: String,
    },
}

/// The latest sweep run (`ir.capture`), mirrored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SweepRun {
    /// Id.
    pub id: SweepId,
    /// Client that started it.
    pub owner: ClientId,
    /// Name of the trace it makes.
    pub name: String,
    /// Reference input.
    pub reference_input: u16,
    /// Measurement input.
    pub measurement_input: u16,
    /// Outputs playing it.
    pub outputs: Vec<u16>,
    /// Level.
    pub level: Dbfs,
    /// Sweep as requested.
    pub sweep: EssSpec,
    /// Actual sweep duration (the rate constant is rounded).
    pub sweep_duration: Seconds,
    /// Silence after each sweep.
    pub post_roll: Seconds,
    /// Sweeps.
    pub repeats: u8,
    /// Linear gate.
    pub gate: Option<Seconds>,
    /// Status.
    pub status: SweepStatus,
    /// When it started.
    pub started_at: WallNs,
}

impl SweepRun {
    /// Total playing time: every sweep with its silence.
    pub fn total(&self) -> Seconds {
        Seconds(f64::from(self.repeats) * (self.sweep_duration.0 + self.post_roll.0))
    }

    /// Still playing or analysing.
    pub fn active(&self) -> bool {
        matches!(
            self.status,
            SweepStatus::Playing { .. } | SweepStatus::Analysing
        )
    }
}

/// A distortion curve on the trace's grid (fundamental frequency per column).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DistortionCurve {
    /// Level re the fundamental, dB; NaN where not measured.
    pub level_db: Vec<f32>,
    /// Noise in the same window re the fundamental, dB; NaN where not measured.
    pub floor_db: Vec<f32>,
}

impl DistortionCurve {
    /// Column `i` counts as distortion: measured and at least `margin` above its floor
    /// ([`SweepInfo::floor_margin`]). Otherwise it reads "< floor".
    pub fn valid(&self, i: usize, margin: Db) -> bool {
        let (Some(l), Some(f)) = (self.level_db.get(i), self.floor_db.get(i)) else {
            return false;
        };
        l.is_finite() && (!f.is_finite() || f64::from(*l) >= f64::from(*f) + margin.0)
    }
}

/// One harmonic order's distortion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarmonicCurve {
    /// Order (2 = second harmonic).
    pub order: u8,
    /// Curve.
    pub curve: DistortionCurve,
}

/// The impulse response of a sweep: from the highest order's window to the end of the
/// linear window, decimated peak-preserving.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SweepIr {
    /// Time of point 0 re the arrival.
    pub t0: Seconds,
    /// Point spacing.
    pub dt: Seconds,
    /// Signed extreme per point (unit: the reference's level).
    pub linear: Vec<f32>,
    /// Hilbert envelope maximum per point, dB.
    pub etc_db: Vec<f32>,
}

/// What the analysis found and used.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SweepInfo {
    /// Sample rate.
    pub sample_rate: Hz,
    /// Rate constant L of the sweep: harmonic k's impulse sits at −L·ln k.
    pub rate: Seconds,
    /// Actual sweep duration.
    pub duration: Seconds,
    /// Sweeps averaged.
    pub repeats: u8,
    /// Arrival of the linear response re the reference (the trace's `delay`).
    pub arrival: Seconds,
    /// The reference's sweep re the emitted level (loopback gain).
    pub reference_level: Db,
    /// Harmonic window before `t_k`.
    pub window_pre: Seconds,
    /// Harmonic window after `t_k`.
    pub window_post: Seconds,
    /// Linear window before the arrival.
    pub gate_pre: Seconds,
    /// Linear window after the arrival.
    pub gate: Seconds,
    /// How far above its noise floor a distortion point must be to count.
    pub floor_margin: Db,
    /// A sample of either input reached full scale.
    pub clipped: bool,
}

/// Distortion and impulse response of a sweep trace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SweepData {
    /// H2 … H5.
    pub harmonics: Vec<HarmonicCurve>,
    /// Total harmonic distortion (power sum of the orders in band).
    pub thd: DistortionCurve,
    /// Impulse response.
    pub ir: SweepIr,
    /// Analysis facts.
    pub info: SweepInfo,
    /// ISO 3382-1 room parameters of the impulse response; `None` for a sweep imported from
    /// an export written without them.
    pub room: Option<RoomAcoustics>,
}

// ---------------------------------------------------------------------------------------
// Room acoustics (ISO 3382-1, docs/design/room-metrics.md)

/// Why a room parameter is not given.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoomRefusal {
    /// The band carries no decay (no energy, or none falling).
    NoDecay,
    /// The decay meets the noise too soon: the parameter needs `needed` of decay range.
    InsufficientRange {
        /// Decay range measured.
        range: Db,
        /// Decay range needed.
        needed: Db,
    },
    /// The decay is too short for the band's filter (bandwidth × decay time below 8).
    FilterLimited {
        /// Bandwidth × decay time found.
        bandwidth_decay: f64,
    },
}

/// A room parameter, or why it is not given.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoomValue {
    /// The parameter (unit: the field's).
    Value {
        /// Value.
        value: f64,
    },
    /// Not given.
    Refused {
        /// Why.
        reason: RoomRefusal,
    },
}

impl RoomValue {
    /// The value, if given.
    pub fn value(self) -> Option<f64> {
        match self {
            RoomValue::Value { value } => Some(value),
            RoomValue::Refused { .. } => None,
        }
    }
}

/// One band's room parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomBand {
    /// Mid-band frequency; `None` = broadband (the impulse response as captured).
    pub centre: Option<Hz>,
    /// The band's onset, re the arrival.
    pub onset: Seconds,
    /// Where its decay meets the noise (truncation point), re the arrival.
    pub truncation: Seconds,
    /// Depth of its decay curve at the truncation point; `None` without a decay.
    pub decay_range: Option<Db>,
    /// Early decay time, seconds.
    pub edt: RoomValue,
    /// T20, seconds.
    pub t20: RoomValue,
    /// T30, seconds.
    pub t30: RoomValue,
    /// Clarity C50, dB.
    pub c50: RoomValue,
    /// Clarity C80, dB.
    pub c80: RoomValue,
    /// Definition D50, ratio 0…1.
    pub d50: RoomValue,
    /// Curvature 100·(T30/T20 − 1), percent, when both are given.
    pub curvature: Option<f64>,
}

/// ISO 3382-1 room parameters of a sweep's impulse response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomAcoustics {
    /// The impulse response as captured (band: the sweep's).
    pub broadband: RoomBand,
    /// Octave bands 63 Hz … 8 kHz inside the sweep's range.
    pub octave: Vec<RoomBand>,
    /// One-third-octave bands 50 Hz … 10 kHz inside the sweep's range.
    pub third: Vec<RoomBand>,
    /// End of the impulse response analysed (the end of the silence after the sweep), re
    /// the arrival.
    pub span_end: Seconds,
}

impl RoomAcoustics {
    /// Curvature above which a decay is not straight, percent.
    pub const CURVATURE_LIMIT: f64 = 10.0;
}

// ---------------------------------------------------------------------------------------
// Calibration (decisions 7a–7c)

/// What a calibration is tied to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalKey {
    /// Capture device.
    pub device: DeviceId,
    /// Input channel.
    pub channel: u16,
    /// Mic name.
    pub mic: String,
}

/// A sensitivity calibration: what 0 dBFS on the input is in dB SPL, and how that was
/// found (`docs/design/q7-calibration.md` §2, §11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplCal {
    /// dB SPL = dBFS + sensitivity.
    pub sensitivity: Db,
    /// How it was measured.
    pub method: CalMethod,
    /// Frequency of the tone read.
    pub freq: Hz,
    /// Broadband level read from the tone, uncorrected.
    pub measured: Dbfs,
    /// When (daemon clock).
    pub calibrated_at: WallNs,
}

/// How a sensitivity calibration was measured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CalMethod {
    /// An acoustic calibrator on the mic: the whole chain, capsule included. The mic curve
    /// is normalised to 0 dB at the calibrator frequency.
    Acoustic {
        /// Calibrator level.
        calibrator_level: DbSpl,
    },
    /// A voltage measured at the input while ac2 read its level, with the mic's
    /// sensitivity from the operator or the data sheet. The mic curve is normalised to 0 dB
    /// at 1 kHz, where mic sensitivities are specified.
    Electrical {
        /// Where the voltage was measured.
        connection: ElectricalConnection,
        /// Voltage measured, RMS.
        volts: Volts,
        /// Voltage at 0 dBFS: `volts / 10^(measured / 20)`.
        full_scale: Volts,
        /// Mic sensitivity used.
        mic_sensitivity: MvPerPa,
        /// Where it came from.
        mic_sensitivity_from: SensitivitySource,
        /// Stated uncertainty of the sensitivity, ± dB.
        uncertainty: Db,
    },
}

/// Where the voltage of an electrical calibration was measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElectricalConnection {
    /// Across XLR pins 2–3 with the mic connected and powered and a steady tone at the
    /// mic: the mic's own source impedance loads the preamp as in use.
    InLine,
    /// A generator in place of the mic (phantom power off).
    Injected,
}

/// Where the mic sensitivity of an electrical calibration came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SensitivitySource {
    /// Typed by the operator.
    Typed,
    /// The stated sensitivity in the header of one of the mic's curve files.
    DataSheet {
        /// The curve's label.
        label: String,
        /// Its file name.
        file_name: String,
    },
}

/// What a readout's calibration rests on, as frames carry it: enough to word it
/// (`cal 94 dB`, `electrical cal (in-line, data sheet 15.0 mV/Pa) ±1 dB`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CalBasis {
    /// An acoustic calibrator.
    Acoustic {
        /// Calibrator level.
        calibrator_level: DbSpl,
    },
    /// A measured voltage and the mic's sensitivity.
    Electrical {
        /// Where the voltage was measured.
        connection: ElectricalConnection,
        /// Mic sensitivity used.
        mic_sensitivity: MvPerPa,
        /// It is the data sheet's (else typed).
        data_sheet: bool,
        /// Stated uncertainty, ± dB.
        uncertainty: Db,
    },
}

impl CalMethod {
    /// What frames carry of it.
    pub fn basis(&self) -> CalBasis {
        match self {
            Self::Acoustic { calibrator_level } => CalBasis::Acoustic {
                calibrator_level: *calibrator_level,
            },
            Self::Electrical {
                connection,
                mic_sensitivity,
                mic_sensitivity_from,
                uncertainty,
                ..
            } => CalBasis::Electrical {
                connection: *connection,
                mic_sensitivity: *mic_sensitivity,
                data_sheet: matches!(mic_sensitivity_from, SensitivitySource::DataSheet { .. }),
                uncertainty: *uncertainty,
            },
        }
    }
}

/// Provenance of one imported mic curve (the points stay in the daemon's store).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MicCurveRef {
    /// Short name among the mic's curves, e.g. `0°`, `90°` (unique per mic).
    pub label: String,
    /// File name as imported.
    pub file_name: String,
    /// FNV-1a 64 of the file bytes, 16 lowercase hex digits.
    pub content_hash: String,
    /// Points parsed.
    pub points: u32,
    /// Lowest point frequency.
    pub f_lo: Hz,
    /// Highest point frequency.
    pub f_hi: Hz,
    /// When (daemon clock).
    pub imported_at: WallNs,
    /// Sensitivity the file's header states, mV/Pa. Used only when the operator takes it
    /// for an electrical calibration (`cal.spl_electrical`); an acoustic calibration
    /// measures the whole chain instead.
    pub stated_sensitivity: Option<f64>,
}

/// A mic in the calibration store's mic library: its curves (one per incidence angle, or
/// whatever the operator keeps). Curves follow the mic name across inputs and devices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mic {
    /// Mic name, as on the input setup.
    pub name: String,
    /// Curves in import order; never empty (a mic without curves is not stored).
    pub curves: Vec<MicCurveRef>,
}

/// One curve of the mic library.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MicCurveId {
    /// Mic name.
    pub mic: String,
    /// Curve label.
    pub label: String,
}

/// Sensitivity calibration entry: one per device + input channel + mic name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalEntry {
    /// Key.
    pub key: CalKey,
    /// The calibration.
    pub spl: SplCal,
}

/// Which of its mic's curves an input applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CurveChoice {
    /// Not chosen yet: no curve applies, and the operator is asked to choose. The daemon
    /// chooses the only curve of a mic that has exactly one.
    NotChosen,
    /// Explicitly no curve.
    Off,
    /// The mic's curve with this label.
    Curve {
        /// Label.
        label: String,
    },
}

/// Input setup of one input channel (decision K8): which mic is on it and which of the
/// mic's curves it applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputSetup {
    /// Zero-based device input channel.
    pub channel: u16,
    /// Mic name; `None` = not set.
    pub mic: Option<String>,
    /// The active curve.
    pub curve: CurveChoice,
}

/// Calibration state of a calibrated readout (decisions 7a/7b). The age is the frame's
/// `capture_wall_ns − calibrated_at`, both on the daemon clock.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CalStatus {
    /// No sensitivity calibration applies: dBFS.
    Uncalibrated,
    /// The calibration of this device + input + mic.
    Verified {
        /// When it was taken.
        calibrated_at: WallNs,
        /// What it rests on.
        basis: CalBasis,
    },
    /// A calibration of another mic on this input, or of this mic on another input or
    /// device, is applied.
    OtherMicOrInput {
        /// When it was taken.
        calibrated_at: WallNs,
        /// What it rests on.
        basis: CalBasis,
    },
}

// ---------------------------------------------------------------------------------------
// SPL log, timing

/// The per-second log of an SPL meter and the state of its Leq windows
/// (`docs/design/leq.md`). Changes only when a window's judgement changes, the windows
/// change or the log starts; values arrive in `leq` frames.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplLog {
    /// SPL measurement.
    pub meas: MeasId,
    /// Wall time of the oldest second held; `None` before the first.
    pub started_at: Option<WallNs>,
    /// Each configured window's state, in configuration order.
    pub windows: Vec<LeqWindowState>,
    /// The peak limits' states.
    pub peaks: PeakStates,
    /// Over and recovered events, oldest first (the newest [`SplLog::MAX_ALARMS`]).
    pub alarms: Vec<LeqAlarm>,
}

/// State of a peak limit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeqPeakState {
    /// Current judgement (`no_limit` without a limit).
    pub judgement: LeqJudgement,
    /// When the judgement began.
    pub since: WallNs,
}

/// The states of an SPL meter's peak limits.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeakStates {
    /// LCpeak.
    pub lcpeak: LeqPeakState,
    /// LAFmax.
    pub lafmax: LeqPeakState,
}

impl PeakStates {
    /// The state of `q`.
    pub fn get(&self, q: PeakQuantity) -> LeqPeakState {
        match q {
            PeakQuantity::LcPeak => self.lcpeak,
            PeakQuantity::LafMax => self.lafmax,
        }
    }

    /// The state of `q`, to change.
    pub fn get_mut(&mut self, q: PeakQuantity) -> &mut LeqPeakState {
        match q {
            PeakQuantity::LcPeak => &mut self.lcpeak,
            PeakQuantity::LafMax => &mut self.lafmax,
        }
    }
}

impl SplLog {
    /// Alarms kept.
    pub const MAX_ALARMS: usize = 100;
}

/// State of one Leq window.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeqWindowState {
    /// Window length.
    pub duration: Seconds,
    /// Window weighting.
    pub weighting: Weighting,
    /// Current judgement.
    pub judgement: LeqJudgement,
    /// When the judgement began.
    pub since: WallNs,
}

/// What happened to a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeqAlarmKind {
    /// The window went over its limit.
    Over,
    /// The window came back to or below its limit.
    Recovered,
}

/// What an alarm is about.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AlarmSubject {
    /// A rolling Leq window.
    Window {
        /// Window length.
        duration: Seconds,
        /// Window weighting.
        weighting: Weighting,
    },
    /// A peak limit.
    Peak {
        /// LCpeak or LAFmax.
        quantity: PeakQuantity,
    },
}

impl AlarmSubject {
    /// A window's length; `None` for a peak limit.
    pub fn duration(&self) -> Option<Seconds> {
        match self {
            AlarmSubject::Window { duration, .. } => Some(*duration),
            AlarmSubject::Peak { .. } => None,
        }
    }
}

/// A window or a peak limit going over its limit, or recovering.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeqAlarm {
    /// When (wall time of the end of the second that decided it).
    pub at: WallNs,
    /// The window or the peak limit.
    pub subject: AlarmSubject,
    /// Over or recovered.
    pub kind: LeqAlarmKind,
    /// The level judged then (a window's Leq, a peak limit's highest second within its
    /// hold), with `position` added.
    pub level: DbSpl,
    /// Its limit.
    pub limit: DbSpl,
    /// The measuring-position correction included in `level` (dB), if any.
    pub position: Option<Db>,
}

/// One second of an SPL meter's log.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplLogRow {
    /// Wall time of the second's first sample.
    pub start: WallNs,
    /// Time measured within the second (< 1 s next to a capture gap).
    pub measured: Seconds,
    /// LAeq over the measured time.
    pub laeq: Dbfs,
    /// LCeq over the measured time.
    pub lceq: Dbfs,
    /// LZeq over the measured time.
    pub lzeq: Dbfs,
    /// Highest C-weighted peak of the second.
    pub lcpeak: Dbfs,
    /// Highest A-weighted Fast level of the second.
    pub lafmax: Dbfs,
    /// Sensitivity in force (dB SPL of 0 dBFS); `None` uncalibrated.
    pub sensitivity: Option<Db>,
    /// Measuring-position correction in force: not in the levels (a row is what was
    /// measured); `None` without one or uncalibrated.
    pub position: Option<PositionCorrection>,
}

/// Which of an SPL meter's logs `spl.log_get` reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplLogWhich {
    /// The log the meter is writing.
    Current,
    /// The log `spl.log_new` ended last (kept in memory until the next one).
    Previous,
}

/// Rows of an SPL meter's log (`spl.log_get`). Rows are numbered from the first second the
/// meter logged; the oldest are dropped after [`SplLogPage::RETAINED_ROWS`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplLogPage {
    /// SPL measurement.
    pub meas: MeasId,
    /// Number of the first row returned.
    pub from: u64,
    /// Rows logged so far (one past the newest row's number).
    pub total: u64,
    /// The rows.
    pub rows: Vec<SplLogRow>,
}

impl SplLogPage {
    /// Rows a meter keeps: 48 h.
    pub const RETAINED_ROWS: usize = 48 * 3600;
    /// Most rows one reply carries.
    pub const MAX_ROWS: u32 = 20_000;
}

/// Each Leq window of an SPL meter second by second, as its `leq` frames carried them
/// (`spl.history_get`): what a client that was not connected missed, computed by the daemon
/// from the meter's current log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplHistory {
    /// SPL measurement.
    pub meas: MeasId,
    /// The windows the series are of: the meter's, in configuration order.
    pub windows: Vec<LeqWindow>,
    /// Unit of `leq`: the meter's as of the newest second (the series start after the
    /// last change of unit).
    pub scale: LevelScale,
    /// End of each second, oldest first. A second without a row (nothing measured) has
    /// none, as no frame was sent for it.
    pub at: Vec<WallNs>,
    /// Per window (as `windows`), its Leq at each second of `at` in `scale`; NaN when
    /// nothing was measured in the window.
    pub leq: Vec<Vec<f32>>,
    /// Per window, whether it was over its limit at each second of `at`.
    pub over: Vec<Vec<bool>>,
}

impl SplHistory {
    /// Longest history one reply covers: 4 h.
    pub const MAX_SECONDS: u32 = 4 * 3600;
}

/// Loopback timing monitor state (Q3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TimingState {
    /// No generator output.
    NoStimulus,
    /// Collecting agreeing windows.
    Acquiring,
    /// Offset validated.
    Locked {
        /// Offset.
        offset: Samples,
    },
    /// A jump was confirmed.
    Jumped {
        /// Before.
        from: Samples,
        /// After.
        to: Samples,
    },
    /// Stimulus present, no confident offset.
    Lost,
}

/// Last lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LastLock {
    /// Offset epoch.
    pub epoch: u64,
    /// Offset.
    pub offset: Samples,
    /// Capture index of the locking window.
    pub at_sample: SampleIndex,
    /// Wall time of it (for age display).
    pub at: WallNs,
}

/// Drift between the output and the input clock, from the slope of the loopback offset
/// (`docs/design/multi-device.md`). Kept after the stimulus stops and through offset
/// epochs of the same stream; a new session starts without one.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Drift {
    /// ppm, positive = offset grows (the output clock is slow against the input's).
    pub ppm: f64,
    /// Capture time the regression covers.
    pub span: Seconds,
    /// Above the threshold on a span long enough to judge: output and input are on
    /// different clocks.
    pub warning: bool,
    /// Wall time of the newest window in the estimate (for its age once the stimulus
    /// stopped).
    pub at: WallNs,
}

/// Timing status entity.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimingStatus {
    /// Offset epoch.
    pub epoch: u64,
    /// State.
    pub state: TimingState,
    /// Last lock.
    pub last_lock: Option<LastLock>,
    /// Drift.
    pub drift: Option<Drift>,
    /// Whether an internal reference is available.
    pub internal_reference: bool,
}

/// The whole mirrored state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    /// Session.
    pub session: Session,
    /// Measurements.
    pub measurements: Vec<Measurement>,
    /// Traces.
    pub traces: Vec<TraceMeta>,
    /// Generator.
    pub generator: Generator,
    /// Sensitivity calibrations.
    pub calibrations: Vec<CalEntry>,
    /// Mic library (named curves per mic).
    pub mics: Vec<Mic>,
    /// Input setup (mic names, active curves), sorted by channel.
    pub inputs: Vec<InputSetup>,
    /// SPL logs.
    pub spl_logs: Vec<SplLog>,
    /// Timing.
    pub timing: TimingStatus,
    /// The latest sweep run, if any.
    pub sweep: Option<SweepRun>,
    /// Autosave of the measurements and traces.
    pub autosave: Autosave,
    /// The latest recording, if any.
    pub recording: Option<RecordingRun>,
}

/// Autosave of the measurements and traces: the daemon writes them, in the session file
/// format, to its autosave directory shortly after they change, and restores them when it
/// starts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Autosave {
    /// What the autosave is doing.
    pub state: AutosaveState,
    /// When the autosave on disk was written (after a restore: when the restored one was).
    /// `None` until something has been written.
    pub saved_at: Option<WallNs>,
}

/// State of the autosave.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AutosaveState {
    /// This daemon does not autosave.
    Off,
    /// What is on disk is the current state.
    Saved,
    /// A change waits to be written, or is being written.
    Pending,
    /// The last write failed; the next change, or a retry, writes again.
    Failed {
        /// Why, as the file system said it.
        reason: String,
    },
}

// ---------------------------------------------------------------------------------------
// Raw capture files (PLAN §3.5, `docs/design/raw-capture.md`)

/// What `rec.start` records, and where it stops on its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordRequest {
    /// Device inputs to record (zero-based), each captured by the open session; the file's
    /// channels follow this order.
    pub inputs: Vec<u16>,
    /// File name stem in the daemon's recording directory (letters, digits, ` `, `-`, `_`,
    /// `.`); `None` = `rec-<UTC date and time>`. An existing recording is never overwritten.
    pub name: Option<String>,
    /// The recording ends by itself after this much audio (at most
    /// [`RecordRequest::MAX_DURATION_S`]).
    pub max_duration: Seconds,
    /// … or once the file would grow past this many bytes.
    pub max_bytes: Option<u64>,
}

impl RecordRequest {
    /// Longest recording one `rec.start` may ask for: a day.
    pub const MAX_DURATION_S: f64 = 86_400.0;
}

/// Why a recording ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecordingEnd {
    /// `rec.stop`.
    Stopped,
    /// The requested duration was reached.
    DurationLimit,
    /// The requested size was reached.
    SizeLimit,
    /// Writing failed (disk full, I/O error); everything before the failure is kept.
    WriteFailed {
        /// What the file system said.
        msg: String,
    },
    /// The audio session closed.
    SessionClosed,
    /// The audio session reopened (device or configuration change, `session.open`).
    SessionReopened,
    /// The session's audio stopped (the device stopped delivering or the host ended the
    /// stream); the file ends with the last audio received, and the audio after the outage,
    /// once the daemon reopens the session, is not spliced onto it.
    AudioStopped,
    /// The daemon shut down.
    DaemonShutdown,
    /// Found unfinished when the daemon started (it was killed or crashed while
    /// recording); the file was finalised from what had reached the disk.
    Interrupted,
}

/// What a recording is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecordingStatus {
    /// Writing.
    Recording,
    /// Finalised: the file and its sidecar are complete.
    Ended {
        /// Why.
        reason: RecordingEnd,
    },
}

/// Why the captured audio is not contiguous at a point of a recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscontinuityCause {
    /// The audio host reported an xrun.
    Xrun,
    /// The device's sample index jumped (audio lost before it reached the daemon).
    Gap,
    /// The daemon's capture ring overflowed.
    Overflow,
    /// The device's rate, buffer size or routing changed.
    ConfigChange,
    /// The recorder fell behind (the disk was too slow) and audio was dropped before it
    /// reached the file.
    RecorderBehind,
}

/// The latest recording (`rec.start`), mirrored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingRun {
    /// File name stem.
    pub name: String,
    /// Audio file on the daemon host; the sidecar is beside it (`<name>.ac2rec.json`).
    pub path: String,
    /// Device inputs recorded, in file channel order.
    pub inputs: Vec<u16>,
    /// Sample rate.
    pub sample_rate_hz: u32,
    /// Session epoch it records.
    pub session_epoch: SessionEpoch,
    /// Session sample of the file's first frame.
    pub start_sample: SampleIndex,
    /// When it started.
    pub started_at: WallNs,
    /// Client that started it.
    pub started_by: ClientId,
    /// Frames in the file so far (updated about once a second while recording).
    pub frames: u64,
    /// File size so far, bytes.
    pub bytes: u64,
    /// Discontinuities recorded so far.
    pub discontinuities: u32,
    /// Duration bound.
    pub max_duration: Seconds,
    /// Size bound.
    pub max_bytes: Option<u64>,
    /// Status.
    pub status: RecordingStatus,
}

impl RecordingRun {
    /// Still writing.
    pub fn active(&self) -> bool {
        self.status == RecordingStatus::Recording
    }
}

/// A recording in the daemon's recording directory (rows of `rec.list`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingFile {
    /// File name stem.
    pub name: String,
    /// Audio file on the daemon host.
    pub path: String,
    /// Sample rate.
    pub sample_rate_hz: u32,
    /// Device inputs recorded, in file channel order.
    pub inputs: Vec<u16>,
    /// Frames in the file.
    pub frames: u64,
    /// When it started.
    pub started_at: WallNs,
    /// Discontinuities in it.
    pub discontinuities: u32,
    /// Why it ended; `None` while it is being written.
    pub end: Option<RecordingEnd>,
}

/// Which recording `session.replay` plays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecordingRef {
    /// A name in the daemon's recording directory.
    Name {
        /// Name (the file stem).
        name: String,
    },
    /// The audio file's or the sidecar's path on the daemon host (local transports only).
    Path {
        /// Path.
        path: String,
    },
}

/// How fast a replay runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayPace {
    /// One second of audio per second, like the device that recorded it.
    Realtime,
    /// As fast as the running measurements take it; no audio is ever dropped.
    Fast,
}

/// The recording an open replay session plays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayInfo {
    /// File name stem.
    pub name: String,
    /// Audio file on the daemon host.
    pub path: String,
    /// Frames in the file.
    pub frames: u64,
    /// One past the replay's last sample index: the frames plus the samples the recording
    /// lost in its dropouts (sample indices jump across them, as they did live).
    pub end_sample: SampleIndex,
    /// Pace.
    pub pace: ReplayPace,
    /// Session sample of the file's first frame in the recorded session (replay sample 0).
    pub recorded_start_sample: SampleIndex,
    /// When the recording started.
    pub recorded_at: WallNs,
}
