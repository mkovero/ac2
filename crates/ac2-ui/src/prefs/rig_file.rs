//! Rigs in `ui.toml`: the `[[rigs]]` tables of remembered rigs, the `[daemon]` table, and
//! rig ids as the file writes them.

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

/// The `[[rigs]]` list, read entry by entry: one entry that does not parse costs only
/// itself, never the whole file (set aside as damaged). A rig the app hosted itself
/// (`reach = "embedded"`, a daemon inside the app, which it no longer has) is counted, so the
/// app can say this computer's `ac2d` takes its place.
#[derive(Debug, Default)]
pub(super) struct RigsFile {
    pub(super) rigs: Vec<RigFile>,
    /// `reach = "embedded"` entries dropped.
    pub(super) app_daemon: usize,
}

impl RigsFile {
    pub(super) fn is_empty(&self) -> bool {
        self.rigs.is_empty()
    }
}

impl Serialize for RigsFile {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.rigs.serialize(s)
    }
}

impl<'de> Deserialize<'de> for RigsFile {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut out = Self::default();
        for v in Vec::<toml::Value>::deserialize(d)? {
            if v.get("reach").and_then(toml::Value::as_str) == Some("embedded") {
                tracing::warn!("ui.toml: a rig hosted inside the app dropped: the app has none");
                out.app_daemon += 1;
                continue;
            }
            match RigFile::deserialize(v) {
                Ok(r) => out.rigs.push(r),
                Err(e) => tracing::warn!("ui.toml: a remembered rig dropped: {e}"),
            }
        }
        Ok(out)
    }
}

/// `[daemon]`: each choice written only when off (the default is on).
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DaemonFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_local: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stop_on_quit: Option<bool>,
}

impl DaemonFile {
    pub(super) fn parse(f: Option<&Self>) -> DaemonPrefs {
        let d = DaemonPrefs::default();
        f.map_or(d, |f| DaemonPrefs {
            start_local: f.start_local.unwrap_or(d.start_local),
            stop_on_quit: f.stop_on_quit.unwrap_or(d.stop_on_quit),
        })
    }

    pub(super) fn of(p: DaemonPrefs) -> Option<Self> {
        (p != DaemonPrefs::default()).then_some(Self {
            start_local: (!p.start_local).then_some(false),
            stop_on_quit: (!p.stop_on_quit).then_some(false),
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
