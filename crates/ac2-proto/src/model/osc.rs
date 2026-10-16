//! OSC messages a cue sends to external gear (lights, a stage display).

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

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
}
