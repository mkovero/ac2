//! Daemon status: loopback timing, the mirrored State root, autosave, and server features.

use serde::{Deserialize, Serialize};

use super::{
    CalEntry, Generator, InputSetup, Measurement, Mic, OutputSetup, RecordingRun, Session, SplLog,
    SweepRun, TraceMeta,
};
use crate::units::{SampleIndex, Samples, Seconds, WallNs};

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
    /// Output labels of the rig, sorted by channel; only labelled outputs are listed.
    pub outputs: Vec<OutputSetup>,
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

/// How the daemon serves clients, and who may connect (`server.info`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerInfo {
    /// Transport and its security.
    pub mode: ServerMode,
    /// Where raw capture files are recorded on the daemon host; `None`: this daemon does
    /// not record.
    pub recording_dir: Option<String>,
}

/// How the daemon listens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ServerMode {
    /// In this process only (a daemon embedded in an app).
    Embedded,
    /// This machine only (`ipc://` or loopback TCP): the operating system's user boundary
    /// is the trust boundary; there are no client keys.
    Local {
        /// Ctrl endpoint.
        ctrl: String,
    },
    /// The network, CURVE on both sockets: only clients whose key is authorized connect.
    Network {
        /// Ctrl endpoint as bound.
        ctrl: String,
        /// Data endpoint as bound.
        data: String,
        /// The server's public key (Z85), which clients pin when pairing.
        server_key: String,
        /// Its fingerprint, as `ac2 discover` and the connect dialog show it.
        fingerprint: String,
        /// The name the rig is advertised under over mDNS; `None`: not advertised.
        advertised_as: Option<String>,
        /// The authorized clients, by name.
        authorized: Vec<AuthorizedClient>,
        /// Keys refused lately, newest first (bounded).
        refused: Vec<RefusedKey>,
    },
}

/// One authorized client key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizedClient {
    /// Its name (the identity its requests carry: lease owner, audit).
    pub name: String,
    /// Public key (Z85).
    pub key: String,
    /// Fingerprint of the key.
    pub fingerprint: String,
}

/// A client key the daemon refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefusedKey {
    /// Public key (Z85); `None` when the peer did not use CURVE.
    pub key: Option<String>,
    /// Fingerprint of the key; `None` with `key`.
    pub fingerprint: Option<String>,
    /// Peer address.
    pub address: String,
    /// Refusals of this key from this address since the daemon started.
    pub count: u64,
    /// The latest refusal.
    pub last_at: WallNs,
}
