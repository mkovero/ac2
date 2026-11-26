//! The rig's sACN (ANSI E1.31) output: the same DMX universe the Art-Net output carries,
//! sent as E1.31 data packets to one receiver or to the universe's multicast group.

use std::fmt;
use std::net::Ipv4Addr;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The UDP port sACN receivers listen on (E1.31 Appendix A, `ACN_SDT_MULTICAST_PORT`).
pub const SACN_PORT: u16 = 5568;

/// An sACN universe, 1–63999 (E1.31 §6.2.7: 0 and 64000–65535 are reserved).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct SacnUniverse(u16);

/// Why a number or text is not an [`SacnUniverse`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SacnUniverseError {
    /// Outside 1–63999.
    #[error("sACN universe {0} is outside 1–63999")]
    Range(u32),
    /// Not a number.
    #[error("{0:?}: an sACN universe is a number 1–63999")]
    Shape(String),
}

impl SacnUniverse {
    /// Highest universe.
    pub const MAX: u16 = 63_999;

    /// Validates `n` (1–63999).
    pub fn new(n: u16) -> Result<Self, SacnUniverseError> {
        if (1..=Self::MAX).contains(&n) {
            Ok(Self(n))
        } else {
            Err(SacnUniverseError::Range(u32::from(n)))
        }
    }

    /// The number, 1–63999.
    pub fn get(self) -> u16 {
        self.0
    }

    /// The universe's multicast group, 239.255.<high byte>.<low byte> (E1.31 §9.3.1).
    pub fn multicast_group(self) -> Ipv4Addr {
        let [hi, lo] = self.0.to_be_bytes();
        Ipv4Addr::new(239, 255, hi, lo)
    }
}

impl TryFrom<u16> for SacnUniverse {
    type Error = SacnUniverseError;
    fn try_from(n: u16) -> Result<Self, Self::Error> {
        Self::new(n)
    }
}

impl From<SacnUniverse> for u16 {
    fn from(u: SacnUniverse) -> Self {
        u.0
    }
}

impl FromStr for SacnUniverse {
    type Err = SacnUniverseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let n: u32 = s
            .trim()
            .parse()
            .map_err(|_| SacnUniverseError::Shape(s.into()))?;
        u16::try_from(n)
            .map_err(|_| SacnUniverseError::Range(n))
            .and_then(Self::new)
    }
}

impl fmt::Display for SacnUniverse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// An sACN source's priority, 0–200 (E1.31 §6.2.3): a receiver takes the universe from the
/// highest-priority source it hears, and merges sources of equal priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct SacnPriority(u8);

/// Why a number or text is not an [`SacnPriority`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SacnPriorityError {
    /// Above 200.
    #[error("sACN priority {0} is outside 0–200")]
    Range(u32),
    /// Not a number.
    #[error("{0:?}: an sACN priority is a number 0–200")]
    Shape(String),
}

impl SacnPriority {
    /// Highest priority.
    pub const MAX: u8 = 200;
    /// The priority E1.31 sources send unless told otherwise (§6.2.3).
    pub const DEFAULT: Self = Self(100);

    /// Validates `n` (0–200).
    pub fn new(n: u8) -> Result<Self, SacnPriorityError> {
        if n <= Self::MAX {
            Ok(Self(n))
        } else {
            Err(SacnPriorityError::Range(u32::from(n)))
        }
    }

    /// The number, 0–200.
    pub fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for SacnPriority {
    type Error = SacnPriorityError;
    fn try_from(n: u8) -> Result<Self, Self::Error> {
        Self::new(n)
    }
}

impl From<SacnPriority> for u8 {
    fn from(p: SacnPriority) -> Self {
        p.0
    }
}

impl FromStr for SacnPriority {
    type Err = SacnPriorityError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let n: u32 = s
            .trim()
            .parse()
            .map_err(|_| SacnPriorityError::Shape(s.into()))?;
        u8::try_from(n)
            .map_err(|_| SacnPriorityError::Range(n))
            .and_then(Self::new)
    }
}

impl fmt::Display for SacnPriority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Where the rig's sACN packets go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SacnDest {
    /// To one receiver's IPv4 address.
    Unicast {
        /// The receiver.
        #[serde(with = "ipv4_text")]
        addr: Ipv4Addr,
    },
    /// To the universe's multicast group ([`SacnUniverse::multicast_group`]), out of the
    /// interface that has this local address: a rig with a lighting network beside its
    /// LAN names which one the group is on rather than leaving it to the routing table.
    Multicast {
        /// A local IPv4 address of the interface toward the receivers.
        #[serde(with = "ipv4_text")]
        interface: Ipv4Addr,
    },
}

/// Where the rig's sACN universe goes, and with what priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SacnTarget {
    /// Unicast or multicast.
    pub dest: SacnDest,
    /// The universe.
    pub universe: SacnUniverse,
    /// The source's priority.
    pub priority: SacnPriority,
}

/// `Ipv4Addr` as dotted text: msgpack has no address type (serde would write four bytes),
/// and the text reads the same in every client language.
pub(crate) mod ipv4_text {
    use std::net::Ipv4Addr;

    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(a: &Ipv4Addr, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&a.to_string())
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Ipv4Addr, D::Error> {
        String::deserialize(d)?.parse().map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn universes_are_1_to_63999_with_their_group() {
        assert!(SacnUniverse::new(0).is_err());
        assert!(SacnUniverse::new(64_000).is_err());
        let u = SacnUniverse::new(1).expect("valid");
        assert_eq!(u.multicast_group(), Ipv4Addr::new(239, 255, 0, 1));
        let u = SacnUniverse::new(63_999).expect("valid");
        assert_eq!(u.multicast_group(), Ipv4Addr::new(239, 255, 249, 255));
        assert_eq!("300".parse::<SacnUniverse>().map(|u| u.get()), Ok(300));
        assert_eq!(
            "70000".parse::<SacnUniverse>(),
            Err(SacnUniverseError::Range(70_000))
        );
        assert!(matches!(
            "0.0.1".parse::<SacnUniverse>(),
            Err(SacnUniverseError::Shape(_))
        ));
        let b = rmp_serde::to_vec_named(&0u16).expect("valid");
        assert!(rmp_serde::from_slice::<SacnUniverse>(&b).is_err());
    }

    #[test]
    fn priorities_are_0_to_200() {
        assert_eq!(SacnPriority::DEFAULT.get(), 100);
        assert!(SacnPriority::new(200).is_ok());
        assert_eq!(SacnPriority::new(201), Err(SacnPriorityError::Range(201)));
        assert_eq!("0".parse::<SacnPriority>().map(|p| p.get()), Ok(0));
        assert!("high".parse::<SacnPriority>().is_err());
        let b = rmp_serde::to_vec_named(&250u8).expect("valid");
        assert!(rmp_serde::from_slice::<SacnPriority>(&b).is_err());
    }

    #[test]
    fn a_target_round_trips_with_its_addresses_as_text() {
        for dest in [
            SacnDest::Unicast {
                addr: Ipv4Addr::new(10, 0, 0, 40),
            },
            SacnDest::Multicast {
                interface: Ipv4Addr::new(10, 0, 0, 5),
            },
        ] {
            let t = SacnTarget {
                dest,
                universe: SacnUniverse::new(7).expect("valid"),
                priority: SacnPriority::DEFAULT,
            };
            let b = rmp_serde::to_vec_named(&t).expect("valid");
            assert_eq!(rmp_serde::from_slice::<SacnTarget>(&b).expect("valid"), t);
            let j = serde_json::to_value(t).expect("valid");
            assert!(j["dest"]["type"].is_string());
        }
        let j = serde_json::json!({
            "dest": {"type": "multicast", "interface": "10.0.0.5"},
            "universe": 1, "priority": 100
        });
        let t: SacnTarget = serde_json::from_value(j).expect("valid");
        assert_eq!(
            t.dest,
            SacnDest::Multicast {
                interface: Ipv4Addr::new(10, 0, 0, 5)
            }
        );
    }
}
