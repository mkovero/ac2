//! State files found damaged at start. A standalone meter is power-cut every night: any file
//! may come back torn, zero-length or garbage, and the daemon must start and meter anyway.
//! A damaged file is set aside (`<name>.damaged-<time>`, never deleted), its backup copy
//! read instead where one is kept, logged as an error and listed in the rig entity
//! (`Rig::damaged_files`) for clients.

use std::path::{Path, PathBuf};

use ac2_proto::model::{DamagedFile, StateFile};
use ac2_proto::units::WallNs;

/// One damaged file and what was done about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Damage {
    pub(crate) kind: StateFile,
    pub(crate) path: PathBuf,
    pub(crate) set_aside_as: Option<PathBuf>,
    /// What was wrong, and what the daemon did instead.
    pub(crate) reason: String,
}

impl Damage {
    /// Logs it (an error: something the operator should know about the morning after).
    pub(crate) fn log(&self) {
        match &self.set_aside_as {
            Some(to) => tracing::error!(
                "damaged {}: {} ({}); set aside as {}",
                kind_name(self.kind),
                self.path.display(),
                self.reason,
                to.display()
            ),
            None => tracing::error!(
                "damaged {}: {} ({})",
                kind_name(self.kind),
                self.path.display(),
                self.reason
            ),
        }
    }

    pub(crate) fn wire(&self, at: WallNs) -> DamagedFile {
        DamagedFile {
            kind: self.kind,
            path: self.path.display().to_string(),
            set_aside_as: self.set_aside_as.as_ref().map(|p| p.display().to_string()),
            reason: self.reason.clone(),
            at,
        }
    }
}

fn kind_name(k: StateFile) -> &'static str {
    match k {
        StateFile::Autosave => "autosave",
        StateFile::Trace => "trace file",
        StateFile::SplLog => "SPL log",
        StateFile::BandLog => "band log",
        StateFile::Working => "working state",
        StateFile::RigSettings => "rig settings",
        StateFile::RigLogs => "rig logs",
        StateFile::Calibrations => "calibration store",
        StateFile::ServerKey => "server key",
        StateFile::AuthorizedClients => "authorized clients",
        StateFile::RigId => "rig id",
    }
}

/// The backup copy kept beside a small store (`rig.json` → `rig.json.prev`): the version
/// before the last write, read when the file itself is damaged.
pub(crate) fn prev_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".prev");
    PathBuf::from(s)
}

/// Writes `bytes` to `path` atomically, the file it replaces kept as [`prev_path`] first.
pub(crate) fn write_with_prev(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Ok(old) = std::fs::read(path)
        && old != bytes
    {
        ac2_paths::write_private_atomic(&prev_path(path), &old)?;
    }
    ac2_paths::write_private_atomic(path, bytes)
}

/// `path` cannot be used (`reason`): set aside, and its backup copy parsed instead when
/// there is one that parses. `Err` when it cannot even be moved (the caller then must not
/// overwrite it).
pub(crate) fn recover<T>(
    path: &Path,
    kind: StateFile,
    reason: &str,
    parse: impl Fn(&[u8]) -> Result<T, String>,
) -> Result<(Option<T>, Damage), String> {
    let to = ac2_paths::set_aside_damaged(path)
        .map_err(|e| format!("{reason}, and it cannot be set aside: {e}"))?;
    let prev = prev_path(path);
    let (value, what) = match std::fs::read(&prev)
        .map_err(|e| e.to_string())
        .and_then(|b| parse(&b))
    {
        Ok(v) => (
            Some(v),
            format!("read the previous copy {}", prev.display()),
        ),
        Err(_) => (None, "started without it".to_owned()),
    };
    let d = Damage {
        kind,
        path: path.to_owned(),
        set_aside_as: Some(to),
        reason: format!("{reason}; {what}"),
    };
    d.log();
    Ok((value, d))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(b: &[u8]) -> Result<u32, String> {
        std::str::from_utf8(b)
            .map_err(|e| e.to_string())?
            .trim()
            .parse()
            .map_err(|e: std::num::ParseIntError| e.to_string())
    }

    #[test]
    fn a_damaged_store_falls_back_to_its_previous_copy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("store.json");
        write_with_prev(&p, b"1").expect("first");
        write_with_prev(&p, b"2").expect("second");
        assert_eq!(std::fs::read(prev_path(&p)).expect("prev"), b"1");
        std::fs::write(&p, b"\0\0\0").expect("tear");
        let (v, d) = recover(&p, StateFile::RigSettings, "garbage", parse).expect("recover");
        assert_eq!(v, Some(1));
        assert!(d.reason.contains("previous copy"), "{}", d.reason);
        let aside = d.set_aside_as.expect("aside");
        assert_eq!(std::fs::read(aside).expect("kept"), b"\0\0\0");
        assert!(!p.exists());

        std::fs::write(&p, b"x").expect("garbage");
        std::fs::write(prev_path(&p), b"").expect("empty prev");
        let (v, d) = recover(&p, StateFile::RigSettings, "garbage", parse).expect("recover");
        assert_eq!(v, None);
        assert!(d.reason.ends_with("started without it"), "{}", d.reason);
    }
}
