//! Spike-sized stand-ins for the ac2-proto messages: ctrl request/reply and data frames.

use serde::{Deserialize, Serialize};

pub const PROTO_VERSION: u16 = 1;

/// Upper bounds checked before decoding (PLAN.md §6.3: validate size before decode).
pub const MAX_HEADER_BYTES: usize = 1024;
pub const MAX_ARRAY_LEN: u32 = 1 << 16;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub v: u16,
    pub id: u64,
    pub cmd: Cmd,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Cmd {
    Echo {
        text: String,
    },
    /// Handler blocks until the test opens the gate with this number.
    Slow {
        gate: u32,
    },
    /// Returns the ZAP user id the daemon saw on this request.
    Whoami,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub v: u16,
    pub id: u64,
    pub result: Result<String, ErrorReply>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorReply {
    pub code: ErrorCode,
    pub msg: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    VersionMismatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameKind {
    Tf,
    Rta,
    Spl,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameHeader {
    pub seq: u64,
    pub audio_sample: u64,
    pub daemon_incarnation: u64,
    pub config_rev: u64,
    pub grid_id: u32,
    pub kind: FrameKind,
    pub n: u32,
    /// Capture wall clock (ns since UNIX epoch) so remote clients can show frame age.
    pub capture_wall_ns: u64,
}

/// A decoded data frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub topic: String,
    pub header: FrameHeader,
    pub values: Vec<f32>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    Parts(usize),
    Topic,
    HeaderTooLarge(usize),
    Header(String),
    ArrayTooLong(u32),
    PayloadLen { expected: usize, got: usize },
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "malformed frame: {self:?}")
    }
}

impl std::error::Error for FrameError {}

pub fn encode_request(r: &Request) -> Vec<u8> {
    rmp_serde::to_vec_named(r).unwrap_or_default()
}

pub fn decode_request(b: &[u8]) -> Result<Request, String> {
    rmp_serde::from_slice(b).map_err(|e| e.to_string())
}

pub fn encode_reply(r: &Reply) -> Vec<u8> {
    rmp_serde::to_vec_named(r).unwrap_or_default()
}

pub fn decode_reply(b: &[u8]) -> Result<Reply, String> {
    rmp_serde::from_slice(b).map_err(|e| e.to_string())
}

/// `[topic][msgpack header][f32 LE payload]`.
pub fn encode_frame(topic: &str, header: &FrameHeader, values: &[f32]) -> [Vec<u8>; 3] {
    let head = rmp_serde::to_vec_named(header).unwrap_or_default();
    let mut payload = Vec::with_capacity(values.len() * 4);
    for v in values {
        payload.extend_from_slice(&v.to_le_bytes());
    }
    [topic.as_bytes().to_vec(), head, payload]
}

pub fn decode_frame(parts: &[Vec<u8>]) -> Result<Frame, FrameError> {
    let [topic, head, payload] = parts else {
        return Err(FrameError::Parts(parts.len()));
    };
    let topic = String::from_utf8(topic.clone()).map_err(|_| FrameError::Topic)?;
    if head.len() > MAX_HEADER_BYTES {
        return Err(FrameError::HeaderTooLarge(head.len()));
    }
    let header: FrameHeader =
        rmp_serde::from_slice(head).map_err(|e| FrameError::Header(e.to_string()))?;
    if header.n > MAX_ARRAY_LEN {
        return Err(FrameError::ArrayTooLong(header.n));
    }
    let expected = header.n as usize * 4;
    if payload.len() != expected {
        return Err(FrameError::PayloadLen {
            expected,
            got: payload.len(),
        });
    }
    let values = payload
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    Ok(Frame {
        topic,
        header,
        values,
    })
}
