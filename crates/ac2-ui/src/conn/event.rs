//! What the link reports to the UI: [`ConnEvent`]s, drained once per frame, and the
//! stimulus lease's outcomes.

use super::*;

/// What the link reports.
#[derive(Clone, Debug)]
pub enum ConnEvent {
    Connecting {
        target: String,
    },
    Connected {
        target: String,
        server: String,
        client_id: ClientId,
        /// The daemon's identity and display name, from its hello reply.
        rig_id: RigId,
        name: String,
    },
    Failed {
        target: String,
        error: String,
        down: ac2_scene::chrome::RigDown,
        retry_in: Duration,
    },
    Mirror(Arc<MirrorView>),
    Data(Arc<DataSnapshot>),
    /// A stored trace's data and grid (fetched once per trace, again when its display
    /// smoothing changes).
    Trace(Arc<TraceData>, Arc<GridDef>),
    /// A command finished; `Err` carries the daemon's (or the transport's) message.
    Reply {
        what: String,
        result: Result<(), String>,
    },
    /// A command was refused as made on a configuration (or the rig settings) another
    /// client changed first: the daemon's `conflict`.
    Conflict {
        what: String,
        error: String,
    },
    /// A command succeeded with warnings the operator should hear about (sent before its
    /// `Reply`).
    Warnings(Vec<ac2_proto::Warning>),
    /// A `delay.find` answered; `pick` is what the operator asked to insert.
    DelayFound {
        meas: MeasId,
        pick: DelayPick,
        finding: Box<DelayFinding>,
    },
    /// A capture into a slot finished: per item of the request, in its order, the item's
    /// measurement name and the trace stored or why none was.
    Captured {
        slot: u8,
        results: Vec<(String, Result<Box<TraceMeta>, String>)>,
    },
    Stimulus(StimEvent),
    /// `session.devices` answered (or could not be asked).
    Devices(Result<Vec<BackendInfo>, String>),
    /// `session.preview` of `backend`/`device` answered.
    Preview {
        backend: BackendKind,
        device: DeviceId,
        result: Result<Preview, String>,
    },
    /// `session.detect_loopback` answered (or could not run).
    LoopbackDetected(Result<LoopbackDetection, String>),
    /// The session dialog's session is open (and its mic names set).
    SessionOpened,
    /// A measurement was created (and, unless a reply says otherwise, started).
    MeasCreated(Box<Measurement>),
    /// `walk.new` answered with the walk.
    WalkCreated(Box<ac2_proto::model::WalkState>),
    /// `server.info` (`what: None`), or a `server.*` change named `what`, answered with the
    /// daemon's server info or why not.
    Server {
        what: Option<String>,
        result: Result<Box<ac2_proto::model::ServerInfo>, String>,
    },
    /// The Leq history of SPL meter `meas` as the daemon rebuilt it from the meter's log
    /// ([`Request::LeqBackfill`] number `ask`).
    LeqBackfill {
        meas: MeasId,
        ask: u64,
        result: Result<Box<SplHistory>, String>,
    },
    /// A span of an SPL meter's log at a resolution, for a history strip looking back
    /// ([`Request::LeqSpan`] number `ask`).
    LeqSpan {
        meas: MeasId,
        ask: u64,
        result: Result<Box<SplHistory>, String>,
    },
    /// A dynamics trace's step table (`dynamics.table`), fetched as the trace is listed.
    DynamicsTable(TraceId, Box<ac2_proto::model::DynamicsTable>),
    /// One step's envelopes of one input ([`Request::DynamicsEnvelope`]).
    DynamicsEnvelope {
        key: crate::state::dynamics::EnvelopeKey,
        result: Result<Box<ac2_proto::model::DynamicsEnvelope>, String>,
    },
    /// A span of a band log read back ([`Request::BandLogGet`]).
    BandLog {
        ask: crate::leq_dialog::Ask,
        result: Result<Box<ac2_proto::model::SplBandLog>, String>,
    },
    /// `spl.band_transfer` answered with the meter that took the transfer, or why not.
    BandTransfer(Result<Box<Measurement>, String>),
    /// A [`Request::Sessions`] job ended.
    Sessions(Box<crate::sessions::SessionOutcome>),
    /// A session file moving: `uploading fri.ac2session: 1.2 of 3.4 MB`.
    SessionProgress(String),
    /// `file.list` answered.
    SessionList(Result<Vec<ac2_proto::model::SessionFile>, String>),
}

/// Stimulus lease outcomes.
#[derive(Clone, Debug, PartialEq)]
pub enum StimEvent {
    /// Lease held and the generator armed (not emitting).
    Armed,
    /// A `gen.set` was accepted (fire, level change).
    Set {
        firing: bool,
    },
    /// Output stopped and the lease released (or no lease was held and `gen.stop` was sent).
    Stopped,
    /// The daemon refused a refresh: the lease expired or was taken over.
    Lost(String),
    /// An arm / set / stop failed.
    Failed(String),
    /// `ir.capture` started this run.
    SweepStarted(Box<ac2_proto::model::SweepRun>),
    DynamicsStarted(Box<ac2_proto::model::DynamicsRun>),
}
