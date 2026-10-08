//! Typed ac2 protocol: commands, replies, state entities, events, data frames, topics and
//! the protocol version. Transport-agnostic: bytes in, bytes out; no socket code here.
//!
//! Normative description: `docs/protocol.md`. Every ctrl and event message is msgpack with
//! named fields; data frames are `[topic][msgpack header][little-endian 4-byte arrays…]`.
#![forbid(unsafe_code)]

pub mod cal;
pub mod ctrl;
pub mod event;
pub mod frame;
pub mod grid;
pub mod model;
pub mod samples;
pub mod topic;
pub mod units;

/// The one protocol version this build speaks. Peers on any other version are refused
/// (`version_mismatch`); there is no negotiation and no fallback.
pub const PROTO_VERSION: u16 = 29;

pub use ctrl::{
    Command, CtrlError, Envelope, ErrorCode, ErrorDetail, ImportProblem, MicCurveFileReason,
    ProtoError, Reply, ReplyBody, Request, Welcome, decode_reply, decode_request, encode_reply,
    encode_request, peek_envelope,
};
pub use event::{Change, Event, EventError, Patch, StateSnapshot, decode_event, encode_event};
pub use frame::{
    DataMessage, DecodeError, EncodeError, Frame, FrameData, FrameHeader, FrameKind, FrameStamp,
    decode_data_message, decode_frame, encode_event_message, encode_frame,
};
pub use grid::{BinColumns, GridDef, GridId};
pub use topic::{Stream, Subscription, Topic};
