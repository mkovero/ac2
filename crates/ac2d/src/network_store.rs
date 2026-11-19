//! The network setting's own file, `<config>/network.json`: whether the network is on, where
//! it binds and its port. Apart from `rig.json` so that a change of either never sets aside
//! the other: a rig whose network setting is lost starts local only, which an operator on
//! the rig fixes in a moment, while the meters and the max level must survive.
//!
//! A missing file, a damaged one and one of another version all mean off: turning the
//! network on is the operator's explicit step, never a guess from a file that cannot be
//! read. A damaged or other-version file is set aside with a damage notice.

use std::io;
use std::path::PathBuf;

use ac2_proto::model::{NetBind, NetSaved, NotSavedReason, StateFile};
use serde::{Deserialize, Serialize};

use crate::damage::Damage;

/// The file's version; any change of its shape bumps it.
pub(crate) const VERSION: u32 = 1;

/// The network setting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NetSettings {
    pub(crate) enabled: bool,
    pub(crate) bind: NetBind,
    pub(crate) port: u16,
}

impl Default for NetSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: NetBind::All,
            port: crate::config::DEFAULT_PORT,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    version: u32,
    #[serde(flatten)]
    settings: NetSettings,
}

#[derive(Deserialize)]
struct Version {
    version: u32,
}

/// Where the setting is kept, and whether the last write went through.
#[derive(Debug)]
pub(crate) struct NetStore {
    path: Option<PathBuf>,
    saved: NetSaved,
}

impl NetStore {
    /// Reads `path` (`None`: kept in memory only). What it holds when it cannot be used is
    /// off; a damaged or other-version file is set aside.
    pub(crate) fn load(path: Option<PathBuf>) -> (Self, NetSettings, Option<Damage>) {
        let Some(path) = path else {
            let store = Self {
                path: None,
                saved: NetSaved::NotSaved {
                    reason: NotSavedReason::NoStore,
                },
            };
            return (store, NetSettings::default(), None);
        };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return (Self::saved(path), NetSettings::default(), None);
            }
            Err(e) => {
                tracing::error!(
                    "{}: cannot read the network setting ({}); the network starts off and \
                     changes are not kept",
                    path.display(),
                    crate::util::io_reason(&e)
                );
                let store = Self {
                    path: Some(path),
                    saved: NetSaved::NotSaved {
                        reason: NotSavedReason::StoreReadOnly,
                    },
                };
                return (store, NetSettings::default(), None);
            }
        };
        match parse(&bytes) {
            Ok(s) => (Self::saved(path), s, None),
            Err(reason) => {
                let set_aside_as = match ac2_paths::set_aside_damaged(&path) {
                    Ok(to) => Some(to),
                    Err(e) => {
                        tracing::error!(
                            "{}: cannot set the damaged network setting aside ({})",
                            path.display(),
                            crate::util::io_reason(&e)
                        );
                        None
                    }
                };
                let damage = Damage {
                    kind: StateFile::NetworkSettings,
                    path: path.clone(),
                    set_aside_as,
                    reason: format!("{reason}; the network starts off"),
                };
                damage.log();
                (Self::saved(path), NetSettings::default(), Some(damage))
            }
        }
    }

    fn saved(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            saved: NetSaved::Saved,
        }
    }

    /// Whether the setting in force outlives a restart.
    pub(crate) fn saved_state(&self) -> NetSaved {
        self.saved.clone()
    }

    /// Writes `s`. A failure is kept in [`Self::saved_state`] and never refuses the change:
    /// the network in force matters now, the file only at the next start.
    pub(crate) fn save(&mut self, s: &NetSettings) {
        let Some(path) = &self.path else {
            return;
        };
        let file = File {
            version: VERSION,
            settings: s.clone(),
        };
        let r = serde_json::to_vec_pretty(&file)
            .map_err(io::Error::other)
            .and_then(|b| {
                if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                    std::fs::create_dir_all(dir)?;
                }
                ac2_paths::write_atomic(path, &b)
            });
        self.saved = match r {
            Ok(()) => NetSaved::Saved,
            Err(e) => {
                tracing::error!(
                    "{}: the network setting is not kept ({})",
                    path.display(),
                    crate::util::io_reason(&e)
                );
                NetSaved::NotSaved {
                    reason: match e.kind() {
                        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => {
                            NotSavedReason::StoreReadOnly
                        }
                        _ => NotSavedReason::WriteFailed {
                            detail: crate::util::io_reason(&e),
                        },
                    },
                }
            }
        };
    }
}

fn parse(bytes: &[u8]) -> Result<NetSettings, String> {
    let v: Version =
        serde_json::from_slice(bytes).map_err(|e| format!("not a network setting: {e}"))?;
    if v.version != VERSION {
        return Err(format!(
            "network setting version {} (this ac2d reads {VERSION})",
            v.version
        ));
    }
    let f: File =
        serde_json::from_slice(bytes).map_err(|e| format!("not a network setting: {e}"))?;
    if f.settings.port == 0 || f.settings.port == u16::MAX {
        return Err(format!("port {} cannot be served", f.settings.port));
    }
    Ok(f.settings)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn round_trips_and_refuses_what_it_cannot_serve() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("network.json");
        let (mut store, s, d) = NetStore::load(Some(path.clone()));
        assert_eq!((s, d), (NetSettings::default(), None));
        let on = NetSettings {
            enabled: true,
            bind: NetBind::Addr {
                addr: "10.0.0.2".parse().unwrap(),
            },
            port: 5000,
        };
        store.save(&on);
        assert_eq!(store.saved_state(), NetSaved::Saved);
        assert_eq!(NetStore::load(Some(path.clone())).1, on);
        std::fs::write(
            &path,
            br#"{"version":1,"enabled":true,"bind":{"type":"all"},"port":0}"#,
        )
        .unwrap();
        let (_, s, d) = NetStore::load(Some(path.clone()));
        assert!(!s.enabled);
        assert!(d.unwrap().set_aside_as.is_some());
        assert!(!path.exists());
    }
}
