//! State entities mirrored by clients, and the configuration enums they are made of.
//!
//! These mirror `ac2-core` / `ac2-audio` types on the wire. They are separate types (not
//! re-exports) so the protocol has no dependency on the DSP or audio crates and so a change
//! there cannot silently change the wire; the daemon converts at its boundary.

use serde::{Deserialize, Serialize};

use crate::grid::GridId;
use crate::units::{
    ClientId, Db, DbSpl, Dbfs, Hz, MeasId, Rev, SampleIndex, Samples, Seconds, SessionEpoch,
    TraceId, WallNs,
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

/// What smoothing averages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmoothingMode {
    /// Power-average magnitude; phase untouched.
    Power,
    /// Power-average magnitude and average unwrapped phase.
    Complex,
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

/// Which delay-finder result to insert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DelayPick {
    /// First arrival within the threshold of the strongest (decision 1a).
    FirstArrival,
    /// Strongest peak.
    Strongest,
    /// Candidate `index` from the finding's list.
    Candidate {
        /// Index into [`DelayFinding::candidates`].
        index: u8,
    },
}

/// One delay-finder candidate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelayCandidate {
    /// Delay.
    pub delay: Seconds,
    /// Level relative to the strongest peak.
    pub relative: Db,
}

/// Result of `delay.find`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelayFinding {
    /// First arrival.
    pub first_arrival: Seconds,
    /// Strongest peak.
    pub strongest: Seconds,
    /// Near-equal peaks: tracking pauses until the operator resolves (decision 1c).
    pub ambiguous: bool,
    /// Candidates, strongest first.
    pub candidates: Vec<DelayCandidate>,
    /// When it was found.
    pub found_at: WallNs,
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
    /// Client that caused it (`None` for expiry).
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
    /// ac2 CSV.
    Ac2Csv,
    /// Generic `freq mag [phase]` text export.
    FreqMagPhaseText,
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
        /// Epoch it was captured in (shared time reference within it).
        epoch: SessionEpoch,
        /// Capture sample index.
        at_sample: SampleIndex,
    },
    /// Imported from a file; independent time reference (decision 8a).
    Imported {
        /// Original file name.
        file_name: String,
        /// Format.
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
    /// IR capture.
    IrCapture {
        /// Epoch.
        epoch: SessionEpoch,
        /// Sweep used.
        sweep: EssSpec,
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
    /// Mic curve applied (by name), `None` if bypassed or absent.
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
}

/// Trace metadata entity (mandatory metadata of PLAN §3.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceMeta {
    /// Id.
    pub id: TraceId,
    /// Editable properties.
    pub edit: TraceEdit,
    /// Origin.
    pub source: TraceSource,
    /// Grid of the stored data.
    pub grid_id: GridId,
    /// Measured delay at capture.
    pub delay: Seconds,
    /// Smoothing at capture.
    pub smoothing: Option<Smoothing>,
    /// Calibration at capture.
    pub cal: CalState,
    /// Mic at capture.
    pub mic: Option<MicState>,
    /// When captured / created.
    pub created_at: WallNs,
}

/// Stored trace data (reply to `trace.get`). Column order = grid order.
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

/// Calibration entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalEntry {
    /// Key.
    pub key: CalKey,
    /// dB SPL = dBFS + sensitivity.
    pub sensitivity: Db,
    /// Calibrator level.
    pub calibrator_level: DbSpl,
    /// Calibrator frequency.
    pub calibrator_freq: Hz,
    /// Level measured during calibration.
    pub measured: Dbfs,
    /// When.
    pub calibrated_at: WallNs,
}

/// One point of a mic correction curve.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurvePoint {
    /// Frequency.
    pub freq: Hz,
    /// Correction subtracted from displayed magnitude.
    pub gain: Db,
}

/// What `cal.mic_curve` does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MicCurveAction {
    /// Assign a curve.
    Assign {
        /// Curve name.
        name: String,
        /// Provenance (file name, vendor serial…).
        provenance: String,
        /// Points, ascending frequency.
        points: Vec<CurvePoint>,
    },
    /// Turn the assigned curve on or off.
    Bypass {
        /// Bypassed.
        bypassed: bool,
    },
    /// Remove the assignment.
    Clear,
}

/// Mic curve assignment entity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MicCurve {
    /// Input it applies to.
    pub input: u16,
    /// Curve name.
    pub name: String,
    /// Provenance.
    pub provenance: String,
    /// Bypassed.
    pub bypassed: bool,
    /// Point count (the points are in the daemon's store).
    pub points: u32,
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
    /// Mic curves.
    pub mic_curves: Vec<MicCurve>,
    /// SPL logs.
    pub spl_logs: Vec<SplLog>,
    /// Timing.
    pub timing: TimingStatus,
}
