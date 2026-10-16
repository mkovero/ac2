//! The rig entity: settings that belong to the daemon's machine rather than to a session,
//! where cues go, how sending fares and the state of every cue.

use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use super::{ArtnetFade, ArtnetTarget, DmxSet, RigCue};
use crate::units::Rev;

/// How sending to one kind of receiver (OSC, Art-Net) fares.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputStatus {
    /// Packets sent since the daemon started.
    pub sent: u64,
    /// The latest send error, as the operating system said it; `None` once a send works.
    pub last_error: Option<String>,
}

/// The rig entity: settings that belong to the daemon's machine rather than to a session,
/// and the monitor running on them (never deleted).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rig {
    /// Rev at which these settings were last committed; `osc.target`, `artnet.target`,
    /// `artnet.fade` and `rig.monitor_start` name it as their `base`.
    pub rev: Rev,
    /// Where OSC cues go (an IP literal and port); `None`: nowhere.
    #[serde(with = "opt_addr")]
    pub osc_target: Option<SocketAddr>,
    /// Where the Art-Net universe goes; `None`: nowhere.
    pub artnet_target: Option<ArtnetTarget>,
    /// How long the DMX channels take to move to a new cue frame (0: they jump).
    pub artnet_fade: ArtnetFade,
    /// At start the daemon reopens the last session's device capture-only, so the rig
    /// measurements meter with no client connected.
    pub monitor_at_start: bool,
    /// Sending OSC.
    pub osc: OutputStatus,
    /// Sending Art-Net.
    pub artnet: OutputStatus,
    /// The cue universe's channels above 0, in channel order: what the cues' current states
    /// make (whether or not a target is set).
    pub dmx: Vec<DmxSet>,
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
    use crate::model::{CueState, CueSubject, DmxChannel, PeakQuantity, PortAddress};
    use crate::units::MeasId;

    #[test]
    fn rig_round_trips_with_its_addresses_as_text() {
        let rig = Rig {
            rev: Rev(7),
            osc_target: Some("192.168.1.20:9000".parse().expect("valid")),
            artnet_target: Some(ArtnetTarget {
                addr: "192.168.1.255:6454".parse().expect("valid"),
                universe: PortAddress::from_parts(0, 0, 1).expect("valid"),
            }),
            artnet_fade: ArtnetFade::new(2.5).expect("valid"),
            monitor_at_start: true,
            osc: OutputStatus {
                sent: 3,
                last_error: Some("unreachable".into()),
            },
            artnet: OutputStatus {
                sent: 9,
                last_error: None,
            },
            dmx: vec![DmxSet {
                channel: DmxChannel::new(12).expect("valid"),
                value: 255,
            }],
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
        assert_eq!(v.artnet_target.addr, "192.168.1.255:6454");
    }

    /// Just the address fields, decoded as the text they are on the wire.
    mod rmpv_like {
        #[derive(serde::Deserialize)]
        pub struct Value {
            pub osc_target: Option<String>,
            pub artnet_target: Target,
        }
        #[derive(serde::Deserialize)]
        pub struct Target {
            pub addr: String,
        }
    }
}
