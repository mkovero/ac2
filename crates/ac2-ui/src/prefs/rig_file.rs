//! Remembered rigs in `ui.toml`: the `[[rigs]]` tables.

use super::file::parse_rig_id;
use super::*;

/// A remembered rig: its id as 16 hex digits (as in the per-rig keys), its name, and where
/// it was reached.
#[derive(Debug, Deserialize, Serialize)]
pub(super) struct RigFile {
    id: String,
    name: String,
    #[serde(flatten)]
    addr: RigAddrFile,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "reach", rename_all = "snake_case")]
enum RigAddrFile {
    Local,
    Embedded {
        backend: String,
    },
    Network {
        host: String,
        port: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fingerprint: Option<String>,
    },
    Endpoints {
        ctrl: String,
        data: String,
    },
}

impl RigFile {
    pub(super) fn of(r: &RememberedRig) -> Self {
        Self {
            id: r.id.to_string(),
            name: r.name.clone(),
            addr: match &r.addr {
                RigAddr::Local => RigAddrFile::Local,
                RigAddr::Embedded(b) => RigAddrFile::Embedded { backend: b.clone() },
                RigAddr::Network {
                    host,
                    port,
                    fingerprint,
                } => RigAddrFile::Network {
                    host: host.clone(),
                    port: *port,
                    fingerprint: fingerprint.clone(),
                },
                RigAddr::Endpoints { ctrl, data } => RigAddrFile::Endpoints {
                    ctrl: ctrl.clone(),
                    data: data.clone(),
                },
            },
        }
    }

    /// The rig, unless its id is not one a daemon has (dropped, as an unknown key is).
    pub(super) fn rig(self) -> Option<RememberedRig> {
        let id = parse_rig_id(&self.id)?;
        Some(RememberedRig {
            id,
            name: self.name,
            addr: match self.addr {
                RigAddrFile::Local => RigAddr::Local,
                RigAddrFile::Embedded { backend } => RigAddr::Embedded(backend),
                RigAddrFile::Network {
                    host,
                    port,
                    fingerprint,
                } => RigAddr::Network {
                    host,
                    port,
                    fingerprint,
                },
                RigAddrFile::Endpoints { ctrl, data } => RigAddr::Endpoints { ctrl, data },
            },
        })
    }
}
