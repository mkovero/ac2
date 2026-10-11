//! Control channel: requests, commands, replies, errors and the version handshake.
//!
//! One msgpack map per ctrl message (named fields). Every ctrl message of every protocol
//! version is a map holding `v` (u16) and `id` (u64); that is the only layout fixed across
//! versions, and it is what [`peek_envelope`] reads before anything else, so a peer on
//! another version gets a typed refusal instead of a decode error.

mod command;
mod error;
mod reply;

pub use command::Command;
pub use error::{ErrorCode, ErrorDetail, ImportProblem, MicCurveFileReason, ProtoError};
pub use reply::{Reply, ReplyBody, Welcome};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::PROTO_VERSION;
use crate::units::{RequestId, Rev};

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
