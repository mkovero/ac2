//! Data frames: `[topic][msgpack header ++ deflated payload]`, as a keyframe or a delta
//! (`wire`).
//!
//! The payload holds the arrays the header's `arrays` list describes, in its order, each
//! `n` elements in its element encoding (`crate::packing`), deflated; a frame without arrays
//! has no payload bytes. Column order = grid order. Invalid values are NaN; where the reason
//! matters a `validity` bitmask array says why.

/// Largest header part, bytes: room for a band meter's state of every one of its
/// [`crate::model::BandLeqConfig::MAX_WINDOWS`] windows (about 50 bytes each).
pub const MAX_HEADER_BYTES: usize = 8192;
/// Largest `n` (columns / points / channels).
pub const MAX_N: u32 = 1 << 16;
/// Most arrays in one frame.
pub const MAX_ARRAYS: usize = 12;
/// Parts of one frame: topic, body (header and payload).
pub const MAX_PARTS: usize = 2;
/// Largest whole frame (all parts), bytes.
pub const MAX_FRAME_BYTES: usize = 2 << 20;

mod audio;
mod coded;
mod data;
mod decode;
mod encode;
mod error;
mod flags;
mod meta;
mod schema;
mod wire;

pub use audio::{
    AudioFlags, AudioFrame, AudioHeader, MAX_AUDIO_FRAMES, decode_audio, encode_audio,
};
pub use coded::CodedFrame;
pub use data::{
    BandLeqFrame, DataMessage, Frame, FrameData, FrameStamp, GONIO_POINTS, GonioFrame, GonioPair,
    IrFrame, LeqFrame, LevelsFrame, METER_FPS, PreviewLevelsFrame, RtaFrame, SessionLevelsFrame,
    SpecFrame, SplFrame, TfFrame,
};
pub use decode::decode_data_message;
pub use encode::{encode_event_message, encode_frame};
pub use error::{DecodeError, EncodeError};
pub use flags::{ClipFlags, LeqFlags, ProtectionFlags, ValidityMask};
pub use meta::{
    AudioHealth, BandLeqMeta, BandWindowState, DaemonHealth, FrameHeader, FrameMeta, GenSummary,
    GonioMeta, GonioPairMeta, IrMeta, KaMeta, LeqMeta, LeqPeak, LeqRun, LevelsMeta, LoudnessSource,
    MathState, OperandState, OperandStatus, PairLevels, PreviewLevelsMeta, RtaMeta,
    SessionLevelsMeta, SourceLoudness, SpecBand, SpecBands, SpecMeta, SplMeta, TfMeta, TimingMeta,
    TimingWindow,
};
pub use schema::{ArrayDesc, ArrayName, Elem, FrameKind, Unit};
pub use wire::{DeltaFrame, Form, WireFrame, decode_frame, decode_wire, encode_delta, encode_key};
