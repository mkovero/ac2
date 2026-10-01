//! Client errors.

use ac2_proto::{CtrlError, ErrorCode, ProtoError};
use thiserror::Error;

/// Everything a client call can fail with.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ClientError {
    /// The ZeroMQ layer failed (socket setup, bind, connect).
    #[error("transport: {0}")]
    Zmq(#[from] ac2_zmq::Error),
    /// No reply within the deadline after every retry of the same request id.
    #[error("daemon did not answer `{op}` ({attempts} attempts)")]
    Timeout {
        /// Command name.
        op: &'static str,
        /// Sends of the same request id.
        attempts: u32,
    },
    /// The daemon speaks another protocol version. There is no fallback.
    #[error("protocol version mismatch: daemon speaks {daemon}, this client speaks {client}")]
    VersionMismatch {
        /// Daemon version (0 when the daemon did not say).
        daemon: u16,
        /// This build's version.
        client: u16,
    },
    /// The daemon refused the command with a typed error.
    #[error("{}: {}", code_name(.0.code), .0.msg)]
    Daemon(ProtoError),
    /// A request could not be encoded.
    #[error("encode: {0}")]
    Encode(CtrlError),
    /// The reply body does not belong to the command.
    #[error("unexpected `{got}` reply to `{op}`")]
    UnexpectedReply {
        /// Command name.
        op: &'static str,
        /// Reply body name.
        got: &'static str,
    },
    /// `grid.get` answered with a grid whose id differs from the one asked for.
    #[error("grid.get({asked:016x}) returned grid {got:016x}")]
    GridMismatch {
        /// Requested id.
        asked: u64,
        /// Id of the returned definition.
        got: u64,
    },
    /// The client's I/O thread has stopped.
    #[error("client is closed")]
    Closed,
    /// Key material is missing or malformed.
    #[error("keys: {0}")]
    Keys(String),
    /// A local file could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// File.
        path: std::path::PathBuf,
        /// Cause.
        source: std::io::Error,
    },
    /// An argument was invalid before anything was sent.
    #[error("invalid: {0}")]
    Invalid(String),
}

impl ClientError {
    /// The daemon's error code, when the daemon refused the command.
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            Self::Daemon(e) => Some(e.code),
            Self::VersionMismatch { .. } => Some(ErrorCode::VersionMismatch),
            _ => None,
        }
    }

    pub(crate) fn from_proto(e: ProtoError) -> Self {
        match (&e.code, &e.detail) {
            (ErrorCode::VersionMismatch, Some(ac2_proto::ErrorDetail::Version { daemon, .. })) => {
                Self::VersionMismatch {
                    daemon: *daemon,
                    client: ac2_proto::PROTO_VERSION,
                }
            }
            (ErrorCode::VersionMismatch, _) => Self::VersionMismatch {
                daemon: 0,
                client: ac2_proto::PROTO_VERSION,
            },
            _ => Self::Daemon(e),
        }
    }
}

/// Wire name of an error code (`lease_held`, …).
pub fn code_name(c: ErrorCode) -> &'static str {
    match c {
        ErrorCode::Invalid => "invalid",
        ErrorCode::NotFound => "not_found",
        ErrorCode::Conflict => "conflict",
        ErrorCode::LeaseRequired => "lease_required",
        ErrorCode::LeaseHeld => "lease_held",
        ErrorCode::Refused => "refused",
        ErrorCode::ResyncRequired => "resync_required",
        ErrorCode::Unsupported => "unsupported",
        ErrorCode::Internal => "internal",
        ErrorCode::VersionMismatch => "version_mismatch",
    }
}
