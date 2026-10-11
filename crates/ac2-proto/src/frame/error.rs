//! Encode and decode errors.

use thiserror::Error;

use crate::event::EventError;

use super::ArrayName;

/// Why a frame was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DecodeError {
    /// Wrong number of parts.
    #[error("frame has {0} parts")]
    Parts(usize),
    /// Total size above [`MAX_FRAME_BYTES`](super::MAX_FRAME_BYTES).
    #[error("frame of {0} bytes exceeds the limit")]
    TooLarge(usize),
    /// Topic invalid.
    #[error(transparent)]
    Topic(#[from] crate::topic::TopicError),
    /// Header part above [`MAX_HEADER_BYTES`](super::MAX_HEADER_BYTES).
    #[error("header of {0} bytes exceeds the limit")]
    HeaderTooLarge(usize),
    /// Header not valid msgpack of the schema.
    #[error("malformed header: {0}")]
    Header(String),
    /// Another protocol version.
    #[error("frame at protocol version {theirs}, this build speaks {ours}")]
    VersionMismatch {
        /// This build.
        ours: u16,
        /// The frame.
        theirs: u16,
    },
    /// `n` above [`MAX_N`](super::MAX_N).
    #[error("n = {0} exceeds the limit")]
    TooManyElements(u32),
    /// More arrays than [`MAX_ARRAYS`](super::MAX_ARRAYS), or a payload part without arrays or arrays
    /// without one.
    #[error("header lists {listed} arrays, frame has {parts} parts")]
    ArrayCount {
        /// In the header.
        listed: usize,
        /// Parts present.
        parts: usize,
    },
    /// The payload is not deflate data of exactly the size `n` and the arrays' element
    /// encodings make.
    #[error("payload does not inflate to {expected} bytes")]
    Payload {
        /// Bytes the arrays take.
        expected: usize,
    },
    /// An array holds a code its element encoding does not define.
    #[error("undefined code in {0:?}")]
    UndefinedCode(ArrayName),
    /// Topic, `kind` and `meta` disagree.
    #[error("topic, kind and meta disagree")]
    KindMismatch,
    /// Arrays do not match the kind's schema.
    #[error("array schema: {0}")]
    Schema(String),
    /// A bitmask has undefined bits.
    #[error("undefined bits in {0:?}")]
    UnknownBits(ArrayName),
    /// Event body malformed.
    #[error(transparent)]
    Event(#[from] EventError),
}

/// Why a frame could not be encoded (it would be refused by the decoder).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EncodeError {
    /// Arrays of one frame have different lengths.
    #[error("array {name:?} has {got} elements, expected {expected}")]
    Length {
        /// Array.
        name: ArrayName,
        /// Expected (`n`).
        expected: usize,
        /// Got.
        got: usize,
    },
    /// `n` above [`MAX_N`](super::MAX_N).
    #[error("n = {0} exceeds the limit")]
    TooManyElements(usize),
    /// Header above [`MAX_HEADER_BYTES`](super::MAX_HEADER_BYTES).
    #[error("header of {0} bytes exceeds the limit")]
    HeaderTooLarge(usize),
    /// Serialization failed.
    #[error("encode: {0}")]
    Msgpack(String),
    /// Event encoding failed.
    #[error(transparent)]
    Event(#[from] EventError),
}
