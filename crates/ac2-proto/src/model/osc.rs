//! OSC cues: the messages the rig sends to external gear (lights, a stage display) when an
//! SPL limit's state changes, and the rig entity that says where they go and how sending
//! fares.

use std::fmt;
use std::net::SocketAddr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{AlarmSubject, PeakQuantity, Weighting};
use crate::units::{Hz, MeasId, Rev, Seconds};

/// An OSC address pattern the rig sends to: a leading `/`, then printable ASCII. The
/// characters OSC gives meaning in patterns (`#*,?[]{}`) and the space are refused, so a
/// receiver matches the address literally.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct OscPath(String);

/// Why a string is not an [`OscPath`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OscPathError {
    /// Empty, or not starting with `/`.
    #[error("an OSC address starts with '/'")]
    NoLeadingSlash,
    /// Longer than [`OscPath::MAX_LEN`].
    #[error("an OSC address is at most {} characters", OscPath::MAX_LEN)]
    TooLong,
    /// A character outside printable ASCII, or one OSC reserves for patterns.
    #[error("{0:?} is not allowed in an OSC address")]
    Char(char),
}

impl OscPath {
    /// Longest address accepted, in bytes.
    pub const MAX_LEN: usize = 128;

    /// Validates `s`.
    pub fn new(s: impl Into<String>) -> Result<Self, OscPathError> {
        let s = s.into();
        if !s.starts_with('/') {
            return Err(OscPathError::NoLeadingSlash);
        }
        if s.len() > Self::MAX_LEN {
            return Err(OscPathError::TooLong);
        }
        if let Some(c) = s
            .chars()
            .find(|&c| !c.is_ascii_graphic() || " #*,?[]{}".contains(c))
        {
            return Err(OscPathError::Char(c));
        }
        Ok(Self(s))
    }

    /// The address.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for OscPath {
    type Error = OscPathError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl From<OscPath> for String {
    fn from(p: OscPath) -> Self {
        p.0
    }
}

impl fmt::Display for OscPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One OSC argument.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum OscArg {
    /// `i`: 32-bit integer.
    Int(i32),
    /// `f`: 32-bit float.
    Float(f32),
    /// `s`: string.
    Str(String),
}

/// One OSC message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OscMessage {
    /// Address.
    pub path: OscPath,
    /// Arguments, in order.
    pub args: Vec<OscArg>,
    /// Append the level the state was judged on, dB SPL, as a trailing float.
    pub append_level: bool,
}

/// The message to send in each state of a limit (or of the whole meter); `None`: nothing
/// for that state.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OscCue {
    /// Under the limit, or no limit to judge.
    pub ok: Option<OscMessage>,
    /// Within the warn margin below the limit.
    pub near: Option<OscMessage>,
    /// Over the limit.
    pub over: Option<OscMessage>,
    /// The rig cannot judge: no audio, device lost, or not calibrated.
    pub fault: Option<OscMessage>,
}

impl OscCue {
    /// The message for `state`.
    pub fn get(&self, state: CueState) -> Option<&OscMessage> {
        match state {
            CueState::Ok => self.ok.as_ref(),
            CueState::Near => self.near.as_ref(),
            CueState::Over => self.over.as_ref(),
            CueState::Fault => self.fault.as_ref(),
        }
    }
}

/// State of a cued limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CueState {
    /// Under the limit, or no limit.
    Ok,
    /// Within the warn margin.
    Near,
    /// Over.
    Over,
    /// Cannot judge.
    Fault,
}

/// What a cue is about: one limit of an SPL meter, or the meter as a whole (the worst
/// state of its limits).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CueSubject {
    /// The whole meter ([`super::SplConfig::cue`]).
    Meter,
    /// A rolling Leq window.
    Window {
        /// Window length.
        duration: Seconds,
        /// Window weighting.
        weighting: Weighting,
    },
    /// A peak limit.
    Peak {
        /// LCpeak or LAFmax.
        quantity: PeakQuantity,
    },
    /// A band window of the band meter.
    Band {
        /// Window length.
        duration: Seconds,
        /// Window weighting.
        weighting: Weighting,
        /// Nominal centre.
        nominal: Hz,
    },
    /// The band meter's predicted LAeq window.
    Predicted,
}

impl From<AlarmSubject> for CueSubject {
    fn from(s: AlarmSubject) -> Self {
        match s {
            AlarmSubject::Window {
                duration,
                weighting,
            } => Self::Window {
                duration,
                weighting,
            },
            AlarmSubject::Peak { quantity } => Self::Peak { quantity },
            AlarmSubject::Band {
                duration,
                weighting,
                nominal,
            } => Self::Band {
                duration,
                weighting,
                nominal,
            },
            AlarmSubject::Predicted => Self::Predicted,
        }
    }
}

/// The current state of one cued subject.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RigCue {
    /// Measurement.
    pub meas: MeasId,
    /// Subject.
    pub subject: CueSubject,
    /// State, as last sent.
    pub state: CueState,
}

/// How sending OSC fares.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OscStatus {
    /// Messages sent since the daemon started.
    pub sent: u64,
    /// The latest send error, as the operating system said it; `None` once a send works.
    pub last_error: Option<String>,
}

/// The rig entity: settings that belong to the daemon's machine rather than to a session,
/// and the monitor running on them (never deleted).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rig {
    /// Rev at which these settings were last committed; `osc.target` and
    /// `rig.monitor_start` name it as their `base`.
    pub rev: Rev,
    /// Where cues go (an IP literal and port); `None`: nowhere.
    #[serde(with = "opt_addr")]
    pub osc_target: Option<SocketAddr>,
    /// At start the daemon reopens the last session's device capture-only, so the rig
    /// measurements meter with no client connected.
    pub monitor_at_start: bool,
    /// Sending.
    pub osc: OscStatus,
    /// Every cued subject's state.
    pub cues: Vec<RigCue>,
}

/// `Option<SocketAddr>` as `"ip:port"` text: msgpack has no address type, and the text
/// reads the same in every client language.
pub(crate) mod opt_addr {
    use std::net::SocketAddr;

    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(
        a: &Option<SocketAddr>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        match a {
            Some(a) => s.serialize_some(&a.to_string()),
            None => s.serialize_none(),
        }
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<SocketAddr>, D::Error> {
        Option::<String>::deserialize(d)?
            .map(|s| s.parse().map_err(D::Error::custom))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc_path_accepts_plain_addresses() {
        for s in [
            "/",
            "/light/red",
            "/a_b-c.d/1",
            "/~!\"$%&'()+-./:;<=>@\\^_`|",
        ] {
            assert_eq!(OscPath::new(s).expect("valid").as_str(), s);
        }
    }

    #[test]
    fn osc_path_refuses_patterns_and_non_printables() {
        assert_eq!(OscPath::new(""), Err(OscPathError::NoLeadingSlash));
        assert_eq!(OscPath::new("light"), Err(OscPathError::NoLeadingSlash));
        for c in [
            ' ', '#', '*', ',', '?', '[', ']', '{', '}', '\t', 'ä', '\u{7f}',
        ] {
            assert_eq!(
                OscPath::new(format!("/a{c}b")),
                Err(OscPathError::Char(c)),
                "{c:?}"
            );
        }
        assert_eq!(
            OscPath::new(format!("/{}", "a".repeat(OscPath::MAX_LEN))),
            Err(OscPathError::TooLong)
        );
    }

    #[test]
    fn osc_path_is_validated_when_decoded() {
        let b = rmp_serde::to_vec_named("/bad path").expect("valid");
        assert!(rmp_serde::from_slice::<OscPath>(&b).is_err());
        let b = rmp_serde::to_vec_named("/good").expect("valid");
        assert_eq!(
            rmp_serde::from_slice::<OscPath>(&b)
                .expect("valid")
                .as_str(),
            "/good"
        );
    }

    #[test]
    fn rig_round_trips_with_its_address_as_text() {
        let rig = Rig {
            rev: Rev(7),
            osc_target: Some("192.168.1.20:9000".parse().expect("valid")),
            monitor_at_start: true,
            osc: OscStatus {
                sent: 3,
                last_error: Some("unreachable".into()),
            },
            cues: vec![RigCue {
                meas: MeasId(2),
                subject: CueSubject::Peak {
                    quantity: PeakQuantity::LcPeak,
                },
                state: CueState::Near,
            }],
        };
        let b = rmp_serde::to_vec_named(&rig).expect("valid");
        assert_eq!(rmp_serde::from_slice::<Rig>(&b).expect("valid"), rig);
        let v: rmpv_like::Value = rmp_serde::from_slice(&b).expect("valid");
        assert_eq!(v.osc_target.as_deref(), Some("192.168.1.20:9000"));
    }

    #[test]
    fn cue_round_trips() {
        let m = |p: &str, args| OscMessage {
            path: OscPath::new(p).expect("valid"),
            args,
            append_level: false,
        };
        let cue = OscCue {
            ok: Some(m("/light", vec![OscArg::Int(0)])),
            near: None,
            over: Some(OscMessage {
                append_level: true,
                ..m(
                    "/light",
                    vec![
                        OscArg::Int(1),
                        OscArg::Float(0.5),
                        OscArg::Str("red".into()),
                    ],
                )
            }),
            fault: Some(m("/fault", vec![])),
        };
        let b = rmp_serde::to_vec_named(&cue).expect("valid");
        assert_eq!(rmp_serde::from_slice::<OscCue>(&b).expect("valid"), cue);
        assert_eq!(cue.get(CueState::Near), None);
        assert_eq!(
            cue.get(CueState::Fault).expect("valid").path.as_str(),
            "/fault"
        );
    }

    /// Just the address field, decoded as the text it is on the wire.
    mod rmpv_like {
        #[derive(serde::Deserialize)]
        pub struct Value {
            pub osc_target: Option<String>,
        }
    }
}
