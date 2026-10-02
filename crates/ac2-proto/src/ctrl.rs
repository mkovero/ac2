//! Control channel: requests, commands, replies, errors and the version handshake.
//!
//! One msgpack map per ctrl message (named fields). Every ctrl message of every protocol
//! version is a map holding `v` (u16) and `id` (u64); that is the only layout fixed across
//! versions, and it is what [`peek_envelope`] reads before anything else, so a peer on
//! another version gets a typed refusal instead of a decode error.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::PROTO_VERSION;
use crate::event::{Event, StateSnapshot};
use crate::grid::{GridDef, GridId};
use crate::model::{
    AverageMethod, CalEntry, DelayFinding, DelayPick, DelayReference, DeviceInfo, EssSpec,
    ExportFormat, FinderBand, Generator, GeneratorDesired, ImportFormat, ImportRole, Lease, MathOp,
    MeasConfig, Measurement, MicCurve, MicCurveAction, Session, SessionConfig, SessionFile,
    SessionRef, SplLog, TraceData, TraceEdit, TraceMeta,
};
use crate::units::{
    Blob, ClientId, DaemonIncarnation, DbSpl, Hz, LeaseToken, MeasId, RequestId, Rev, Seconds,
    SessionEpoch, TraceId,
};

/// Largest ctrl message accepted, bytes (trace import / export bodies included).
pub const MAX_CTRL_BYTES: usize = 4 << 20;

/// A ctrl request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// Protocol version.
    pub v: u16,
    /// Request id; retries reuse it and get the stored reply.
    pub id: RequestId,
    /// Command.
    pub cmd: Command,
    /// Mutation precondition: refused with `conflict` if state has moved past this rev.
    pub expect_rev: Option<Rev>,
}

impl Request {
    /// A request at this build's version.
    pub fn new(id: RequestId, cmd: Command) -> Self {
        Self {
            v: PROTO_VERSION,
            id,
            cmd,
            expect_rev: None,
        }
    }
}

/// Every command. Wire: `{"op": "<group>.<name>", "args": {…}}`; `args` is absent for
/// commands without arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", content = "args", deny_unknown_fields)]
pub enum Command {
    /// Version handshake; the first request of every connection.
    #[serde(rename = "hello")]
    Hello {
        /// Client software name and version, for logs.
        client: String,
    },

    // -- session ------------------------------------------------------------------------
    /// List devices.
    #[serde(rename = "session.devices")]
    SessionDevices,
    /// Open the audio session (new epoch).
    #[serde(rename = "session.open")]
    SessionOpen {
        /// Configuration.
        config: SessionConfig,
    },
    /// Close the audio session (new epoch).
    #[serde(rename = "session.close")]
    SessionClose,
    /// Current session.
    #[serde(rename = "session.status")]
    SessionStatus,

    // -- gen (Q6 lease) -----------------------------------------------------------------
    /// Acquire the stimulus lease.
    #[serde(rename = "gen.acquire")]
    GenAcquire {
        /// Take over from another holder (stops output and disarms first).
        force: bool,
    },
    /// Set the full desired generator state; refreshes the lease.
    #[serde(rename = "gen.set")]
    GenSet {
        /// Lease.
        lease_token: LeaseToken,
        /// Desired state.
        desired: GeneratorDesired,
    },
    /// Refresh the lease.
    #[serde(rename = "gen.refresh")]
    GenRefresh {
        /// Lease.
        lease_token: LeaseToken,
    },
    /// Stop, disarm and release.
    #[serde(rename = "gen.release")]
    GenRelease {
        /// Lease.
        lease_token: LeaseToken,
    },
    /// Universal stop: fade out and disarm; no lease needed.
    #[serde(rename = "gen.stop")]
    GenStop,

    // -- meas ---------------------------------------------------------------------------
    /// Create a measurement.
    #[serde(rename = "meas.create")]
    MeasCreate {
        /// Configuration.
        config: MeasConfig,
    },
    /// Replace a measurement's configuration.
    #[serde(rename = "meas.update")]
    MeasUpdate {
        /// Measurement.
        meas: MeasId,
        /// New configuration.
        config: MeasConfig,
    },
    /// Delete a measurement.
    #[serde(rename = "meas.delete")]
    MeasDelete {
        /// Measurement.
        meas: MeasId,
    },
    /// Start the job.
    #[serde(rename = "meas.start")]
    MeasStart {
        /// Measurement.
        meas: MeasId,
    },
    /// Stop the job.
    #[serde(rename = "meas.stop")]
    MeasStop {
        /// Measurement.
        meas: MeasId,
    },
    /// Freeze or unfreeze the published result.
    #[serde(rename = "meas.freeze")]
    MeasFreeze {
        /// Measurement.
        meas: MeasId,
        /// Frozen.
        frozen: bool,
    },
    /// Reset averages.
    #[serde(rename = "meas.reset")]
    MeasReset {
        /// Measurement.
        meas: MeasId,
    },

    // -- delay --------------------------------------------------------------------------
    /// Run the delay finder.
    #[serde(rename = "delay.find")]
    DelayFind {
        /// Measurement.
        meas: MeasId,
        /// Analysis band.
        band: FinderBand,
        /// Measurement block length; nil = as much audio as there is, up to the band's
        /// default (decision D2: sub 2, 4 or 8 s, default 4 s).
        observation: Option<Seconds>,
    },
    /// Apply a finder result.
    #[serde(rename = "delay.insert")]
    DelayInsert {
        /// Measurement.
        meas: MeasId,
        /// Which result.
        pick: DelayPick,
    },
    /// Set the delay explicitly.
    #[serde(rename = "delay.set")]
    DelaySet {
        /// Measurement.
        meas: MeasId,
        /// Delay.
        delay: Seconds,
    },
    /// Enable or disable tracking.
    #[serde(rename = "delay.track")]
    DelayTrack {
        /// Measurement.
        meas: MeasId,
        /// Enabled.
        enabled: bool,
    },

    // -- trace --------------------------------------------------------------------------
    /// Capture the live result into a trace.
    #[serde(rename = "trace.capture")]
    TraceCapture {
        /// Source measurement.
        meas: MeasId,
        /// Name.
        name: String,
        /// Slot 1…9 to put the trace in (taken from any trace holding it).
        slot: Option<u8>,
    },
    /// List trace metadata.
    #[serde(rename = "trace.list")]
    TraceList,
    /// Get one trace's data.
    #[serde(rename = "trace.get")]
    TraceGet {
        /// Trace.
        trace: TraceId,
    },
    /// Replace a trace's editable properties.
    #[serde(rename = "trace.update")]
    TraceUpdate {
        /// Trace.
        trace: TraceId,
        /// New properties.
        edit: TraceEdit,
    },
    /// Delete a trace.
    #[serde(rename = "trace.delete")]
    TraceDelete {
        /// Trace.
        trace: TraceId,
    },
    /// Average traces into a new trace.
    #[serde(rename = "trace.average")]
    TraceAverage {
        /// Inputs.
        traces: Vec<TraceId>,
        /// Method.
        method: AverageMethod,
        /// Phase reference.
        reference: DelayReference,
        /// Name of the result.
        name: String,
    },
    /// A − B into a new trace.
    #[serde(rename = "trace.math")]
    TraceMath {
        /// A.
        a: TraceId,
        /// B.
        b: TraceId,
        /// Operation.
        op: MathOp,
        /// Name of the result.
        name: String,
    },
    /// Import a file sent by the client.
    #[serde(rename = "trace.import")]
    TraceImport {
        /// Original file name.
        file_name: String,
        /// Format.
        format: ImportFormat,
        /// Measured trace or target curve.
        role: ImportRole,
        /// File content.
        content: Blob,
    },
    /// Export a trace.
    #[serde(rename = "trace.export")]
    TraceExport {
        /// Trace.
        trace: TraceId,
        /// Format.
        format: ExportFormat,
    },

    // -- cal ----------------------------------------------------------------------------
    /// Calibrate an input against an acoustic calibrator.
    #[serde(rename = "cal.spl")]
    CalSpl {
        /// Input channel.
        input: u16,
        /// Mic name.
        mic: String,
        /// Calibrator level.
        calibrator_level: DbSpl,
        /// Calibrator frequency.
        calibrator_freq: Hz,
    },
    /// Assign, bypass or clear an input's mic curve.
    #[serde(rename = "cal.mic_curve")]
    CalMicCurve {
        /// Input channel.
        input: u16,
        /// Action.
        action: MicCurveAction,
    },
    /// List calibrations.
    #[serde(rename = "cal.list")]
    CalList,

    // -- spl ----------------------------------------------------------------------------
    /// Start logging an SPL measurement.
    #[serde(rename = "spl.log_start")]
    SplLogStart {
        /// SPL measurement.
        meas: MeasId,
        /// Row interval.
        interval: Seconds,
    },
    /// Stop logging.
    #[serde(rename = "spl.log_stop")]
    SplLogStop {
        /// SPL measurement.
        meas: MeasId,
    },

    // -- ir -----------------------------------------------------------------------------
    /// ESS impulse response capture; holds the lease for the whole capture.
    #[serde(rename = "ir.capture")]
    IrCapture {
        /// Lease.
        lease_token: LeaseToken,
        /// Measurement input.
        input: u16,
        /// Sweep.
        sweep: EssSpec,
        /// Name of the resulting trace.
        name: String,
    },

    // -- state, grid, file --------------------------------------------------------------
    /// Full state snapshot.
    #[serde(rename = "state.snapshot")]
    StateSnapshot,
    /// Events after `rev`, or `resync_required`.
    #[serde(rename = "state.since")]
    StateSince {
        /// Last applied rev.
        rev: Rev,
    },
    /// Grid definition.
    #[serde(rename = "grid.get")]
    GridGet {
        /// Grid.
        grid_id: GridId,
    },
    /// Save measurements and traces to a session directory on the daemon host.
    #[serde(rename = "file.save")]
    FileSave {
        /// Where.
        session: SessionRef,
    },
    /// Replace measurements and traces with a saved session; always comes up disarmed with
    /// no generator owner, in a new session epoch.
    #[serde(rename = "file.load")]
    FileLoad {
        /// Which.
        session: SessionRef,
    },
    /// Sessions in the daemon's session directory.
    #[serde(rename = "file.list")]
    FileList,
}

impl Command {
    /// Wire name (`op`).
    pub fn name(&self) -> &'static str {
        match self {
            Self::Hello { .. } => "hello",
            Self::SessionDevices => "session.devices",
            Self::SessionOpen { .. } => "session.open",
            Self::SessionClose => "session.close",
            Self::SessionStatus => "session.status",
            Self::GenAcquire { .. } => "gen.acquire",
            Self::GenSet { .. } => "gen.set",
            Self::GenRefresh { .. } => "gen.refresh",
            Self::GenRelease { .. } => "gen.release",
            Self::GenStop => "gen.stop",
            Self::MeasCreate { .. } => "meas.create",
            Self::MeasUpdate { .. } => "meas.update",
            Self::MeasDelete { .. } => "meas.delete",
            Self::MeasStart { .. } => "meas.start",
            Self::MeasStop { .. } => "meas.stop",
            Self::MeasFreeze { .. } => "meas.freeze",
            Self::MeasReset { .. } => "meas.reset",
            Self::DelayFind { .. } => "delay.find",
            Self::DelayInsert { .. } => "delay.insert",
            Self::DelaySet { .. } => "delay.set",
            Self::DelayTrack { .. } => "delay.track",
            Self::TraceCapture { .. } => "trace.capture",
            Self::TraceList => "trace.list",
            Self::TraceGet { .. } => "trace.get",
            Self::TraceUpdate { .. } => "trace.update",
            Self::TraceDelete { .. } => "trace.delete",
            Self::TraceAverage { .. } => "trace.average",
            Self::TraceMath { .. } => "trace.math",
            Self::TraceImport { .. } => "trace.import",
            Self::TraceExport { .. } => "trace.export",
            Self::CalSpl { .. } => "cal.spl",
            Self::CalMicCurve { .. } => "cal.mic_curve",
            Self::CalList => "cal.list",
            Self::SplLogStart { .. } => "spl.log_start",
            Self::SplLogStop { .. } => "spl.log_stop",
            Self::IrCapture { .. } => "ir.capture",
            Self::StateSnapshot => "state.snapshot",
            Self::StateSince { .. } => "state.since",
            Self::GridGet { .. } => "grid.get",
            Self::FileSave { .. } => "file.save",
            Self::FileLoad { .. } => "file.load",
            Self::FileList => "file.list",
        }
    }

    /// Whether the command commits a state change (and so honours `expect_rev`).
    pub fn is_mutation(&self) -> bool {
        !matches!(
            self,
            Self::Hello { .. }
                | Self::SessionDevices
                | Self::SessionStatus
                | Self::TraceList
                | Self::TraceGet { .. }
                | Self::TraceExport { .. }
                | Self::CalList
                | Self::StateSnapshot
                | Self::StateSince { .. }
                | Self::GridGet { .. }
                | Self::DelayFind { .. }
                | Self::FileSave { .. }
                | Self::FileList
        )
    }

    /// The lease token, for commands that require the stimulus lease.
    pub fn lease_token(&self) -> Option<LeaseToken> {
        match self {
            Self::GenSet { lease_token, .. }
            | Self::GenRefresh { lease_token }
            | Self::GenRelease { lease_token }
            | Self::IrCapture { lease_token, .. } => Some(*lease_token),
            _ => None,
        }
    }
}

/// Reply to `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Welcome {
    /// Daemon software name and version, for logs.
    pub server: String,
    /// The identity the daemon bound to this connection (leases, dedup).
    pub client_id: ClientId,
    /// Incarnation.
    pub daemon_incarnation: DaemonIncarnation,
    /// Session epoch.
    pub session_epoch: SessionEpoch,
    /// Current rev.
    pub rev: Rev,
}

/// Successful reply payloads. Wire: `{"type": "<name>", "value": …}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ReplyBody {
    /// Done; the new rev after the commit (unchanged for non-mutations).
    Ack {
        /// Rev after the command.
        rev: Rev,
    },
    /// `hello`.
    Welcome(Welcome),
    /// `session.devices`.
    Devices(Vec<DeviceInfo>),
    /// `session.open` / `session.status`.
    Session(Session),
    /// `gen.acquire` / `gen.refresh`.
    Lease(Lease),
    /// `gen.set`.
    Generator(Generator),
    /// `meas.*` that return the entity.
    Measurement(Measurement),
    /// `delay.find`.
    DelayFinding(DelayFinding),
    /// `trace.capture/update/average/math/import`, `ir.capture`.
    Trace(TraceMeta),
    /// `trace.list`.
    Traces(Vec<TraceMeta>),
    /// `trace.get`.
    TraceData(TraceData),
    /// `trace.export`.
    Export {
        /// Suggested file name.
        file_name: String,
        /// Content.
        content: Blob,
    },
    /// `cal.spl`.
    Calibration(CalEntry),
    /// `cal.list`.
    Calibrations(Vec<CalEntry>),
    /// `cal.mic_curve`.
    MicCurve(Option<MicCurve>),
    /// `spl.log_*`.
    SplLog(SplLog),
    /// `state.snapshot`.
    Snapshot(Box<StateSnapshot>),
    /// `state.since`.
    Events(Vec<Event>),
    /// `grid.get`.
    Grid(GridDef),
    /// `file.save` / `file.load`.
    SessionFile(SessionFile),
    /// `file.list`.
    Sessions(Vec<SessionFile>),
}

/// Error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Arguments invalid.
    Invalid,
    /// Entity missing.
    NotFound,
    /// `expect_rev` stale.
    Conflict,
    /// Command needs the stimulus lease.
    LeaseRequired,
    /// Another client holds the lease.
    LeaseHeld,
    /// Refused for safety (level ceiling, firing without arming, …).
    Refused,
    /// `state.since` gap expired; take a snapshot.
    ResyncRequired,
    /// Not supported by this daemon / device.
    Unsupported,
    /// Daemon fault.
    Internal,
    /// Protocol version differs.
    VersionMismatch,
}

/// Typed data for some errors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ErrorDetail {
    /// `conflict`: the current rev.
    Conflict {
        /// Current rev.
        rev: Rev,
    },
    /// `lease_held`: who holds it.
    LeaseHeld {
        /// Holder.
        owner: ClientId,
    },
    /// `version_mismatch`.
    Version {
        /// Daemon's version.
        daemon: u16,
        /// Version the client sent.
        client: u16,
    },
    /// `resync_required`: the oldest rev still replayable.
    Resync {
        /// Oldest replayable rev.
        oldest: Rev,
    },
    /// `invalid` from `trace.import`: what is wrong and where.
    Import {
        /// 1-based line of the file, when the problem has one.
        line: Option<u32>,
        /// Problem.
        problem: ImportProblem,
    },
    /// `unsupported` from `file.load`: the session format version is not this build's.
    SessionVersion {
        /// Version in the file.
        found: u32,
        /// The one version this build reads.
        supported: u32,
    },
}

/// Why `trace.import` refused a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportProblem {
    /// Not UTF-8 / Latin-1 text.
    NotText,
    /// No data rows.
    NoData,
    /// A data row with a field that is not a number.
    BadNumber,
    /// A data row with fewer or more columns than the first one.
    ColumnCount,
    /// Fewer than two columns (frequency and magnitude are required).
    TooFewColumns,
    /// Frequencies not strictly ascending.
    NotAscending,
    /// A frequency that is not positive and finite, or a non-finite magnitude.
    OutOfRange,
    /// More rows than an import accepts.
    TooManyRows,
    /// An ac2 CSV header of another format version (or none where `ac2_csv` was asked).
    BadHeader,
    /// Coherence outside 0…1.
    BadCoherence,
}

/// Error reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
#[serde(deny_unknown_fields)]
#[error("{code:?}: {msg}")]
pub struct ProtoError {
    /// Code.
    pub code: ErrorCode,
    /// Human-readable message.
    pub msg: String,
    /// Typed detail.
    pub detail: Option<ErrorDetail>,
}

/// A ctrl reply. `result` is msgpack `{"Ok": ReplyBody}` or `{"Err": ProtoError}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    /// Protocol version.
    pub v: u16,
    /// Id of the request answered.
    pub id: RequestId,
    /// Outcome.
    pub result: Result<ReplyBody, ProtoError>,
}

impl Reply {
    /// A reply at this build's version.
    pub fn new(id: RequestId, result: Result<ReplyBody, ProtoError>) -> Self {
        Self {
            v: PROTO_VERSION,
            id,
            result,
        }
    }

    /// The daemon's answer to a request at another version (or without one).
    pub fn version_refusal(id: RequestId, client: Option<u16>) -> Self {
        let client = client.unwrap_or(0);
        Self::new(
            id,
            Err(ProtoError {
                code: ErrorCode::VersionMismatch,
                msg: format!("daemon speaks protocol {PROTO_VERSION}, client sent {client}"),
                detail: Some(ErrorDetail::Version {
                    daemon: PROTO_VERSION,
                    client,
                }),
            }),
        )
    }
}

/// The version-independent part of every ctrl message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Envelope {
    /// Version; `None` when absent (never defaulted).
    pub v: Option<u16>,
    /// Request id, if present.
    pub id: Option<RequestId>,
}

/// Ctrl decode failures.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CtrlError {
    /// Larger than [`MAX_CTRL_BYTES`].
    #[error("ctrl message of {0} bytes exceeds the limit")]
    TooLarge(usize),
    /// No `v` field.
    #[error("ctrl message without protocol version")]
    MissingVersion,
    /// Another protocol version.
    #[error("protocol version {theirs}, this build speaks {ours}")]
    VersionMismatch {
        /// This build.
        ours: u16,
        /// The peer.
        theirs: u16,
    },
    /// Not valid msgpack of the expected shape.
    #[error("malformed ctrl message: {0}")]
    Malformed(String),
    /// Encoding failed.
    #[error("encode: {0}")]
    Encode(String),
}

/// Read `v` and `id` of any ctrl message without decoding the rest.
pub fn peek_envelope(b: &[u8]) -> Result<Envelope, CtrlError> {
    if b.len() > MAX_CTRL_BYTES {
        return Err(CtrlError::TooLarge(b.len()));
    }
    rmp_serde::from_slice(b).map_err(|e| CtrlError::Malformed(e.to_string()))
}

fn check_version(b: &[u8]) -> Result<(), CtrlError> {
    match peek_envelope(b)?.v {
        None => Err(CtrlError::MissingVersion),
        Some(v) if v != PROTO_VERSION => Err(CtrlError::VersionMismatch {
            ours: PROTO_VERSION,
            theirs: v,
        }),
        Some(_) => Ok(()),
    }
}

fn encode<T: Serialize>(t: &T) -> Result<Vec<u8>, CtrlError> {
    rmp_serde::to_vec_named(t).map_err(|e| CtrlError::Encode(e.to_string()))
}

/// Encode a request.
pub fn encode_request(r: &Request) -> Result<Vec<u8>, CtrlError> {
    encode(r)
}

/// Decode a request; the version is checked before the body.
pub fn decode_request(b: &[u8]) -> Result<Request, CtrlError> {
    check_version(b)?;
    rmp_serde::from_slice(b).map_err(|e| CtrlError::Malformed(e.to_string()))
}

/// Encode a reply.
pub fn encode_reply(r: &Reply) -> Result<Vec<u8>, CtrlError> {
    encode(r)
}

/// Decode a reply; a reply at another version is [`CtrlError::VersionMismatch`].
pub fn decode_reply(b: &[u8]) -> Result<Reply, CtrlError> {
    check_version(b)?;
    rmp_serde::from_slice(b).map_err(|e| CtrlError::Malformed(e.to_string()))
}
