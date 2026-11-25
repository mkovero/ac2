//! `ac2 auth …`: who may use the daemon (client keys and their roles, other rigs), and this
//! client's own keys for remote mode.

use ac2_client::RemoteAddr;
use ac2_proto::model::ClientRole;
use clap::{ArgGroup, Subcommand, ValueEnum};

/// A role `ac2 auth add` / `ac2 auth role` gives; a rig is admitted with `admit-rig`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RoleArg {
    /// Drives the rig.
    Operator,
    /// Only looks; may stop the generator.
    Viewer,
}

impl From<RoleArg> for ClientRole {
    fn from(r: RoleArg) -> Self {
        match r {
            RoleArg::Operator => Self::Operator,
            RoleArg::Viewer => Self::Viewer,
        }
    }
}

/// `auth …`.
#[derive(Debug, Subcommand)]
pub enum AuthCmd {
    /// Authorize a client key under NAME as an operator or a viewer: its connections are
    /// admitted from now on, network on or off. Give the key itself, or the fingerprint of
    /// a key the daemon refused (`ac2 auth list` shows them on the daemon host; the refused
    /// client prints its own). A name given again takes the new key and role.
    #[command(group(ArgGroup::new("which").required(true).args(["key", "refused"])))]
    Add {
        /// The name its requests carry (1 … 64 characters, no spaces).
        name: String,
        /// The client's Z85 public key (40 characters); may start with `-`.
        #[arg(allow_hyphen_values = true)]
        key: Option<String>,
        /// The fingerprint of a refused key, as listed (`1a2b-3c4d-…`).
        #[arg(long, value_name = "FINGERPRINT")]
        refused: Option<String>,
        /// What it may do: `operator` drives the rig, `viewer` only looks (and may stop the
        /// generator).
        #[arg(long, value_enum)]
        role: RoleArg,
    },
    /// Change the role of the client authorized as NAME, keeping its key: its connections
    /// drop and come back with the new role; made a viewer, it loses the generator lease
    /// (output fades out) and its transfers.
    Role {
        /// The authorized name.
        name: String,
        /// The new role.
        #[arg(value_enum)]
        role: RoleArg,
    },
    /// Remove the client key authorized as NAME (an app's or an admitted rig's): its
    /// requests are refused at once, its data connection ends, and a lease it holds ends.
    Remove {
        /// The authorized name.
        name: String,
    },
    /// The authorized client keys with their roles, and (on the daemon host) the keys it
    /// lately refused.
    List,
    /// Pin a daemon's public key and print this client's key for the daemon operator to
    /// authorize. Compare the printed fingerprint with the one the daemon shows.
    Pair {
        /// Daemon host as used with `--remote`.
        host: RemoteAddr,
        /// The daemon's Z85 public key (40 characters). Z85 includes `-`, so a key may start
        /// with one; it is still read as the value.
        #[arg(long, allow_hyphen_values = true)]
        server_key: String,
        /// Name to authorize this client under (default: this host's name).
        #[arg(long)]
        name: Option<String>,
    },
    /// Show this client's key and the pinned daemon keys.
    Show,
    /// Admit another rig on the daemon: it may then take this rig's inputs (and nothing
    /// else). Give it the other rig's server key and id (`ac2 status` there); removed with
    /// `ac2 auth remove` under the same name.
    AdmitRig {
        /// The name its connections carry.
        name: String,
        /// The other rig's Z85 server key (40 characters).
        #[arg(long, allow_hyphen_values = true)]
        key: String,
        /// The other rig's id, 16 hex digits.
        #[arg(long)]
        rig_id: String,
    },
    /// Pair the daemon with another rig it may take inputs from (that rig admits this one
    /// with `admit-rig`).
    PeerAdd {
        /// Its name, as `--remote-in` names it.
        name: String,
        /// Its id, 16 hex digits.
        #[arg(long)]
        rig_id: String,
        /// Its address as `--remote` takes it (`host[:port]`, data on port + 1).
        #[arg(long)]
        at: RemoteAddr,
        /// Its Z85 server key (40 characters).
        #[arg(long, allow_hyphen_values = true)]
        key: String,
    },
    /// Unpair the daemon from a rig.
    PeerRemove {
        /// Its name.
        name: String,
    },
}
