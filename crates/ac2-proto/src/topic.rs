//! Data-socket topics and subscription prefixes.
//!
//! Topic strings: `d/<meas>/<stream>` (measurement streams), `session/levels` and
//! `session/preview` (input meters of the session and of a device preview), `timing`,
//! `evt`, `ka`.
//! `<meas>` is the decimal [`MeasId`] without sign or leading zeros, so every topic has
//! exactly one spelling and ZMQ prefix matching on `d/<meas>/` selects one measurement.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::units::MeasId;

/// Longest valid topic, bytes.
pub const MAX_TOPIC_BYTES: usize = 32;

/// A measurement's published stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    /// Transfer function.
    Tf,
    /// Impulse response view.
    Ir,
    /// Fractional-octave RTA.
    Rta,
    /// Narrowband spectrum.
    Spec,
    /// SPL meter.
    Spl,
    /// Rolling Leq windows of an SPL meter (once a second).
    Leq,
    /// 1/3-octave band Leq of an SPL meter's band meter (once a second).
    BandLeq,
    /// Input meters.
    Levels,
}

impl Stream {
    /// All streams.
    pub const ALL: [Stream; 8] = [
        Self::Tf,
        Self::Ir,
        Self::Rta,
        Self::Spec,
        Self::Spl,
        Self::Leq,
        Self::BandLeq,
        Self::Levels,
    ];

    /// Whether the daemon publishes the stream once a second by design (its values are
    /// per-second windows), rather than at the display rate: a reader judging a frame late
    /// waits longer for these.
    pub fn once_a_second(self) -> bool {
        match self {
            Self::Leq | Self::BandLeq => true,
            Self::Tf | Self::Ir | Self::Rta | Self::Spec | Self::Spl | Self::Levels => false,
        }
    }

    /// Topic segment.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tf => "tf",
            Self::Ir => "ir",
            Self::Rta => "rta",
            Self::Spec => "spec",
            Self::Spl => "spl",
            Self::Leq => "leq",
            Self::BandLeq => "band_leq",
            Self::Levels => "levels",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// A data-socket topic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Topic {
    /// `d/<meas>/<stream>`.
    Data {
        /// Measurement.
        meas: MeasId,
        /// Stream.
        stream: Stream,
    },
    /// `session/levels`: meters of every input of the open session.
    SessionLevels,
    /// `session/preview`: meters of every input of the previewed device.
    PreviewLevels,
    /// `timing`: loopback timing monitor.
    Timing,
    /// `evt`: state events.
    Evt,
    /// `ka`: keepalive.
    Ka,
}

/// A topic that does not parse.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid topic {0:?}")]
pub struct TopicError(pub String);

impl Topic {
    /// Wire bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.to_string().into_bytes()
    }

    /// Parse wire bytes.
    pub fn parse(b: &[u8]) -> Result<Self, TopicError> {
        let err = || TopicError(String::from_utf8_lossy(&b[..b.len().min(MAX_TOPIC_BYTES)]).into());
        if b.len() > MAX_TOPIC_BYTES {
            return Err(err());
        }
        let s = std::str::from_utf8(b).map_err(|_| err())?;
        match s {
            "timing" => return Ok(Self::Timing),
            "session/levels" => return Ok(Self::SessionLevels),
            "session/preview" => return Ok(Self::PreviewLevels),
            "evt" => return Ok(Self::Evt),
            "ka" => return Ok(Self::Ka),
            _ => {}
        }
        let mut it = s.split('/');
        let (Some("d"), Some(id), Some(kind), None) = (it.next(), it.next(), it.next(), it.next())
        else {
            return Err(err());
        };
        let canonical = !id.is_empty()
            && id.bytes().all(|c| c.is_ascii_digit())
            && (id == "0" || !id.starts_with('0'));
        if !canonical {
            return Err(err());
        }
        let meas = id.parse::<u32>().map_err(|_| err())?;
        let stream = Stream::parse(kind).ok_or_else(err)?;
        Ok(Self::Data {
            meas: MeasId(meas),
            stream,
        })
    }
}

impl fmt::Display for Topic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Data { meas, stream } => write!(f, "d/{}/{}", meas.0, stream.as_str()),
            Self::SessionLevels => f.write_str("session/levels"),
            Self::PreviewLevels => f.write_str("session/preview"),
            Self::Timing => f.write_str("timing"),
            Self::Evt => f.write_str("evt"),
            Self::Ka => f.write_str("ka"),
        }
    }
}

/// A ZMQ subscription, as a prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Subscription {
    /// Every measurement stream (`d/`).
    AllData,
    /// Every stream of one measurement (`d/<meas>/`).
    Meas(MeasId),
    /// The session's and the preview's input meters (`session/`).
    InputMeters,
    /// Exactly one topic.
    Topic(Topic),
}

impl Subscription {
    /// Prefix bytes for `ZMQ_SUBSCRIBE`.
    pub fn prefix(&self) -> Vec<u8> {
        match self {
            Self::AllData => b"d/".to_vec(),
            Self::Meas(m) => format!("d/{}/", m.0).into_bytes(),
            Self::InputMeters => b"session/".to_vec(),
            Self::Topic(t) => t.to_bytes(),
        }
    }

    /// Whether a received topic falls under this subscription (what ZMQ's prefix filter
    /// does, but on parsed topics so `ka` never matches a hypothetical `kaX`).
    pub fn matches(&self, topic: &Topic) -> bool {
        match (self, topic) {
            (Self::AllData, Topic::Data { .. }) => true,
            (Self::Meas(m), Topic::Data { meas, .. }) => m == meas,
            (Self::InputMeters, Topic::SessionLevels | Topic::PreviewLevels) => true,
            (Self::Topic(t), other) => t == other,
            _ => false,
        }
    }

    /// The subscriptions a client needs before `state.snapshot` (Q5 step 1).
    pub fn sync_set() -> [Subscription; 2] {
        [Self::Topic(Topic::Evt), Self::Topic(Topic::Ka)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_canonical() {
        for s in Stream::ALL {
            for m in [0, 1, 12, u32::MAX] {
                let t = Topic::Data {
                    meas: MeasId(m),
                    stream: s,
                };
                assert_eq!(Topic::parse(&t.to_bytes()), Ok(t));
                assert!(Subscription::Meas(MeasId(m)).prefix().len() <= MAX_TOPIC_BYTES);
                assert!(
                    t.to_bytes()
                        .starts_with(&Subscription::Meas(MeasId(m)).prefix())
                );
            }
        }
        for t in [
            Topic::Evt,
            Topic::Ka,
            Topic::Timing,
            Topic::SessionLevels,
            Topic::PreviewLevels,
        ] {
            assert_eq!(Topic::parse(&t.to_bytes()), Ok(t));
        }
        for t in [Topic::SessionLevels, Topic::PreviewLevels] {
            assert!(
                t.to_bytes()
                    .starts_with(&Subscription::InputMeters.prefix())
            );
            assert!(Subscription::InputMeters.matches(&t));
            assert!(!Subscription::AllData.matches(&t));
        }
        assert!(Topic::parse(b"session/").is_err());
        assert!(Topic::parse(b"session/levelsx").is_err());
        for bad in [
            &b"d/01/tf"[..],
            b"d/+1/tf",
            b"d//tf",
            b"d/1/tf/",
            b"d/1/xx",
            b"d/4294967296/tf",
            b"kaa",
            b"\xff",
            b"",
        ] {
            assert!(Topic::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn prefixes_do_not_collide() {
        let one = Subscription::Meas(MeasId(1)).prefix();
        let t12 = Topic::Data {
            meas: MeasId(12),
            stream: Stream::Tf,
        };
        assert!(!t12.to_bytes().starts_with(&one));
        assert!(!Subscription::Meas(MeasId(1)).matches(&t12));
        assert!(Subscription::AllData.matches(&t12));
        assert!(!Subscription::AllData.matches(&Topic::Ka));
    }
}
