//! Error codes and their typed details.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::SessionOrigin;
use crate::units::{ClientId, Rev, WallNs};

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
    /// `conflict` from `file.new` / `file.load`: the working state has unsaved changes.
    UnsavedChanges {
        /// Its session; `None`: untitled.
        origin: Option<SessionOrigin>,
    },
    /// `conflict` from `file.save` / `file.import`: a session of that name exists.
    SessionExists {
        /// The session.
        name: String,
        /// When it was saved.
        saved_at: WallNs,
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
    /// Not a WAV / RF64 file this build reads (16-, 24-, 32-bit integer or 32-bit float).
    NotWav,
    /// A multi-channel impulse response without a channel chosen, or a channel it does not
    /// have.
    PickChannel,
    /// An impulse response with fewer than 2 samples, every sample zero, or a non-finite one.
    NoImpulse,
    /// Impulse-response options with a text file, or a gate that is not ≥ 0 before and > 0
    /// after the peak.
    BadOptions,
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
