//! How the daemon serves clients (`server.info`, `server.network`, `State.server`): the local
//! pair it always serves, the network listener beside it, and who may connect over it.

use std::fmt;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::units::{RigId, WallNs};

/// How the daemon serves clients, and who may connect (`server.info`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerInfo {
    /// The local pair; `None` to a network client, which has no use for this host's local
    /// endpoints.
    pub local: Option<LocalServer>,
    /// The network listener and its keys.
    pub network: NetworkServer,
    /// Whether `server.network` may switch the network, as it would answer.
    pub toggle: NetToggle,
    /// The host's interfaces the network can bind (loopback left out), read at each
    /// `server.info`; empty to a network client.
    pub interfaces: Vec<NetInterface>,
    /// Where raw capture files are recorded on the daemon host; `None`: this daemon does
    /// not record.
    pub recording_dir: Option<String>,
}

/// Whether `server.network` from the asking client may switch the network: the same answer
/// the command itself gives it, so an app offers the switch only where it works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NetToggle {
    /// It may.
    Allowed,
    /// Fixed by `ac2d --listen` for the daemon's life.
    Fixed,
    /// This daemon has no network to switch.
    Unavailable {
        /// Why.
        reason: NetUnavailable,
    },
}

/// Why the network cannot be switched from the asking client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetUnavailable {
    /// It serves in its own process only (embedded in an app or a test).
    InProcess,
    /// It keeps no key files (no config directory), so it has no server key to serve
    /// CURVE with.
    NoKeyStore,
    /// The asker is a network client: only a client on the rig's own host may switch it.
    NotLocal,
}

/// The local pair, served for the daemon's whole life.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LocalServer {
    /// In this process only (a daemon embedded in an app or a test): there is no network
    /// to turn on.
    Embedded,
    /// This machine only (`ipc://` or loopback TCP): the operating system's user boundary
    /// is the trust boundary.
    Local {
        /// Ctrl endpoint.
        ctrl: String,
        /// Data endpoint.
        data: String,
    },
}

/// The network listener: CURVE on both sockets, only authorized keys connect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkServer {
    /// What it is set to, as much as the asking client may know.
    pub setting: NetSettingView,
    /// What it is doing, as much as the asking client may know.
    pub status: NetStatusView,
    /// The server's public key (Z85), which clients pin when pairing; `None` until the
    /// network is first turned on.
    pub server_key: Option<String>,
    /// Its fingerprint, as `ac2 discover` and the connect dialog show it.
    pub fingerprint: Option<String>,
    /// The name the rig is advertised under over mDNS; `None`: not advertised.
    pub advertised_as: Option<String>,
    /// The authorized clients, by name.
    pub authorized: Vec<AuthorizedClient>,
    /// Keys refused lately, newest first (bounded); `None` to a network client: who else
    /// tried this rig, and from where, is for its own host to judge.
    pub refused: Option<Vec<RefusedKey>>,
}

/// The network setting as one client is told it. A network client learns only whether it
/// is on: where the rig binds names this host's interfaces and addresses, and why a setting
/// is not kept carries the file system's words, neither of which a remote app needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NetSettingView {
    /// To a client on the rig's own host: all of it.
    Host {
        /// The setting.
        setting: NetSetting,
    },
    /// To a network client.
    Peer {
        /// Its kind only.
        setting: NetSettingKind,
    },
}

/// [`NetSetting`] without its details.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetSettingKind {
    /// Local only.
    Off,
    /// On, from `server.network`.
    On,
    /// Fixed by `ac2d --listen`.
    Fixed,
}

/// What the network is set to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NetSetting {
    /// Local only.
    Off {
        /// Where it was last on, which `server.network` turns it on at again unless told
        /// otherwise; `None` until it was first on.
        kept: Option<NetChoice>,
        /// Whether that outlives a restart.
        saved: NetSaved,
    },
    /// On, from `server.network`.
    On {
        /// Where.
        choice: NetChoice,
        /// Whether that outlives a restart.
        saved: NetSaved,
    },
    /// Fixed for the daemon's life by `ac2d --listen`: `server.network` is refused, and
    /// nothing is kept (the command line is the setting).
    Fixed {
        /// The address as given, `tcp://<interface or address>:<port>`.
        listen: String,
    },
}

impl NetSetting {
    /// Its kind, as a network client is told it.
    pub fn kind(&self) -> NetSettingKind {
        match self {
            Self::Off { .. } => NetSettingKind::Off,
            Self::On { .. } => NetSettingKind::On,
            Self::Fixed { .. } => NetSettingKind::Fixed,
        }
    }
}

/// Where the network binds, and its port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetChoice {
    /// Where it binds.
    pub bind: NetBind,
    /// Ctrl port; data is on the next one.
    pub port: u16,
}

/// Where the network binds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NetBind {
    /// Every IPv4 interface (`tcp://*`).
    All,
    /// One interface by name, resolved by the daemon to its address at each bind: its first
    /// IPv4 address, else its first global IPv6 one.
    Interface {
        /// OS name (`eth0`, `en0`, `Ethernet 2`).
        name: String,
    },
    /// One address of this host.
    Addr {
        /// The address.
        #[serde(with = "ip_text")]
        addr: IpAddr,
    },
}

/// Whether the network setting outlives a restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NetSaved {
    /// Written: the daemon starts with it.
    Saved,
    /// In force now, lost at the next start.
    NotSaved {
        /// Why.
        reason: NotSavedReason,
    },
}

/// Why the network setting is not kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NotSavedReason {
    /// The settings file cannot be written (read-only file or directory).
    StoreReadOnly,
    /// The settings file could not be read at start: it is left as it is, never written
    /// over, until the daemon restarts and can read it.
    Unreadable {
        /// As the file system said it.
        detail: String,
    },
    /// The settings file was damaged and could not be set aside: each change tries again,
    /// and writes only once it is.
    DamagedInPlace,
    /// This daemon keeps no settings file (an in-process daemon, or one given no
    /// settings path).
    NoStore,
    /// The write failed.
    WriteFailed {
        /// As the file system said it.
        detail: String,
    },
}

/// How the network listener is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NetStatus {
    /// Not served.
    Off,
    /// Bound: the endpoints as bound.
    Listening {
        /// Ctrl endpoint.
        ctrl: String,
        /// Data endpoint.
        data: String,
    },
    /// Not bound yet; retried.
    Waiting {
        /// Why.
        reason: NetReason,
    },
    /// Was bound and failed; retried with a new context.
    Failed {
        /// Why.
        reason: NetReason,
    },
}

impl NetStatus {
    /// Its kind, as `State.server` carries it.
    pub fn kind(&self) -> NetStatusKind {
        match self {
            Self::Off => NetStatusKind::Off,
            Self::Listening { .. } => NetStatusKind::Listening,
            Self::Waiting { .. } => NetStatusKind::Waiting,
            Self::Failed { .. } => NetStatusKind::Failed,
        }
    }
}

/// The listener's status as one client is told it. A network client gets why it is not
/// listening by kind only: an unforeseen failure is worded by the host's OS, which a
/// remote app has no business reading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NetStatusView {
    /// To a client on the rig's own host: all of it.
    Host {
        /// The status.
        status: NetStatus,
    },
    /// To a network client.
    Peer {
        /// The status, reasons by kind.
        status: PeerNetStatus,
    },
}

impl NetStatusView {
    /// Its kind.
    pub fn kind(&self) -> NetStatusKind {
        match self {
            Self::Host { status } => status.kind(),
            Self::Peer { status } => match status {
                PeerNetStatus::Off => NetStatusKind::Off,
                PeerNetStatus::Listening { .. } => NetStatusKind::Listening,
                PeerNetStatus::Waiting { .. } => NetStatusKind::Waiting,
                PeerNetStatus::Failed { .. } => NetStatusKind::Failed,
            },
        }
    }

    /// The ctrl and data endpoints while listening.
    pub fn endpoints(&self) -> Option<(&str, &str)> {
        match self {
            Self::Host {
                status: NetStatus::Listening { ctrl, data },
            }
            | Self::Peer {
                status: PeerNetStatus::Listening { ctrl, data },
            } => Some((ctrl, data)),
            _ => None,
        }
    }
}

/// [`NetStatus`] as a network client is told it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PeerNetStatus {
    /// Not served.
    Off,
    /// Bound: the endpoints as bound (the client reached one of them).
    Listening {
        /// Ctrl endpoint.
        ctrl: String,
        /// Data endpoint.
        data: String,
    },
    /// Not bound yet; retried.
    Waiting {
        /// Why, by kind.
        reason: NetReasonKind,
    },
    /// Was bound and failed; retried.
    Failed {
        /// Why, by kind.
        reason: NetReasonKind,
    },
}

impl From<&NetStatus> for PeerNetStatus {
    fn from(s: &NetStatus) -> Self {
        match s {
            NetStatus::Off => Self::Off,
            NetStatus::Listening { ctrl, data } => Self::Listening {
                ctrl: ctrl.clone(),
                data: data.clone(),
            },
            NetStatus::Waiting { reason } => Self::Waiting {
                reason: reason.kind(),
            },
            NetStatus::Failed { reason } => Self::Failed {
                reason: reason.kind(),
            },
        }
    }
}

/// Why the network is not listening.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NetReason {
    /// Another listener holds the port.
    AddressInUse,
    /// No interface of that name on this host.
    InterfaceMissing,
    /// The address (or any usable address of the interface) is not on this host.
    AddressMissing,
    /// The ZAP handler that authenticates the network's clients stopped.
    ZapFailed,
    /// No server key to serve CURVE with (no key files: an in-process or ephemeral daemon).
    NoServerKey,
    /// The daemon's I/O thread could not be handed the sockets (its internal pipe stayed
    /// full).
    NotHandedOver,
    /// Anything else.
    Other {
        /// What failed.
        detail: String,
    },
}

/// [`NetReason`] without its details.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetReasonKind {
    /// [`NetReason::AddressInUse`].
    AddressInUse,
    /// [`NetReason::InterfaceMissing`].
    InterfaceMissing,
    /// [`NetReason::AddressMissing`].
    AddressMissing,
    /// [`NetReason::ZapFailed`].
    ZapFailed,
    /// [`NetReason::NoServerKey`].
    NoServerKey,
    /// [`NetReason::NotHandedOver`].
    NotHandedOver,
    /// [`NetReason::Other`].
    Other,
}

impl NetReason {
    /// Its kind, as a network client is told it.
    pub fn kind(&self) -> NetReasonKind {
        match self {
            Self::AddressInUse => NetReasonKind::AddressInUse,
            Self::InterfaceMissing => NetReasonKind::InterfaceMissing,
            Self::AddressMissing => NetReasonKind::AddressMissing,
            Self::ZapFailed => NetReasonKind::ZapFailed,
            Self::NoServerKey => NetReasonKind::NoServerKey,
            Self::NotHandedOver => NetReasonKind::NotHandedOver,
            Self::Other { .. } => NetReasonKind::Other,
        }
    }
}

/// For logs; apps word the reason themselves.
impl fmt::Display for NetReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AddressInUse => f.write_str("the port is in use"),
            Self::InterfaceMissing => f.write_str("no such interface"),
            Self::AddressMissing => f.write_str("the address is not on this host"),
            Self::ZapFailed => f.write_str("the authentication handler stopped"),
            Self::NoServerKey => f.write_str("no server key"),
            Self::NotHandedOver => f.write_str("the I/O thread did not take the network sockets"),
            Self::Other { detail } => f.write_str(detail),
        }
    }
}

/// One interface of the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetInterface {
    /// OS name (`eth0`, `en0`, `Ethernet 2`).
    pub name: String,
    /// Its addresses.
    #[serde(with = "ips_text")]
    pub addrs: Vec<IpAddr>,
    /// Operationally up.
    pub up: bool,
}

/// What every mirrored client sees of the network (`State.server`), so a change made
/// elsewhere shows at once; the details and key lists are in `server.info`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerSummary {
    /// What the listener is doing.
    pub status: NetStatusKind,
    /// Ctrl endpoint while listening.
    pub ctrl: Option<String>,
    /// The ctrl port it is set to; `None` while off.
    pub port: Option<u16>,
    /// The server key's fingerprint, once a key exists.
    pub fingerprint: Option<String>,
}

/// [`NetStatus`] without its details.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetStatusKind {
    /// Not served.
    Off,
    /// Bound.
    Listening,
    /// Not bound yet; retried.
    Waiting,
    /// Was bound and failed; retried.
    Failed,
}

/// One authorized client key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizedClient {
    /// Its name (the identity its requests carry: lease owner, audit).
    pub name: String,
    /// Public key (Z85).
    pub key: String,
    /// Fingerprint of the key.
    pub fingerprint: String,
    /// The rig it is, for a key admitted as a rig (`auth.admit_rig`); `None` for an app.
    pub rig: Option<RigId>,
}

/// A client key the daemon refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefusedKey {
    /// Public key (Z85); `None` when the peer did not use CURVE.
    pub key: Option<String>,
    /// Fingerprint of the key; `None` with `key`.
    pub fingerprint: Option<String>,
    /// Peer address.
    pub address: String,
    /// Refusals of this key from this address since the daemon started.
    pub count: u64,
    /// The latest refusal.
    pub last_at: WallNs,
}

/// An address as text: msgpack has no address type, and the text reads the same in every
/// client language.
mod ip_text {
    use std::net::IpAddr;

    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(a: &IpAddr, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&a.to_string())
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<IpAddr, D::Error> {
        String::deserialize(d)?.parse().map_err(D::Error::custom)
    }
}

/// [`ip_text`] for a list.
mod ips_text {
    use std::net::IpAddr;

    use serde::de::Error as _;
    use serde::ser::SerializeSeq;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(a: &[IpAddr], s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(a.len()))?;
        for ip in a {
            seq.serialize_element(&ip.to_string())?;
        }
        seq.end()
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<IpAddr>, D::Error> {
        Vec::<String>::deserialize(d)?
            .iter()
            .map(|s| s.parse().map_err(D::Error::custom))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A network client is told why by kind: an unforeseen failure's words stay on the host.
    #[test]
    fn a_peer_is_told_reasons_by_kind_only() {
        let failed = NetStatus::Failed {
            reason: NetReason::Other {
                detail: "bind: Permission denied (os error 13) on /secret/path".into(),
            },
        };
        let told = PeerNetStatus::from(&failed);
        assert_eq!(
            told,
            PeerNetStatus::Failed {
                reason: NetReasonKind::Other
            }
        );
        let b = rmp_serde::to_vec_named(&told).expect("encode");
        assert!(!b.windows(6).any(|w| w == b"secret"));
        let view = NetStatusView::Peer { status: told };
        assert_eq!(view.kind(), NetStatusKind::Failed);
        assert_eq!(view.endpoints(), None);
    }

    #[test]
    fn addresses_travel_as_text() {
        let i = NetInterface {
            name: "eth0".into(),
            addrs: vec![
                "192.168.1.20".parse().expect("ip"),
                "fe80::1".parse().expect("ip"),
            ],
            up: true,
        };
        let b = rmp_serde::to_vec_named(&i).expect("encode");
        #[derive(Deserialize)]
        struct Raw {
            addrs: Vec<String>,
        }
        let raw: Raw = rmp_serde::from_slice(&b).expect("text");
        assert_eq!(raw.addrs, ["192.168.1.20", "fe80::1"]);
        assert_eq!(
            rmp_serde::from_slice::<NetInterface>(&b).expect("decode"),
            i
        );
        let bind = NetBind::Addr {
            addr: "10.0.0.2".parse().expect("ip"),
        };
        let b = rmp_serde::to_vec_named(&bind).expect("encode");
        assert_eq!(rmp_serde::from_slice::<NetBind>(&b).expect("decode"), bind);
    }
}
