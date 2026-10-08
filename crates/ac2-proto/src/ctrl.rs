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
    AverageMethod, BackendInfo, BackendKind, CalEntry, CalKey, DelayFinding, DelayPick,
    DelayReference, DeviceId, ElectricalConnection, ExportFormat, FinderBand, Generator,
    GeneratorDesired, ImportFormat, ImportRole, InputSetup, Lease, LoopbackDetection, MeasConfig,
    Measurement, Mic, MicCurveId, OutputSetup, OwnedTraces, Preview, RecordRequest, RecordingFile,
    RecordingRef, RecordingRun, ReplayPace, ServerInfo, Session, SessionConfig, SessionFile,
    SessionRef, SplHistory, SplLogPage, SplLogWhich, SweepRun, TraceData, TraceEdit, TraceMeta,
};
use crate::units::{
    Blob, ClientId, DaemonIncarnation, Db, DbSpl, Dbfs, Hz, LeaseToken, MeasId, MvPerPa, RequestId,
    Rev, Seconds, SessionEpoch, TraceId, Volts,
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
    /// List the daemon's backends and their devices.
    #[serde(rename = "session.devices")]
    SessionDevices,
    /// Meter every input of a device without opening a session on it: capture only, the
    /// outputs are never opened. Naming the same device again keeps the preview open;
    /// another device replaces it. Closed by `session.preview_stop`, `session.open`,
    /// `session.detect_loopback` or when not renewed within `expires_in_ms`.
    #[serde(rename = "session.preview")]
    SessionPreview {
        /// Backend.
        backend: BackendKind,
        /// Device.
        device: DeviceId,
    },
    /// Close the preview.
    #[serde(rename = "session.preview_stop")]
    SessionPreviewStop,
    /// Play a short band-limited noise burst at `level` on `output` of a device and find
    /// the input it returns on. Needs the stimulus lease and an explicit level; refused while
    /// the stimulus is firing.
    #[serde(rename = "session.detect_loopback")]
    SessionDetectLoopback {
        /// Lease.
        lease_token: LeaseToken,
        /// Backend.
        backend: BackendKind,
        /// Device.
        device: DeviceId,
        /// Output to play the burst on (zero-based).
        output: u16,
        /// RMS level of the burst; refused when absent (there is no default level).
        level: Option<Dbfs>,
    },
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
    /// Set the mic name and active mic curve of the listed inputs (others unchanged). A
    /// chosen curve must be stored for the row's mic (unless the row is unchanged).
    #[serde(rename = "session.inputs")]
    SessionInputs {
        /// Rows to upsert, one per channel.
        inputs: Vec<InputSetup>,
    },
    /// Set or clear the labels of the listed outputs (others unchanged). Kept by the daemon
    /// with the rig's settings.
    #[serde(rename = "session.outputs")]
    SessionOutputs {
        /// Rows to upsert, one per channel; a `None` label clears it.
        outputs: Vec<OutputSetup>,
    },

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
    /// Set the system maximum level (`generator.ceiling`). Any client may lower it: a
    /// stimulus armed or playing above the new maximum is stopped and disarmed. Raising it
    /// needs `confirm_raise` and is refused while anything is armed or playing. Never above
    /// `generator.ceiling_bound`. Audited, and kept by the daemon across restarts.
    #[serde(rename = "gen.ceiling")]
    GenCeiling {
        /// New maximum, dBFS RMS.
        ceiling: Dbfs,
        /// The operator confirmed a raise (ignored for a lowering).
        confirm_raise: bool,
    },

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
    /// Delete a measurement, and say what becomes of the stored traces and math channels
    /// it owns. Refused while a math channel that stays computes from it or from a trace
    /// deleted with it, and while a run of it (a sweep measurement) plays.
    #[serde(rename = "meas.delete")]
    MeasDelete {
        /// Measurement.
        meas: MeasId,
        /// Its traces and math channels: kept (moved to the imported group) or deleted.
        traces: OwnedTraces,
    },
    /// Start the job (refused for a sweep measurement: `sweep.run` plays it).
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
    /// Move the applied delay by `by` (either sign, fractions of a sample allowed). The
    /// transfer function keeps its averages where it can, so the curve moves at once.
    #[serde(rename = "delay.nudge")]
    DelayNudge {
        /// Measurement.
        meas: MeasId,
        /// Step.
        by: Seconds,
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
    /// Apply a curve of the mic library to a stored trace, or remove the one applied
    /// (`curve: None`). A display edit: the stored columns stay as measured
    /// ([`TraceMeta::mic_curve`]). Refused for a trace whose columns already carry a curve
    /// (captured with one) and for targets.
    #[serde(rename = "trace.mic_curve")]
    TraceMicCurve {
        /// Trace.
        trace: TraceId,
        /// The curve to apply; `None` removes.
        curve: Option<MicCurveId>,
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
    /// Calibrate an input electrically: the operator measured `volts` (RMS) at the input
    /// while a steady tone of `freq` is on it; the daemon reads the input's level and
    /// stores `20·lg(V_FS / (S · 20 µPa))` with `V_FS = volts / 10^(level/20)` and the mic
    /// sensitivity `S` (`None`: the data-sheet value of `mic`'s curve files, when they
    /// state exactly one).
    #[serde(rename = "cal.spl_electrical")]
    CalSplElectrical {
        /// Input channel.
        input: u16,
        /// Mic name.
        mic: String,
        /// Where the voltage was measured.
        connection: ElectricalConnection,
        /// Voltage measured, RMS.
        volts: Volts,
        /// Frequency of the tone.
        freq: Hz,
        /// Mic sensitivity; `None` = the data sheet's.
        mic_sensitivity: Option<MvPerPa>,
        /// Stated uncertainty, ± dB; `None` = 1 dB.
        uncertainty: Option<Db>,
        /// Replace an acoustic calibration of this input and mic (refused otherwise: a
        /// calibrator reading is the better one).
        replace_acoustic: bool,
    },
    /// Import a mic curve file into the mic library as `mic`'s curve `label` (default:
    /// the angle the file names, `90°`, else its file stem); a curve with that label is
    /// replaced. With `input`, that input's mic name is set to `mic`, and the curve becomes
    /// its active one when it is the mic's only curve.
    #[serde(rename = "cal.curve_import")]
    CalCurveImport {
        /// Mic name.
        mic: String,
        /// Label; `None` = from the file.
        label: Option<String>,
        /// Original file name.
        file_name: String,
        /// File content (`.frd`, `.txt`, CSV); the daemon parses and validates it.
        content: Blob,
        /// Input whose mic name is set.
        input: Option<u16>,
    },
    /// Rename a curve; inputs that chose it follow.
    #[serde(rename = "cal.curve_rename")]
    CalCurveRename {
        /// The curve.
        curve: MicCurveId,
        /// New label.
        label: String,
    },
    /// Delete a curve. Inputs that chose it keep the label and say it is not stored.
    #[serde(rename = "cal.curve_delete")]
    CalCurveDelete {
        /// The curve.
        curve: MicCurveId,
    },
    /// List sensitivity calibrations and the mic library.
    #[serde(rename = "cal.list")]
    CalList,
    /// Delete a sensitivity calibration, on any device (no open session needed).
    #[serde(rename = "cal.delete")]
    CalDelete {
        /// Entry.
        key: CalKey,
    },

    // -- spl ----------------------------------------------------------------------------
    /// Rows of an SPL meter's per-second log, from row number `from` (rows already dropped
    /// are skipped: the reply says where it starts), at most `max` (capped at
    /// [`crate::model::SplLogPage::MAX_ROWS`]).
    #[serde(rename = "spl.log_get")]
    SplLogGet {
        /// SPL measurement.
        meas: MeasId,
        /// Which log: the current one, or the one `spl.log_new` ended last.
        log: SplLogWhich,
        /// First row wanted.
        from: u64,
        /// Most rows wanted.
        max: u32,
    },
    /// Each Leq window of an SPL meter second by second over the newest `seconds` (at most
    /// [`crate::model::SplHistory::MAX_SECONDS`]) of its current log, as the meter's job
    /// computed them and its `leq` frames carried them.
    #[serde(rename = "spl.history_get")]
    SplHistoryGet {
        /// SPL measurement.
        meas: MeasId,
        /// Seconds of history wanted.
        seconds: u32,
    },
    /// Ends an SPL meter's log and starts a new one: the windows, their states, the alarms,
    /// the run clock and the total start over; the windows and limits are kept. The ended
    /// log stays readable (`spl.log_get` with `log: previous`) until the next `spl.log_new`
    /// of the meter, the meter's deletion or a daemon restart.
    #[serde(rename = "spl.log_new")]
    SplLogNew {
        /// SPL measurement.
        meas: MeasId,
    },
    /// Computes a transfer per band from the meter's mic to the place the band limits are
    /// for (`docs/design/band-leq.md`, *The transfer*) from the band levels of a steady test
    /// signal at FOH and at the place (`at_place`), and the place's background with the
    /// system silent, and stores it in the band
    /// meter of `meas` (which must have one); the reply is the updated measurement. The
    /// levels come from band logs over a span or as typed values.
    #[serde(rename = "spl.band_transfer")]
    SplBandTransfer {
        /// SPL meter whose band meter gets the transfer.
        meas: MeasId,
        /// Band levels at FOH (the meter's own mic position).
        foh: crate::model::BandLevelSource,
        /// Band levels at the place the limits are for, the same signal.
        at_place: crate::model::BandLevelSource,
        /// The place's background, the system silent; `None`: every band unchecked.
        background: Option<crate::model::BandLevelSource>,
        /// The operator's name of the place ([`crate::model::BandTransferSet::place`]).
        place: String,
    },

    /// A span of an SPL meter's band log (its current log): the energy average over
    /// `[from, until)` as `spl.band_transfer` takes it from a log source, and with `step`
    /// every `step`-th logged second (at most [`crate::model::SplBandLog::MAX_ROWS`] rows,
    /// else refused).
    #[serde(rename = "spl.band_log_get")]
    SplBandLogGet {
        /// SPL meter.
        meas: MeasId,
        /// Start (inclusive).
        from: crate::units::WallNs,
        /// End (exclusive).
        until: crate::units::WallNs,
        /// Rows wanted: every `step`-th second (1: each); `None`: the average alone.
        step: Option<u32>,
    },

    // -- sweep --------------------------------------------------------------------------
    /// Run a sweep measurement with its settings: plays its `repeats` synchronised sweeps,
    /// records the reference and measurement inputs and stores a `sweep` trace it owns
    /// (response, harmonic distortion, IR, room parameters). Like firing, it needs the
    /// stimulus lease and the generator armed; the reply is the started run, whose progress
    /// and outcome follow as `sweep` events.
    #[serde(rename = "sweep.run")]
    SweepRun {
        /// Lease.
        lease_token: LeaseToken,
        /// The sweep measurement.
        meas: MeasId,
        /// Name of the resulting trace; `None` = `Run <number>`.
        name: Option<String>,
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

    // -- rec ----------------------------------------------------------------------------
    /// Record inputs of the open session to a raw capture file (f32 WAV / RF64 and a JSON
    /// sidecar) in the daemon's recording directory. One recording at a time; the reply is
    /// the started run, whose progress and end follow as `recording` events.
    #[serde(rename = "rec.start")]
    RecStart {
        /// What to record and its bounds.
        request: RecordRequest,
    },
    /// End the recording and finalise its file.
    #[serde(rename = "rec.stop")]
    RecStop,
    /// Recordings in the daemon's recording directory.
    #[serde(rename = "rec.list")]
    RecList,
    /// How the daemon serves clients: transport, server key, mDNS name, the authorized
    /// client keys and the keys refused lately.
    #[serde(rename = "server.info")]
    ServerInfo,
    /// Authorize a client key under `name` (network mode). Applies to connections from now
    /// on; written to the authorized-clients file.
    #[serde(rename = "server.authorize")]
    ServerAuthorize {
        /// Name the client's requests will carry (1 … 64 characters, no spaces).
        name: String,
        /// The client's public key (Z85, 40 characters).
        key: String,
    },
    /// Revoke the client key named `name` (network mode): its requests are refused at once,
    /// and it cannot connect again. A client cannot revoke its own key.
    #[serde(rename = "server.revoke")]
    ServerRevoke {
        /// The authorized name.
        name: String,
    },
    /// Open a session that plays a recording instead of a device (new epoch): its inputs
    /// are the recorded ones under their device numbers, it has no outputs, and the running
    /// measurements analyse it as they would the device. A replay never reopens itself: it
    /// stops after the last frame.
    #[serde(rename = "session.replay")]
    SessionReplay {
        /// Which recording.
        recording: RecordingRef,
        /// How fast.
        pace: ReplayPace,
    },
}

impl Command {
    /// Wire name (`op`).
    pub fn name(&self) -> &'static str {
        match self {
            Self::Hello { .. } => "hello",
            Self::SessionDevices => "session.devices",
            Self::SessionPreview { .. } => "session.preview",
            Self::SessionPreviewStop => "session.preview_stop",
            Self::SessionDetectLoopback { .. } => "session.detect_loopback",
            Self::SessionOpen { .. } => "session.open",
            Self::SessionClose => "session.close",
            Self::SessionStatus => "session.status",
            Self::SessionInputs { .. } => "session.inputs",
            Self::SessionOutputs { .. } => "session.outputs",
            Self::GenAcquire { .. } => "gen.acquire",
            Self::GenSet { .. } => "gen.set",
            Self::GenRefresh { .. } => "gen.refresh",
            Self::GenRelease { .. } => "gen.release",
            Self::GenStop => "gen.stop",
            Self::GenCeiling { .. } => "gen.ceiling",
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
            Self::DelayNudge { .. } => "delay.nudge",
            Self::DelayTrack { .. } => "delay.track",
            Self::TraceCapture { .. } => "trace.capture",
            Self::TraceList => "trace.list",
            Self::TraceGet { .. } => "trace.get",
            Self::TraceUpdate { .. } => "trace.update",
            Self::TraceDelete { .. } => "trace.delete",
            Self::TraceAverage { .. } => "trace.average",
            Self::TraceImport { .. } => "trace.import",
            Self::TraceMicCurve { .. } => "trace.mic_curve",
            Self::TraceExport { .. } => "trace.export",
            Self::CalSpl { .. } => "cal.spl",
            Self::CalSplElectrical { .. } => "cal.spl_electrical",
            Self::CalCurveImport { .. } => "cal.curve_import",
            Self::CalCurveRename { .. } => "cal.curve_rename",
            Self::CalCurveDelete { .. } => "cal.curve_delete",
            Self::CalList => "cal.list",
            Self::CalDelete { .. } => "cal.delete",
            Self::SplLogGet { .. } => "spl.log_get",
            Self::SplLogNew { .. } => "spl.log_new",
            Self::SplBandTransfer { .. } => "spl.band_transfer",
            Self::SplBandLogGet { .. } => "spl.band_log_get",
            Self::SplHistoryGet { .. } => "spl.history_get",
            Self::SweepRun { .. } => "sweep.run",
            Self::StateSnapshot => "state.snapshot",
            Self::StateSince { .. } => "state.since",
            Self::GridGet { .. } => "grid.get",
            Self::FileSave { .. } => "file.save",
            Self::FileLoad { .. } => "file.load",
            Self::FileList => "file.list",
            Self::RecStart { .. } => "rec.start",
            Self::RecStop => "rec.stop",
            Self::RecList => "rec.list",
            Self::SessionReplay { .. } => "session.replay",
            Self::ServerInfo => "server.info",
            Self::ServerAuthorize { .. } => "server.authorize",
            Self::ServerRevoke { .. } => "server.revoke",
        }
    }

    /// Whether the command commits a state change (and so honours `expect_rev`).
    pub fn is_mutation(&self) -> bool {
        !matches!(
            self,
            Self::Hello { .. }
                | Self::SessionDevices
                | Self::SessionPreview { .. }
                | Self::SessionPreviewStop
                | Self::SessionDetectLoopback { .. }
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
                | Self::SplLogGet { .. }
                | Self::SplHistoryGet { .. }
                | Self::SplBandLogGet { .. }
                | Self::FileList
                | Self::RecList
                | Self::ServerInfo
        )
    }

    /// The lease token, for commands that require the stimulus lease.
    pub fn lease_token(&self) -> Option<LeaseToken> {
        match self {
            Self::GenSet { lease_token, .. }
            | Self::GenRefresh { lease_token }
            | Self::GenRelease { lease_token }
            | Self::SessionDetectLoopback { lease_token, .. }
            | Self::SweepRun { lease_token, .. } => Some(*lease_token),
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
    /// `session.devices`: every backend the daemon offers, the default one first.
    Backends(Vec<BackendInfo>),
    /// `session.preview`.
    Preview(Preview),
    /// `session.detect_loopback`.
    LoopbackDetection(LoopbackDetection),
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
    /// `trace.capture/update/average/import`.
    Trace(TraceMeta),
    /// `sweep.run`: the run, as started.
    Sweep(SweepRun),
    /// `trace.list`.
    Traces(Vec<TraceMeta>),
    /// `trace.get`.
    TraceData(Box<TraceData>),
    /// `trace.export`.
    Export {
        /// Suggested file name.
        file_name: String,
        /// Content.
        content: Blob,
    },
    /// `cal.spl`, `cal.spl_electrical`.
    Calibration(CalEntry),
    /// `cal.curve_import`, `cal.curve_rename`: the mic with its curves.
    Mic(Mic),
    /// `cal.list`.
    Calibrations {
        /// Sensitivity calibrations.
        calibrations: Vec<CalEntry>,
        /// Mic library.
        mics: Vec<Mic>,
    },
    /// `session.inputs`: the whole input setup.
    Inputs(Vec<InputSetup>),
    /// `session.outputs`: every output label.
    Outputs(Vec<OutputSetup>),
    /// `server.info`, `server.authorize`, `server.revoke`.
    Server(ServerInfo),
    /// `spl.log_get`.
    SplLogPage(SplLogPage),
    /// `spl.history_get`.
    SplHistory(Box<SplHistory>),
    /// `spl.band_log_get`.
    SplBandLog(Box<crate::model::SplBandLog>),
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
    /// `rec.start` / `rec.stop`: the run.
    Recording(RecordingRun),
    /// `rec.list`.
    Recordings(Vec<RecordingFile>),
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
    /// `invalid` from `cal.curve_import`: why the file was refused.
    MicCurveFile {
        /// 1-based line, where one applies.
        line: Option<u32>,
        /// Reason.
        reason: MicCurveFileReason,
    },
    /// `refused`: the calibration store file cannot be read, so it is never written.
    CalStore {
        /// File on the daemon host.
        path: String,
        /// What is wrong with it.
        reason: String,
    },
}

/// Why a mic-curve file was refused (`docs/design/q7-calibration.md` §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MicCurveFileReason {
    /// Fewer than two data lines.
    TooFewPoints,
    /// More than 10 000 data lines.
    TooManyPoints,
    /// The gain field is not a number.
    BadNumber,
    /// A frequency without a gain.
    MissingGain,
    /// Frequency ≤ 0.
    NonPositiveFrequency,
    /// NaN or infinite value.
    NonFinite,
    /// |gain| > 40 dB.
    GainOutOfRange,
    /// Frequencies not strictly ascending.
    NotAscending,
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
