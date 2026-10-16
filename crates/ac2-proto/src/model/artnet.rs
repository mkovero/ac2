//! The rig's Art-Net universe: DMX channel levels the cues set, and where the universe goes.

use std::fmt;
use std::net::SocketAddr;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Channels in a DMX512 universe.
pub const DMX_CHANNELS: usize = 512;

/// The UDP port Art-Net nodes listen on.
pub const ARTNET_PORT: u16 = 6454;

/// A DMX channel, 1–512 as lighting desks number them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct DmxChannel(u16);

/// Why a number is not a [`DmxChannel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("DMX channel {0} is outside 1–512")]
pub struct DmxChannelError(pub u16);

impl DmxChannel {
    /// Validates `n` (1–512).
    pub fn new(n: u16) -> Result<Self, DmxChannelError> {
        if (1..=DMX_CHANNELS as u16).contains(&n) {
            Ok(Self(n))
        } else {
            Err(DmxChannelError(n))
        }
    }

    /// The channel number, 1–512.
    pub fn get(self) -> u16 {
        self.0
    }

    /// The slot in a frame's data, 0–511.
    pub fn index(self) -> usize {
        usize::from(self.0 - 1)
    }
}

impl TryFrom<u16> for DmxChannel {
    type Error = DmxChannelError;
    fn try_from(n: u16) -> Result<Self, Self::Error> {
        Self::new(n)
    }
}

impl From<DmxChannel> for u16 {
    fn from(c: DmxChannel) -> Self {
        c.0
    }
}

impl fmt::Display for DmxChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// One channel's level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DmxSet {
    /// Channel.
    pub channel: DmxChannel,
    /// Level, 0–255.
    pub value: u8,
}

/// Why text is not a [`DmxSet`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DmxSetError {
    /// No `=`.
    #[error("{0:?}: expected channel=value, e.g. 12=255")]
    Shape(String),
    /// The channel is not 1–512.
    #[error("{0:?}: the channel is a number 1–512")]
    Channel(String),
    /// The value is not 0–255.
    #[error("{0:?}: the value is a number 0–255")]
    Value(String),
}

impl FromStr for DmxSet {
    type Err = DmxSetError;
    /// `channel=value`, e.g. `12=255`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (c, v) = s
            .split_once('=')
            .ok_or_else(|| DmxSetError::Shape(s.into()))?;
        let channel = c
            .trim()
            .parse::<u16>()
            .ok()
            .and_then(|n| DmxChannel::new(n).ok())
            .ok_or_else(|| DmxSetError::Channel(s.into()))?;
        let value = v
            .trim()
            .parse::<u8>()
            .map_err(|_| DmxSetError::Value(s.into()))?;
        Ok(Self { channel, value })
    }
}

impl fmt::Display for DmxSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}={}", self.channel, self.value)
    }
}

/// Parses a space- or comma-separated list of `channel=value`, e.g. `12=255 13=0`. A
/// channel listed twice is refused: which of the two levels was meant is a guess.
pub fn parse_dmx_list(s: &str) -> Result<Vec<DmxSet>, DmxSetError> {
    let mut out: Vec<DmxSet> = Vec::new();
    for item in s.split([' ', ',', '\t']).filter(|t| !t.is_empty()) {
        let d: DmxSet = item.parse()?;
        if out.iter().any(|x| x.channel == d.channel) {
            return Err(DmxSetError::Shape(format!(
                "channel {} is listed twice in {s:?}",
                d.channel
            )));
        }
        out.push(d);
    }
    Ok(out)
}

/// `sets` as text, e.g. `12=255 13=0`.
pub fn dmx_list_text(sets: &[DmxSet]) -> String {
    sets.iter()
        .map(DmxSet::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The universe the lists make: each channel at the highest level any list gives it (HTP,
/// as lighting consoles merge sources), 0 where none lists it.
pub fn dmx_htp<'a>(lists: impl IntoIterator<Item = &'a [DmxSet]>) -> [u8; DMX_CHANNELS] {
    let mut frame = [0u8; DMX_CHANNELS];
    for list in lists {
        for d in list {
            let slot = &mut frame[d.channel.index()];
            *slot = (*slot).max(d.value);
        }
    }
    frame
}

/// The channels of `frame` above 0, in channel order.
pub fn dmx_nonzero(frame: &[u8; DMX_CHANNELS]) -> Vec<DmxSet> {
    frame
        .iter()
        .enumerate()
        .filter(|(_, v)| **v > 0)
        .filter_map(|(i, v)| {
            let channel = DmxChannel::new(u16::try_from(i + 1).ok()?).ok()?;
            Some(DmxSet { channel, value: *v })
        })
        .collect()
}

/// An Art-Net Port-Address: 15 bits, net (7) · sub-net (4) · universe (4). Written
/// `net.sub.uni` (e.g. `0.0.1`) or as the flat number 0–32767.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct PortAddress(u16);

/// Why a number or text is not a [`PortAddress`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PortAddressError {
    /// Above 32767.
    #[error("Art-Net port-address {0} is above 32767")]
    Range(u32),
    /// A part of `net.sub.uni` out of its range.
    #[error("{0:?}: net is 0–127, sub-net 0–15, universe 0–15")]
    Part(String),
    /// Neither a number nor `net.sub.uni`.
    #[error("{0:?}: give a universe as a number 0–32767 or as net.sub.uni (e.g. 0.0.1)")]
    Shape(String),
}

impl PortAddress {
    /// Highest port-address.
    pub const MAX: u16 = 0x7fff;

    /// Validates a flat port-address (0–32767).
    pub fn new(n: u16) -> Result<Self, PortAddressError> {
        if n <= Self::MAX {
            Ok(Self(n))
        } else {
            Err(PortAddressError::Range(u32::from(n)))
        }
    }

    /// From net (0–127), sub-net (0–15) and universe (0–15).
    pub fn from_parts(net: u8, sub: u8, uni: u8) -> Result<Self, PortAddressError> {
        if net > 127 || sub > 15 || uni > 15 {
            return Err(PortAddressError::Part(format!("{net}.{sub}.{uni}")));
        }
        Ok(Self(
            (u16::from(net) << 8) | (u16::from(sub) << 4) | u16::from(uni),
        ))
    }

    /// The flat number, 0–32767.
    pub fn get(self) -> u16 {
        self.0
    }

    /// Net, 0–127 (bits 14–8).
    pub fn net(self) -> u8 {
        (self.0 >> 8) as u8
    }

    /// Sub-net, 0–15 (bits 7–4).
    pub fn sub(self) -> u8 {
        ((self.0 >> 4) & 0xf) as u8
    }

    /// Universe within the sub-net, 0–15 (bits 3–0).
    pub fn uni(self) -> u8 {
        (self.0 & 0xf) as u8
    }

    /// The low byte, sub-net and universe, as ArtDmx's `SubUni` carries it.
    pub fn sub_uni(self) -> u8 {
        (self.0 & 0xff) as u8
    }
}

impl TryFrom<u16> for PortAddress {
    type Error = PortAddressError;
    fn try_from(n: u16) -> Result<Self, Self::Error> {
        Self::new(n)
    }
}

impl From<PortAddress> for u16 {
    fn from(p: PortAddress) -> Self {
        p.0
    }
}

impl FromStr for PortAddress {
    type Err = PortAddressError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let shape = || PortAddressError::Shape(s.into());
        let parts: Vec<&str> = s.trim().split('.').collect();
        match parts.as_slice() {
            [n] => {
                let n: u32 = n.parse().map_err(|_| shape())?;
                u16::try_from(n)
                    .ok()
                    .filter(|n| *n <= Self::MAX)
                    .map(Self)
                    .ok_or(PortAddressError::Range(n))
            }
            [net, sub, uni] => {
                let p = |t: &str| {
                    t.parse::<u8>()
                        .map_err(|_| PortAddressError::Part(s.into()))
                };
                Self::from_parts(p(net)?, p(sub)?, p(uni)?)
                    .map_err(|_| PortAddressError::Part(s.into()))
            }
            _ => Err(shape()),
        }
    }
}

impl fmt::Display for PortAddress {
    /// `net.sub.uni`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.net(), self.sub(), self.uni())
    }
}

/// Where the rig's Art-Net universe goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtnetTarget {
    /// A node's IP literal (unicast) or a subnet's broadcast address, and the port (Art-Net
    /// uses [`ARTNET_PORT`]).
    #[serde(with = "addr_text")]
    pub addr: SocketAddr,
    /// The universe's port-address.
    pub universe: PortAddress,
}

/// How long the rig's DMX channels take to move to a new cue frame, 0–60 s; 0 jumps.
/// Without a setting it is [`ArtnetFade::DEFAULT`]: a light that changes over a second
/// reads as a change of state, not a flicker.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
pub struct ArtnetFade(f64);

/// Why a number is not an [`ArtnetFade`].
#[derive(Debug, Clone, Copy, PartialEq, Error)]
#[error("Art-Net fade {0} s is outside 0–60 s")]
pub struct ArtnetFadeError(pub f64);

impl ArtnetFade {
    /// Longest fade, seconds.
    pub const MAX: f64 = 60.0;
    /// The fade of a rig that has not set one.
    pub const DEFAULT: Self = Self(1.0);
    /// No fade: every change jumps.
    pub const SNAP: Self = Self(0.0);

    /// Validates `seconds` (finite, 0–60).
    pub fn new(seconds: f64) -> Result<Self, ArtnetFadeError> {
        if seconds.is_finite() && (0.0..=Self::MAX).contains(&seconds) {
            // -0.0 reads as 0 everywhere it is shown.
            Ok(Self(seconds + 0.0))
        } else {
            Err(ArtnetFadeError(seconds))
        }
    }

    /// Seconds.
    pub fn seconds(self) -> f64 {
        self.0
    }

    /// As a duration.
    pub fn duration(self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(self.0)
    }
}

impl Default for ArtnetFade {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl TryFrom<f64> for ArtnetFade {
    type Error = ArtnetFadeError;
    fn try_from(s: f64) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl From<ArtnetFade> for f64 {
    fn from(f: ArtnetFade) -> Self {
        f.0
    }
}

/// `SocketAddr` as `"ip:port"` text (see [`super::opt_addr`]).
mod addr_text {
    use std::net::SocketAddr;

    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(a: &SocketAddr, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&a.to_string())
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SocketAddr, D::Error> {
        String::deserialize(d)?.parse().map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(n: u16) -> DmxChannel {
        DmxChannel::new(n).expect("valid")
    }

    #[test]
    fn fades_are_0_to_60_s() {
        assert_eq!(ArtnetFade::new(0.0).map(ArtnetFade::seconds), Ok(0.0));
        assert_eq!(ArtnetFade::new(60.0).map(ArtnetFade::seconds), Ok(60.0));
        assert!(
            ArtnetFade::new(-0.0)
                .expect("valid")
                .seconds()
                .is_sign_positive()
        );
        for bad in [-0.1, 60.01, f64::NAN, f64::INFINITY] {
            assert!(ArtnetFade::new(bad).is_err(), "{bad}");
        }
        assert_eq!(ArtnetFade::default().seconds(), 1.0);
        let b = rmp_serde::to_vec_named(&61.0f64).expect("valid");
        assert!(rmp_serde::from_slice::<ArtnetFade>(&b).is_err());
        let b = rmp_serde::to_vec_named(&ArtnetFade::new(0.5).expect("valid")).expect("valid");
        assert_eq!(rmp_serde::from_slice::<f64>(&b).expect("valid"), 0.5);
    }

    #[test]
    fn channels_are_1_to_512() {
        assert_eq!(DmxChannel::new(0), Err(DmxChannelError(0)));
        assert_eq!(DmxChannel::new(513), Err(DmxChannelError(513)));
        assert_eq!(ch(1).index(), 0);
        assert_eq!(ch(512).index(), 511);
        let b = rmp_serde::to_vec_named(&0u16).expect("valid");
        assert!(rmp_serde::from_slice::<DmxChannel>(&b).is_err());
        let b = rmp_serde::to_vec_named(&512u16).expect("valid");
        assert_eq!(
            rmp_serde::from_slice::<DmxChannel>(&b).expect("valid"),
            ch(512)
        );
    }

    #[test]
    fn sets_parse_and_print() {
        let l = parse_dmx_list("12=255 13=0,14=40").expect("valid");
        assert_eq!(dmx_list_text(&l), "12=255 13=0 14=40");
        assert!(matches!("12".parse::<DmxSet>(), Err(DmxSetError::Shape(_))));
        assert!(matches!(
            "0=1".parse::<DmxSet>(),
            Err(DmxSetError::Channel(_))
        ));
        assert!(matches!(
            "1=256".parse::<DmxSet>(),
            Err(DmxSetError::Value(_))
        ));
        assert!(parse_dmx_list("1=2 1=3").is_err());
        assert_eq!(parse_dmx_list("").expect("valid"), []);
    }

    #[test]
    fn htp_takes_the_highest_level_and_zero_elsewhere() {
        let a = [
            DmxSet {
                channel: ch(12),
                value: 255,
            },
            DmxSet {
                channel: ch(13),
                value: 10,
            },
        ];
        let b = [
            DmxSet {
                channel: ch(12),
                value: 0,
            },
            DmxSet {
                channel: ch(13),
                value: 40,
            },
            DmxSet {
                channel: ch(512),
                value: 1,
            },
        ];
        let f = dmx_htp([&a[..], &b[..]]);
        assert_eq!(f[11], 255);
        assert_eq!(f[12], 40);
        assert_eq!(f[511], 1);
        assert_eq!(f.iter().filter(|v| **v > 0).count(), 3);
        assert_eq!(dmx_list_text(&dmx_nonzero(&f)), "12=255 13=40 512=1");
        assert_eq!(dmx_htp([]), [0u8; DMX_CHANNELS]);
    }

    #[test]
    fn port_address_splits_into_net_sub_universe() {
        let p = PortAddress::from_parts(1, 2, 3).expect("valid");
        assert_eq!(p.get(), 0x0123);
        assert_eq!((p.net(), p.sub(), p.uni()), (1, 2, 3));
        assert_eq!(p.sub_uni(), 0x23);
        assert_eq!(p.to_string(), "1.2.3");
        let top = PortAddress::new(PortAddress::MAX).expect("valid");
        assert_eq!((top.net(), top.sub(), top.uni()), (127, 15, 15));
        assert!(PortAddress::new(0x8000).is_err());
        assert!(PortAddress::from_parts(128, 0, 0).is_err());
        assert!(PortAddress::from_parts(0, 16, 0).is_err());
        assert!(PortAddress::from_parts(0, 0, 16).is_err());
    }

    #[test]
    fn port_address_reads_flat_or_dotted() {
        assert_eq!("0.0.1".parse::<PortAddress>().expect("valid").get(), 1);
        assert_eq!("1.0.0".parse::<PortAddress>().expect("valid").get(), 256);
        assert_eq!("32767".parse::<PortAddress>().expect("valid").get(), 32767);
        assert_eq!(
            "32768".parse::<PortAddress>(),
            Err(PortAddressError::Range(32768))
        );
        assert!(matches!(
            "0.16.0".parse::<PortAddress>(),
            Err(PortAddressError::Part(_))
        ));
        assert!(matches!(
            "0.1".parse::<PortAddress>(),
            Err(PortAddressError::Shape(_))
        ));
        assert!(matches!(
            "x".parse::<PortAddress>(),
            Err(PortAddressError::Shape(_))
        ));
        let b = rmp_serde::to_vec_named(&40000u16).expect("valid");
        assert!(rmp_serde::from_slice::<PortAddress>(&b).is_err());
    }

    #[test]
    fn target_round_trips_with_its_address_as_text() {
        let t = ArtnetTarget {
            addr: "2.255.255.255:6454".parse().expect("valid"),
            universe: PortAddress::new(1).expect("valid"),
        };
        let b = rmp_serde::to_vec_named(&t).expect("valid");
        assert_eq!(rmp_serde::from_slice::<ArtnetTarget>(&b).expect("valid"), t);
        #[derive(serde::Deserialize)]
        struct Raw {
            addr: String,
            universe: u16,
        }
        let raw: Raw = rmp_serde::from_slice(&b).expect("valid");
        assert_eq!(raw.addr, "2.255.255.255:6454");
        assert_eq!(raw.universe, 1);
    }
}
