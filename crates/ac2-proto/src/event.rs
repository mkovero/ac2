//! State events and snapshots (Q5).
//!
//! An event carries the full new value of one entity (or its deletion), so applying events
//! is assignment and a client can never mis-merge within an entity. Wire:
//! `{"rev": u64, "kind": "<entity>", "payload": …}`.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::{
    Autosave, CalEntry, CalKey, Generator, InputSetup, Measurement, Mic, RecordingRun, Session,
    SplLog, State, SweepRun, TimingStatus, TraceMeta,
};
use crate::units::{DaemonIncarnation, MeasId, Rev, SessionEpoch, TraceId};

/// Largest event message accepted, bytes.
pub const MAX_EVENT_BYTES: usize = 1 << 20;

/// Full value of a keyed entity, or its deletion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Patch<T, K> {
    /// New full value.
    Set(T),
    /// Deleted; carries the key.
    Deleted(K),
}

/// What changed.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// Session (never deleted; closed is a value).
    Session(Session),
    /// A measurement.
    Measurement(Patch<Measurement, MeasId>),
    /// A trace.
    Trace(Patch<TraceMeta, TraceId>),
    /// Generator, including every audited action.
    Generator(Generator),
    /// A sensitivity calibration.
    Calibration(Patch<CalEntry, CalKey>),
    /// A mic of the mic library, keyed by name.
    Mic(Patch<Mic, String>),
    /// The whole input setup (mic names, active curves), sorted by channel.
    Inputs(Vec<InputSetup>),
    /// An SPL log.
    SplLog(Patch<SplLog, MeasId>),
    /// Timing monitor state.
    Timing(TimingStatus),
    /// The latest sweep run (never deleted; a new run replaces it).
    Sweep(SweepRun),
    /// Autosave status (never deleted).
    Autosave(Autosave),
    /// The latest recording (never deleted; a new recording replaces it).
    Recording(RecordingRun),
}

/// One committed state change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(into = "WireEvent", from = "WireEvent")]
pub struct Event {
    /// Rev of the commit.
    pub rev: Rev,
    /// Change.
    pub change: Change,
}

/// Wire shape of [`Event`]: `kind` tags the entity, `rev` and `payload` sit beside it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WireEvent {
    /// [`Change::Session`].
    Session {
        /// Rev.
        rev: Rev,
        /// Payload.
        payload: Session,
    },
    /// [`Change::Measurement`].
    Measurement {
        /// Rev.
        rev: Rev,
        /// Payload.
        payload: Patch<Measurement, MeasId>,
    },
    /// [`Change::Trace`].
    Trace {
        /// Rev.
        rev: Rev,
        /// Payload.
        payload: Patch<TraceMeta, TraceId>,
    },
    /// [`Change::Generator`].
    Generator {
        /// Rev.
        rev: Rev,
        /// Payload.
        payload: Generator,
    },
    /// [`Change::Calibration`].
    Calibration {
        /// Rev.
        rev: Rev,
        /// Payload.
        payload: Patch<CalEntry, CalKey>,
    },
    /// [`Change::Mic`].
    Mic {
        /// Rev.
        rev: Rev,
        /// Payload.
        payload: Patch<Mic, String>,
    },
    /// [`Change::Inputs`].
    Inputs {
        /// Rev.
        rev: Rev,
        /// Payload.
        payload: Vec<InputSetup>,
    },
    /// [`Change::SplLog`].
    SplLog {
        /// Rev.
        rev: Rev,
        /// Payload.
        payload: Patch<SplLog, MeasId>,
    },
    /// [`Change::Timing`].
    Timing {
        /// Rev.
        rev: Rev,
        /// Payload.
        payload: TimingStatus,
    },
    /// [`Change::Sweep`].
    Sweep {
        /// Rev.
        rev: Rev,
        /// Payload.
        payload: SweepRun,
    },
    /// [`Change::Autosave`].
    Autosave {
        /// Rev.
        rev: Rev,
        /// Payload.
        payload: Autosave,
    },
    /// [`Change::Recording`].
    Recording {
        /// Rev.
        rev: Rev,
        /// Payload.
        payload: RecordingRun,
    },
}

impl From<Event> for WireEvent {
    fn from(e: Event) -> Self {
        let rev = e.rev;
        match e.change {
            Change::Session(payload) => Self::Session { rev, payload },
            Change::Measurement(payload) => Self::Measurement { rev, payload },
            Change::Trace(payload) => Self::Trace { rev, payload },
            Change::Generator(payload) => Self::Generator { rev, payload },
            Change::Calibration(payload) => Self::Calibration { rev, payload },
            Change::Mic(payload) => Self::Mic { rev, payload },
            Change::Inputs(payload) => Self::Inputs { rev, payload },
            Change::SplLog(payload) => Self::SplLog { rev, payload },
            Change::Timing(payload) => Self::Timing { rev, payload },
            Change::Sweep(payload) => Self::Sweep { rev, payload },
            Change::Autosave(payload) => Self::Autosave { rev, payload },
            Change::Recording(payload) => Self::Recording { rev, payload },
        }
    }
}

impl From<WireEvent> for Event {
    fn from(w: WireEvent) -> Self {
        let (rev, change) = match w {
            WireEvent::Session { rev, payload } => (rev, Change::Session(payload)),
            WireEvent::Measurement { rev, payload } => (rev, Change::Measurement(payload)),
            WireEvent::Trace { rev, payload } => (rev, Change::Trace(payload)),
            WireEvent::Generator { rev, payload } => (rev, Change::Generator(payload)),
            WireEvent::Calibration { rev, payload } => (rev, Change::Calibration(payload)),
            WireEvent::Mic { rev, payload } => (rev, Change::Mic(payload)),
            WireEvent::Inputs { rev, payload } => (rev, Change::Inputs(payload)),
            WireEvent::SplLog { rev, payload } => (rev, Change::SplLog(payload)),
            WireEvent::Timing { rev, payload } => (rev, Change::Timing(payload)),
            WireEvent::Sweep { rev, payload } => (rev, Change::Sweep(payload)),
            WireEvent::Autosave { rev, payload } => (rev, Change::Autosave(payload)),
            WireEvent::Recording { rev, payload } => (rev, Change::Recording(payload)),
        };
        Event { rev, change }
    }
}

impl Change {
    /// Wire `kind`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Session(_) => "session",
            Self::Measurement(_) => "measurement",
            Self::Trace(_) => "trace",
            Self::Generator(_) => "generator",
            Self::Calibration(_) => "calibration",
            Self::Mic(_) => "mic",
            Self::Inputs(_) => "inputs",
            Self::SplLog(_) => "spl_log",
            Self::Timing(_) => "timing",
            Self::Sweep(_) => "sweep",
            Self::Autosave(_) => "autosave",
            Self::Recording(_) => "recording",
        }
    }
}

/// Reply to `state.snapshot`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateSnapshot {
    /// State at `rev`.
    pub state: State,
    /// Rev of the snapshot; apply buffered events with rev > this.
    pub rev: Rev,
    /// Incarnation.
    pub daemon_incarnation: DaemonIncarnation,
    /// Epoch.
    pub session_epoch: SessionEpoch,
}

/// Event decode failures.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EventError {
    /// Too large.
    #[error("event of {0} bytes exceeds the limit")]
    TooLarge(usize),
    /// Malformed.
    #[error("malformed event: {0}")]
    Malformed(String),
    /// Encoding failed.
    #[error("encode: {0}")]
    Encode(String),
}

/// Encode an event body (the part after the `evt` topic).
pub fn encode_event(e: &Event) -> Result<Vec<u8>, EventError> {
    rmp_serde::to_vec_named(e).map_err(|x| EventError::Encode(x.to_string()))
}

/// Decode an event body; size is checked first.
pub fn decode_event(b: &[u8]) -> Result<Event, EventError> {
    if b.len() > MAX_EVENT_BYTES {
        return Err(EventError::TooLarge(b.len()));
    }
    rmp_serde::from_slice(b).map_err(|x| EventError::Malformed(x.to_string()))
}
