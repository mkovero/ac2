//! Rigs in `ui.toml`: the `[[rigs]]` tables of remembered rigs, and rig ids as the file
//! writes them.

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

/// The file's key of a device remembered for a rig: the rig's id (16 hex digits) and the
/// device's key, `000000000000fa4e/jack:system`. Keys are strings in TOML.
pub(super) fn rig_key((rig, device): &(RigId, String)) -> String {
    format!("{rig}/{device}")
}

/// A key written by [`rig_key`]. A key without a rig (written before devices were
/// remembered per rig) names no rig's device, so it is dropped, and the next save leaves
/// it out.
pub(super) fn parse_rig_key(key: &str) -> Option<(RigId, String)> {
    let (rig, device) = key.split_once('/')?;
    Some((parse_rig_id(rig)?, device.to_owned()))
}

/// A rig id as the file writes it, 16 hex digits; `None` for one no daemon has.
pub(super) fn parse_rig_id(s: &str) -> Option<RigId> {
    Some(s)
        .filter(|s| s.len() == 16)
        .and_then(|s| u64::from_str_radix(s, 16).ok())
        .filter(|n| *n != 0)
        .map(RigId)
}
