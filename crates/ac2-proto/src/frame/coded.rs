//! A frame quantised to its wire codes: what a keyframe carries whole and a delta changes.

use crate::topic::Topic;

use super::{ArrayDesc, DecodeError, EncodeError, Frame, FrameKind, FrameMeta, FrameStamp};

/// A frame as codes: the header fields and every array quantised to its element encoding
/// (`i16` hundredths, `u16` ten-thousandths, `bfp16` mantissas, bitmasks), concatenated in
/// `arrays` order, little-endian, a `bfp16` array's exponent byte first.
///
/// The daemon's jobs quantise (the costly half of an encode) and its I/O thread forms
/// keyframes and deltas from these; a receiver applies deltas on the codes it holds and
/// turns them into values only to draw. Codes, not values, are what a delta chain and its
/// CRC are exact on: a value decoded and quantised again would be, too, but would cost a
/// second quantisation per frame.
#[derive(Debug, Clone, PartialEq)]
pub struct CodedFrame {
    /// Topic.
    pub topic: Topic,
    /// Header fields shared by every kind.
    pub stamp: FrameStamp,
    /// Elements per array.
    pub n: u32,
    /// Arrays, in code order.
    pub arrays: Vec<ArrayDesc>,
    /// Per-kind metadata.
    pub meta: FrameMeta,
    /// Codes of every array, `arrays` order.
    pub codes: Vec<u8>,
}

impl CodedFrame {
    /// `frame` quantised; refused where the decoder would refuse it.
    pub fn from_frame(frame: &Frame) -> Result<Self, EncodeError> {
        super::encode::quantise(frame)
    }

    /// The frame's values, checked against its kind's schema.
    pub fn to_frame(&self) -> Result<Frame, DecodeError> {
        super::decode::to_frame(self)
    }

    /// CRC-32 (IEEE, as zlib's) of the codes: what a receiver checks a frame it built from a
    /// chain of deltas against, so an encoder or decoder bug or a wrong base never draws.
    pub fn crc(&self) -> u32 {
        crc32fast::hash(&self.codes)
    }

    /// Kind.
    pub fn kind(&self) -> FrameKind {
        self.meta.kind()
    }
}
