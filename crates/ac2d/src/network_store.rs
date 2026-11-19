//! The network setting's own file, `<config>/network.json`: whether the network is on, and
//! where it binds and its port, kept while it is off for the next time it goes on. Apart
//! from `rig.json` so that a change of either never sets aside
//! the other: a rig whose network setting is lost starts local only, which an operator on
//! the rig fixes in a moment, while the meters and the max level must survive.
//!
//! A missing file, a damaged one and one of another version all mean off: turning the
//! network on is the operator's explicit step, never a guess from a file that cannot be
//! read. A damaged or other-version file is set aside with a damage notice. A file that
//! cannot be read at all is never written over: it may hold a good setting behind a
//! passing fault, which the next start can still read.

use std::io;
use std::path::PathBuf;

use ac2_proto::model::{NetBind, NetChoice, NetSaved, NotSavedReason, StateFile};
use serde::{Deserialize, Serialize};

use crate::config::Listen;
use crate::damage::Damage;

/// The file's version; any change of its shape bumps it.
pub(crate) const VERSION: u32 = 2;

/// The network setting.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NetSettings {
    pub(crate) enabled: bool,
    /// Where it is on, or was last on; `None` until it was first on.
    pub(crate) choice: Option<NetChoice>,
}

impl NetSettings {
    /// Where it is on, or would go on: the choice, else every interface at the default port.
    pub(crate) fn effective(&self) -> NetChoice {
        self.choice.clone().unwrap_or(NetChoice {
            bind: NetBind::All,
            port: crate::config::DEFAULT_PORT,
        })
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

/// Whether a change may be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Write {
    Ready,
    /// The damaged file is still in place: set it aside first.
    SetAsideFirst,
    /// The file could not be read: left alone for this run.
    Never,
}

/// Where the setting is kept, and whether the last write went through.
#[derive(Debug)]
pub(crate) struct NetStore {
    path: Option<PathBuf>,
    write: Write,
    saved: NetSaved,
}

impl NetStore {
    /// Reads `path` (`None`: kept in memory only), whose ports must not take the `local`
    /// endpoints'. What it holds when it cannot be used is off; a damaged or other-version
    /// file is set aside.
    pub(crate) fn load(
        path: Option<PathBuf>,
        local: &Listen,
    ) -> (Self, NetSettings, Option<Damage>) {
        let Some(path) = path else {
            let store = Self {
                path: None,
                write: Write::Never,
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
                let detail = crate::util::io_reason(&e);
                tracing::error!(
                    "{}: cannot read the network setting ({detail}); the network starts off \
                     and changes are not kept",
                    path.display(),
                );
                let store = Self {
                    path: Some(path),
                    write: Write::Never,
                    saved: NetSaved::NotSaved {
                        reason: NotSavedReason::Unreadable { detail },
                    },
                };
                return (store, NetSettings::default(), None);
            }
        };
        match parse(&bytes, local) {
            Ok(s) => (Self::saved(path), s, None),
            Err(reason) => {
                let set_aside_as = set_aside(&path);
                let store = if set_aside_as.is_some() {
                    Self::saved(path.clone())
                } else {
                    Self {
                        path: Some(path.clone()),
                        write: Write::SetAsideFirst,
                        saved: NetSaved::NotSaved {
                            reason: NotSavedReason::DamagedInPlace,
                        },
                    }
                };
                let damage = Damage {
                    kind: StateFile::NetworkSettings,
                    path: path.clone(),
                    set_aside_as,
                    reason: format!("{reason}; the network starts off"),
                };
                damage.log();
                (store, NetSettings::default(), Some(damage))
            }
        }
    }

    fn saved(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            write: Write::Ready,
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
        match self.write {
            Write::Never => return,
            Write::SetAsideFirst => {
                if set_aside(path).is_none() {
                    return;
                }
                self.write = Write::Ready;
            }
            Write::Ready => {}
        }
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

/// Moves a damaged file aside; where to, or `None` (logged) when it stays in place.
fn set_aside(path: &std::path::Path) -> Option<PathBuf> {
    match ac2_paths::set_aside_damaged(path) {
        Ok(to) => Some(to),
        Err(e) => {
            tracing::error!(
                "{}: cannot set the damaged network setting aside ({})",
                path.display(),
                crate::util::io_reason(&e)
            );
            None
        }
    }
}

/// The setting in the file, held to the rules `server.network` holds a new one to.
fn parse(bytes: &[u8], local: &Listen) -> Result<NetSettings, String> {
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
    if let Some(c) = &f.settings.choice {
        crate::network::check_port(c.port, local).map_err(|e| e.0)?;
        crate::network::check_bind(&c.bind).map_err(|e| e.0)?;
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
        let local = Listen::Local {
            ctrl: "tcp://127.0.0.1:6000".into(),
            data: "tcp://127.0.0.1:6001".into(),
        };
        let (mut store, s, d) = NetStore::load(Some(path.clone()), &local);
        assert_eq!((s, d), (NetSettings::default(), None));
        let on = NetSettings {
            enabled: true,
            choice: Some(NetChoice {
                bind: NetBind::Addr {
                    addr: "10.0.0.2".parse().unwrap(),
                },
                port: 5000,
            }),
        };
        store.save(&on);
        assert_eq!(store.saved_state(), NetSaved::Saved);
        assert_eq!(NetStore::load(Some(path.clone()), &local).1, on);
        // Off, the choice is kept for the next time it goes on.
        let off = NetSettings {
            enabled: false,
            ..on
        };
        store.save(&off);
        assert_eq!(NetStore::load(Some(path.clone()), &local).1, off);
        for bad in [
            r#"{"version":2,"enabled":true,"choice":{"bind":{"type":"all"},"port":0}}"#,
            r#"{"version":2,"enabled":false,"choice":{"bind":{"type":"all"},"port":5999}}"#,
            r#"{"version":2,"enabled":true,"choice":{"bind":{"type":"interface","name":"e\u0000"},"port":5000}}"#,
            r#"{"version":1,"enabled":true,"bind":{"type":"all"},"port":5000}"#,
        ] {
            std::fs::write(&path, bad).unwrap();
            let (_, s, d) = NetStore::load(Some(path.clone()), &local);
            assert!(!s.enabled, "{bad}");
            assert!(d.unwrap().set_aside_as.is_some(), "{bad}");
            assert!(!path.exists(), "{bad}");
        }
    }

    /// A file that cannot be read is never written over; one that could not be set aside
    /// is, once a later change manages to set it aside.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_file_is_left_and_a_stuck_damaged_one_set_aside_before_a_write() {
        use std::os::unix::fs::PermissionsExt;
        let local = Listen::Inproc {
            name: "store".into(),
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("network.json");
        std::fs::write(&path, b"held").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&path).is_ok() {
            return; // Permissions do not bind this user (root): nothing to test.
        }
        let (mut store, s, d) = NetStore::load(Some(path.clone()), &local);
        assert_eq!((s.enabled, d.is_none()), (false, true));
        store.save(&NetSettings {
            enabled: true,
            ..NetSettings::default()
        });
        assert!(matches!(
            store.saved_state(),
            NetSaved::NotSaved {
                reason: NotSavedReason::Unreadable { .. }
            }
        ));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"held", "never written over");

        // Damaged in a directory it cannot be moved out of: kept, then set aside first.
        std::fs::write(&path, b"{").unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let (mut store, _, d) = NetStore::load(Some(path.clone()), &local);
        assert!(d.unwrap().set_aside_as.is_none());
        assert_eq!(
            store.saved_state(),
            NetSaved::NotSaved {
                reason: NotSavedReason::DamagedInPlace
            }
        );
        let on = NetSettings {
            enabled: true,
            ..NetSettings::default()
        };
        store.save(&on);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"{",
            "not written over while stuck"
        );
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        store.save(&on);
        assert_eq!(store.saved_state(), NetSaved::Saved);
        assert_eq!(NetStore::load(Some(path.clone()), &local).1, on);
        let files = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(files, 2, "the damaged file set aside beside the new one");
    }
}
