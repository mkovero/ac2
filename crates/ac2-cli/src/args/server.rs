//! `ac2 server …`: how the daemon serves clients, its network listener switched live, and
//! the client keys it admits.

use std::net::IpAddr;

use ac2_proto::model::NetBind;
use clap::{ArgGroup, Args, Subcommand};

/// `server …`.
#[derive(Debug, Subcommand)]
pub enum ServerCmd {
    /// How the daemon serves clients: its local endpoints, the network (setting, status,
    /// server key), the authorized and lately refused client keys, the host's interfaces.
    Info,
    /// The network listener beside the local one: alone, shows its setting, status and
    /// whether the setting is kept; `on` / `off` switch it at once, session and local
    /// clients running on. Only on the rig's own host, and not while `ac2d --listen` fixes it.
    Network {
        /// Action; none shows the network.
        #[command(subcommand)]
        cmd: Option<NetworkCmd>,
    },
    /// Authorize a client key under NAME: its connections are admitted from now on, network
    /// on or off. Give the fingerprint of a key the daemon refused (`ac2 server info` lists
    /// them; the refused client prints its own), or the key itself.
    #[command(group(ArgGroup::new("which").required(true).args(["refused", "key"])))]
    Authorize {
        /// The name its requests carry (1 … 64 characters, no spaces).
        name: String,
        /// The fingerprint of a refused key, as listed (`1a2b-3c4d-…`).
        #[arg(long, value_name = "FINGERPRINT")]
        refused: Option<String>,
        /// The client's Z85 public key (40 characters); may start with `-`.
        #[arg(long, allow_hyphen_values = true)]
        key: Option<String>,
    },
}

/// `server network …`.
#[derive(Debug, Subcommand)]
pub enum NetworkCmd {
    /// Turn the network on. Where and on which port: as given, else as last set (first
    /// time: every interface, port 47820; data on the next port). Stored, so the daemon
    /// starts with it; an address that cannot be bound now is kept and retried.
    On(NetworkOn),
    /// Turn the network off: every network client is cut off at once; local ones run on.
    Off,
}

/// `server network on`.
#[derive(Debug, Args)]
#[command(group(ArgGroup::new("bind").args(["interface", "addr", "all"])))]
pub struct NetworkOn {
    /// One interface by name (`eth0`, `en0`, `Ethernet 2`), bound at its address as it is
    /// at each bind.
    #[arg(long, value_name = "NAME")]
    pub interface: Option<String>,
    /// One address of this host.
    #[arg(long, value_name = "IP")]
    pub addr: Option<IpAddr>,
    /// Every interface.
    #[arg(long)]
    pub all: bool,
    /// Ctrl port; data is on the next one (1 … 65534).
    #[arg(long)]
    pub port: Option<u16>,
}

impl NetworkOn {
    /// Where to bind; `None` keeps the stored choice.
    pub fn bind(&self) -> Option<NetBind> {
        if let Some(name) = &self.interface {
            Some(NetBind::Interface { name: name.clone() })
        } else if let Some(addr) = self.addr {
            Some(NetBind::Addr { addr })
        } else {
            self.all.then_some(NetBind::All)
        }
    }
}
