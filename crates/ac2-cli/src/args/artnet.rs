//! `ac2 artnet …`: where the rig's Art-Net universe goes.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::str::FromStr;

use ac2_proto::model::{ARTNET_PORT, ArtnetFade, DmxSet, PortAddress};
use clap::Subcommand;

use crate::units::{Time, UnitError};

/// `artnet …`.
#[derive(Debug, Subcommand)]
pub enum ArtnetCmd {
    /// Where the rig sends its Art-Net universe: a node's IPv4 address or a broadcast
    /// address (port 6454 unless given), and the universe as a number 0–32767 or as
    /// net.sub.uni; or `off` to send nothing. Kept by the rig, whatever session is open.
    #[command(after_help = ARTNET_TARGET_HELP)]
    Target {
        /// `ip[:port]` or `off`.
        addr: ArtnetAddrArg,
        /// Universe: `N` (0–32767) or `net.sub.uni` (e.g. `0.0.1`); required unless `off`.
        universe: Option<PortAddressArg>,
    },
    /// How long the lights take to move to a new cue frame: each channel fades from its
    /// level to the new one evenly in brightness. 0–60 s; 0 jumps. Kept by the rig.
    Fade {
        /// Seconds (`1.5`) or with a unit (`1.5s`, `500ms`).
        seconds: ArtnetFadeArg,
    },
    /// Send one frame with these levels (every other channel 0) now, to find a fixture's
    /// channels. It holds 5 s, then the cues' frame comes back.
    Test {
        /// `channel=value` (channel 1–512, value 0–255), e.g. `12=255 13=40`.
        #[arg(required = true)]
        channels: Vec<DmxSet>,
    },
}

const ARTNET_TARGET_HELP: &str = "\
ac2 owns one universe and sends all 512 channels; channels no cue sets are 0. Mix it with the
lighting desk at the receiver: an Art-Net input in QLC+ on this universe, or a node's HTP merge.

Examples: ac2 artnet target 2.255.255.255 0.0.1
          ac2 artnet target 192.168.1.40:6454 17
          ac2 artnet target off";

/// An Art-Net target address: `ip`, `ip:port` (IPv4), or `off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtnetAddrArg(pub Option<SocketAddr>);

impl FromStr for ArtnetAddrArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let t = s.trim();
        if t.eq_ignore_ascii_case("off") {
            return Ok(Self(None));
        }
        if let Ok(ip) = t.parse::<Ipv4Addr>() {
            return Ok(Self(Some(SocketAddrV4::new(ip, ARTNET_PORT).into())));
        }
        t.parse::<SocketAddrV4>()
            .map(|a| Self(Some(a.into())))
            .map_err(|_| {
                UnitError(format!(
                    "{s:?}: expected an IPv4 address with an optional port (e.g. \
                     2.255.255.255, 192.168.1.40:6454) or off"
                ))
            })
    }
}

/// An Art-Net fade: seconds, bare or with a unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArtnetFadeArg(pub ArtnetFade);

impl FromStr for ArtnetFadeArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let sec = match s.trim().parse::<f64>() {
            Ok(n) => n,
            Err(_) => s.parse::<Time>()?.0.0,
        };
        ArtnetFade::new(sec)
            .map(Self)
            .map_err(|e| UnitError(e.to_string()))
    }
}

/// An Art-Net universe: flat `N` or `net.sub.uni`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortAddressArg(pub PortAddress);

impl FromStr for PortAddressArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        s.parse::<PortAddress>()
            .map(Self)
            .map_err(|e| UnitError(e.to_string()))
    }
}
