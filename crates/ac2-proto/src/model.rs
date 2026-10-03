//! State entities mirrored by clients, and the configuration enums they are made of.
//!
//! These mirror `ac2-core` / `ac2-audio` types on the wire. They are separate types (not
//! re-exports) so the protocol has no dependency on the DSP or audio crates and so a change
//! there cannot silently change the wire; the daemon converts at its boundary.

use serde::{Deserialize, Serialize};

use crate::grid::GridId;
use crate::units::{
    Blob, ClientId, Db, DbSpl, Dbfs, Degrees, Hz, MeasId, Rev, SampleIndex, Samples, Seconds,
    SessionEpoch, SweepId, TraceId, WallNs,
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
    /// Mean of the last `frames` frames.
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
}

/// Session entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    /// Current epoch.
    pub epoch: SessionEpoch,
    /// `None` while closed.
    pub open: Option<OpenSession>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    /// `input` with the given weightings and a C-weighted peak.
    pub fn on_input(input: u16, weighting: Weighting, time_weighting: TimeWeighting) -> Self {
        Self {
            input,
            weighting,
            time_weighting,
            peak_weighting: PeakWeighting::C,
        }
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
    /// Applied delay in samples (exact value used by DSP).
    pub applied_samples: Samples,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MicState {
    /// Mic name.
    pub name: String,
    /// Mic curve applied (by name), `None` when switched off or absent.
    pub curve: Option<String>,
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
    /// common reference delay; 0 for imported traces.
    pub delay: Seconds,
    /// Averaging depth policy at capture (transfer captures).
    pub depth: Option<DepthPolicy>,
    /// Calibration at capture.
    pub cal: CalState,
    /// Mic at capture.
    pub mic: Option<MicState>,
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
/// smoothing (`meta.edit.smoothing`) applied. Column order = grid order.
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
}

impl SweepRequest {
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

/// A sensitivity calibration against an acoustic calibrator.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplCal {
    /// dB SPL = dBFS + sensitivity.
    pub sensitivity: Db,
    /// Calibrator level.
    pub calibrator_level: DbSpl,
    /// Calibrator frequency; the mic curve is normalised to 0 dB here.
    pub calibrator_freq: Hz,
    /// Broadband level read from the calibrator, uncorrected.
    pub measured: Dbfs,
    /// When (daemon clock).
    pub calibrated_at: WallNs,
}

/// Provenance of an imported mic curve (the points stay in the daemon's store).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MicCurveRef {
    /// Display name (the file name without extension).
    pub name: String,
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
}

/// Calibration entry: what is known for one device + input channel + mic name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalEntry {
    /// Key.
    pub key: CalKey,
    /// Sensitivity calibration, if taken.
    pub spl: Option<SplCal>,
    /// Mic curve, if imported.
    pub mic_curve: Option<MicCurveRef>,
}

/// What `cal.mic_curve` does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MicCurveAction {
    /// Import a magnitude file (`.frd`, `.txt`, CSV) sent by the client; the daemon parses
    /// and validates it.
    Import {
        /// Original file name.
        file_name: String,
        /// File content.
        content: Blob,
    },
    /// Remove the curve from the entry.
    Clear,
}

/// What `cal.delete` removes from an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalPart {
    /// The sensitivity calibration (`spl`); a curve on the entry stays.
    Sensitivity,
    /// The mic curve; a sensitivity calibration on the entry stays.
    MicCurve,
    /// The whole entry.
    All,
}

/// Input setup of one input channel (decision K8): which mic is on it and whether its
/// mic curve is applied (decision 7c, on/off per input).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputSetup {
    /// Zero-based device input channel.
    pub channel: u16,
    /// Mic name; `None` = not set.
    pub mic: Option<String>,
    /// Apply the mic's curve (when one resolves).
    pub mic_curve: bool,
}

/// Calibration state of a calibrated readout (decisions 7a/7b). The age is the frame's
/// `capture_wall_ns − calibrated_at`, both on the daemon clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CalStatus {
    /// No sensitivity calibration applies: dBFS.
    Uncalibrated,
    /// The calibration of this device + input + mic.
    Verified {
        /// When it was taken.
        calibrated_at: WallNs,
    },
    /// A calibration of another mic on this input, or of this mic on another input or
    /// device, is applied.
    OtherMicOrInput {
        /// When it was taken.
        calibrated_at: WallNs,
    },
}

// ---------------------------------------------------------------------------------------
// SPL log, timing

/// SPL log state of one SPL measurement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplLog {
    /// Measurement logged.
    pub meas: MeasId,
    /// Logging.
    pub running: bool,
    /// Interval between log rows.
    pub interval: Seconds,
    /// When the log started.
    pub started_at: Option<WallNs>,
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

/// Clock drift estimate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Drift {
    /// ppm, positive = offset grows.
    pub ppm: f64,
    /// Span the estimate covers.
    pub span: Seconds,
    /// Above threshold.
    pub warning: bool,
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
    /// Calibrations.
    pub calibrations: Vec<CalEntry>,
    /// Input setup (mic names, mic-curve switches), sorted by channel.
    pub inputs: Vec<InputSetup>,
    /// SPL logs.
    pub spl_logs: Vec<SplLog>,
    /// Timing.
    pub timing: TimingStatus,
    /// The latest sweep run, if any.
    pub sweep: Option<SweepRun>,
}
