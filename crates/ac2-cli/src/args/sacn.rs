//! `ac2 sacn …`: where the rig's universe goes as sACN (E1.31), beside Art-Net.

use std::net::Ipv4Addr;
use std::str::FromStr;

use ac2_proto::model::{SacnPriority, SacnUniverse};
use clap::Subcommand;

use ac2_scene::unit_text::UnitError;

/// `sacn …`.
#[derive(Debug, Subcommand)]
pub enum SacnCmd {
    /// Where the rig sends its universe as sACN: a receiver's IPv4 address, or `multicast`
    /// (the universe's group 239.255.x.y, out of the interface `--via` names); or `off` to end
    /// the stream. Kept by the rig, whatever session is open.
    #[command(after_help = SACN_TARGET_HELP)]
    Target {
        /// A receiver's IPv4 address, `multicast` or `off`.
        dest: SacnDestArg,
        /// Universe 1–63999; required unless `off`.
        universe: Option<SacnUniverseArg>,
        /// With `multicast`: this host's IPv4 address on the lighting network, the interface
        /// the group is sent out of.
        #[arg(long, value_name = "IP")]
        via: Option<Ipv4Addr>,
        /// 0–200: a receiver hearing several sources takes the highest. Default 100.
        #[arg(long, value_name = "P")]
        priority: Option<SacnPriorityArg>,
    },
}

const SACN_TARGET_HELP: &str = "\
sACN carries the same frame as Art-Net: the cues' levels, `ac2 artnet fade` and
`ac2 artnet test` apply to both. Off, a changed target and a clean daemon stop end the old
stream at once (Stream_Terminated), so receivers let go of the rig.

Examples: ac2 sacn target 192.168.1.40 1
          ac2 sacn target multicast 1 --via 192.168.1.4 --priority 120
          ac2 sacn target off";

/// Where an sACN target sends, as typed: an address, `multicast` or `off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SacnDestArg {
    Off,
    Unicast(Ipv4Addr),
    Multicast,
}

impl FromStr for SacnDestArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let t = s.trim();
        if t.eq_ignore_ascii_case("off") {
            return Ok(Self::Off);
        }
        if t.eq_ignore_ascii_case("multicast") {
            return Ok(Self::Multicast);
        }
        t.parse::<Ipv4Addr>().map(Self::Unicast).map_err(|_| {
            UnitError(format!(
                "{s:?}: expected a receiver's IPv4 address (e.g. 192.168.1.40), multicast or off"
            ))
        })
    }
}

/// An sACN universe, 1–63999.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SacnUniverseArg(pub SacnUniverse);

impl FromStr for SacnUniverseArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        s.parse::<SacnUniverse>()
            .map(Self)
            .map_err(|e| UnitError(e.to_string()))
    }
}

/// An sACN priority, 0–200.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SacnPriorityArg(pub SacnPriority);

impl FromStr for SacnPriorityArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        s.parse::<SacnPriority>()
            .map(Self)
            .map_err(|e| UnitError(e.to_string()))
    }
}
