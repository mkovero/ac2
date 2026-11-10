//! Raw capture frames, rig to rig: `[audio/<channel>][msgpack header][f32 LE samples]`.
//!
//! One frame per device block of one channel, never deflated (noise does not compress), so
//! a rig taking another rig's input receives the samples as captured with the stamps it
//! needs to place them.

use serde::{Deserialize, Serialize};

use super::{DecodeError, EncodeError, MAX_FRAME_BYTES, MAX_HEADER_BYTES};
use crate::PROTO_VERSION;
use crate::model::StreamIncarnation;
use crate::topic::Topic;

/// Most frames in one audio frame: a device period is far below this.
pub const MAX_AUDIO_FRAMES: u32 = 1 << 16;

/// Conditions of an audio frame's block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AudioFlags(u8);

impl AudioFlags {
    /// None.
    pub const NONE: Self = Self(0);
    /// The source rig's backend reported an xrun before this block.
    pub const XRUN: Self = Self(1);
    /// The block does not continue the one before on the source rig (lost audio there).
    pub const GAP: Self = Self(2);
    const ALL: u8 = 3;

    /// Whether every bit of `o` is set.
    pub const fn contains(self, o: Self) -> bool {
        self.0 & o.0 == o.0
    }

    /// Raw bits.
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// `self` with `o` set.
    #[must_use]
    pub const fn with(self, o: Self) -> Self {
        Self(self.0 | o.0)
    }
}

/// The header of an audio frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioHeader {
    /// Protocol version.
    pub v: u16,
    /// The source rig's sample index of the first frame.
    pub start_sample: u64,
    /// Frames in the block.
    pub frames: u32,
    /// Its conditions.
    pub flags: AudioFlags,
    /// The source rig's stamp of the first frame's capture, ns on its stamp clock (its
    /// state's stamp map puts it on system time); `None` where its backend stamps no
    /// capture times.
    pub capture_ns: Option<u64>,
    /// The source rig's rate, Hz.
    pub rate: u32,
    /// The run of capture the block belongs to.
    pub incarnation: StreamIncarnation,
}

/// A decoded audio frame.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioFrame {
    /// Device input channel of the source rig.
    pub channel: u16,
    /// Header.
    pub header: AudioHeader,
    /// The samples, `header.frames` of them.
    pub samples: Vec<f32>,
}

/// The parts of an audio frame of `channel`.
pub fn encode_audio(
    channel: u16,
    header: &AudioHeader,
    samples: &[f32],
) -> Result<Vec<Vec<u8>>, EncodeError> {
    let topic = Topic::Audio { channel }.to_bytes();
    let head = rmp_serde::to_vec_named(header).map_err(|e| EncodeError::Msgpack(e.to_string()))?;
    let mut body = Vec::with_capacity(samples.len() * 4);
    for s in samples {
        body.extend_from_slice(&s.to_le_bytes());
    }
    Ok(vec![topic, head, body])
}

/// Decodes the parts of an audio frame.
pub fn decode_audio(parts: &[&[u8]]) -> Result<AudioFrame, DecodeError> {
    let [topic, head, body] = parts else {
        return Err(DecodeError::Parts(parts.len()));
    };
    let total: usize = parts.iter().map(|p| p.len()).sum();
    if total > MAX_FRAME_BYTES {
        return Err(DecodeError::TooLarge(total));
    }
    let Topic::Audio { channel } = Topic::parse(topic)? else {
        return Err(DecodeError::KindMismatch);
    };
    if head.len() > MAX_HEADER_BYTES {
        return Err(DecodeError::HeaderTooLarge(head.len()));
    }
    // The version is read before the rest, so another build's header is told as such.
    match crate::ctrl::peek_envelope(head).map(|e| e.v) {
        Ok(Some(v)) if v == PROTO_VERSION => {}
        Ok(Some(theirs)) => {
            return Err(DecodeError::VersionMismatch {
                ours: PROTO_VERSION,
                theirs,
            });
        }
        Ok(None) | Err(_) => return Err(DecodeError::Header("no protocol version".into())),
    }
    let header: AudioHeader =
        rmp_serde::from_slice(head).map_err(|e| DecodeError::Header(e.to_string()))?;
    if header.frames > MAX_AUDIO_FRAMES {
        return Err(DecodeError::TooManyElements(header.frames));
    }
    if header.flags.0 & !AudioFlags::ALL != 0 {
        return Err(DecodeError::Header("undefined audio flags".into()));
    }
    if body.len() != header.frames as usize * 4 {
        return Err(DecodeError::Payload {
            expected: header.frames as usize * 4,
        });
    }
    let samples = body
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    Ok(AudioFrame {
        channel,
        header,
        samples,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::units::{DaemonIncarnation, SessionEpoch};

    fn header(frames: u32) -> AudioHeader {
        AudioHeader {
            v: PROTO_VERSION,
            start_sample: 1 << 40,
            frames,
            flags: AudioFlags::XRUN,
            capture_ns: Some(123_456_789),
            rate: 96_000,
            incarnation: StreamIncarnation {
                daemon: DaemonIncarnation(7),
                epoch: SessionEpoch(3),
            },
        }
    }

    #[test]
    fn a_frame_roundtrips_bit_exact() {
        let s: Vec<f32> = (0..256).map(|i| (i as f32 * 0.37).sin()).collect();
        let parts = encode_audio(5, &header(256), &s).unwrap();
        assert_eq!(parts[0], b"audio/5");
        let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
        let f = decode_audio(&refs).unwrap();
        assert_eq!(f.channel, 5);
        assert_eq!(f.header, header(256));
        assert_eq!(f.samples, s);
    }

    #[test]
    fn a_short_payload_or_a_curve_topic_is_refused() {
        let parts = encode_audio(1, &header(256), &[0.0; 255]).unwrap();
        let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
        assert!(matches!(
            decode_audio(&refs),
            Err(DecodeError::Payload { .. })
        ));
        let refs: Vec<&[u8]> = vec![b"timing", &parts[1], &parts[2]];
        assert!(decode_audio(&refs).is_err());
    }
}
