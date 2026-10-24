//! Data frames: `[topic][msgpack header][payload]`.
//!
//! The payload holds the arrays the header's `arrays` list describes, in its order, each
//! `n` elements in its element encoding (`crate::packing`), deflated as one part; a frame
//! without arrays has no payload part. Column order = grid order. Invalid values are NaN;
//! where the reason matters a `validity` bitmask array says why.
//!
//! Decoding validates every size before it parses anything: part count, topic length,
//! header length, total length, then (after the bounded header parse) `n`, the array count,
//! and the payload, inflated to exactly the size the header implies and no further. A
//! malformed frame is an error value, never a panic.

/// Largest header part, bytes: room for a band meter's state of every one of its
/// [`crate::model::BandLeqConfig::MAX_WINDOWS`] windows (about 50 bytes each).
pub const MAX_HEADER_BYTES: usize = 8192;
/// Largest `n` (columns / points / channels).
pub const MAX_N: u32 = 1 << 16;
/// Most arrays in one frame.
pub const MAX_ARRAYS: usize = 8;
/// Most parts of one frame: topic, header, payload.
pub const MAX_PARTS: usize = 3;
/// Largest whole frame (all parts), bytes.
pub const MAX_FRAME_BYTES: usize = 2 << 20;

mod data;
mod decode;
mod encode;
mod error;
mod flags;
mod meta;
mod schema;

pub use data::{
    BandLeqFrame, DataMessage, Frame, FrameData, FrameStamp, GONIO_POINTS, GonioFrame, GonioPair,
    IrFrame, LeqFrame, LevelsFrame, PreviewLevelsFrame, RtaFrame, SessionLevelsFrame, SpecFrame,
    SplFrame, TfFrame,
};
pub use decode::{decode_data_message, decode_frame};
pub use encode::{encode_event_message, encode_frame};
pub use error::{DecodeError, EncodeError};
pub use flags::{ClipFlags, LeqFlags, ProtectionFlags, ValidityMask};
pub use meta::{
    AudioHealth, BandLeqMeta, BandWindowState, DaemonHealth, FrameHeader, FrameMeta, GenSummary,
    GonioMeta, GonioPairMeta, IrMeta, KaMeta, LeqMeta, LeqPeak, LeqRun, LevelsMeta, MathState,
    OperandState, OperandStatus, PairLevels, PreviewLevelsMeta, RtaMeta, SessionLevelsMeta,
    SpecBand, SpecBands, SpecMeta, SplMeta, TfMeta, TimingMeta, TimingWindow,
};
pub use schema::{ArrayDesc, ArrayName, Elem, FrameKind, Unit};
