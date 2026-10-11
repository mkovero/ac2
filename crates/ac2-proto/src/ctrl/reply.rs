//! Successful replies and the reply envelope.

use serde::{Deserialize, Serialize};

use super::{ErrorCode, ErrorDetail, ProtoError};
use crate::PROTO_VERSION;
use crate::event::{Event, StateSnapshot};
use crate::grid::GridDef;
use crate::model::{
    BackendInfo, CalEntry, DelayFinding, Generator, InputSetup, Lease, LoopbackDetection,
    Measurement, Mic, OutputSetup, Preview, RecordingFile, RecordingRun, ServerInfo, Session,
    SessionFile, SplHistory, SplLogPage, SweepRun, TraceData, TraceMeta,
};
use crate::units::{Blob, ClientId, DaemonIncarnation, RequestId, Rev, SessionEpoch};

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
    /// `file.save` / `file.load` / `file.import`.
    SessionFile(SessionFile),
    /// `file.upload_begin` / `file.download_begin`.
    Transfer(crate::model::Transfer),
    /// `file.download_chunk`.
    Chunk(Blob),
    /// `file.salvage`.
    Salvage(crate::model::Salvage),
    /// `file.list`.
    Sessions(Vec<SessionFile>),
    /// `rec.start` / `rec.stop`: the run.
    Recording(RecordingRun),
    /// `rec.list`.
    Recordings(Vec<RecordingFile>),
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
