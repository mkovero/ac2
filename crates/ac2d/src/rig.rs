//! The rig's settings the daemon keeps across restarts: the system max level (the
//! generator ceiling) and the output labels. They belong to the rig, not to a session: a
//! loaded session must never raise the level limit, and the wiring outlives any session.
//!
//! One small JSON file in the daemon's config directory, written atomically (temporary file,
//! synced, renamed). Like the calibration store, a file that cannot be read is never
//! written: the daemon starts at its `--max-level` bound with no labels, and every change is
//! refused with the reason, so a lowered limit is never silently replaced. A file of another
//! format version is set aside (renamed, never deleted).

use std::io;
use std::path::{Path, PathBuf};

use ac2_proto::model::OutputSetup;
use ac2_proto::{ErrorCode, ProtoError};
use serde::{Deserialize, Serialize};

use crate::util::perr;

const FORMAT: &str = "ac2-rig-settings";
const VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RigFile {
    format: String,
    version: u32,
    /// The system max level last set, dBFS RMS; `None`: never set (the bound applies).
    ceiling_dbfs: Option<f64>,
    /// Labelled outputs, sorted by channel.
    outputs: Vec<OutputSetup>,
}

/// Just enough of any file to tell its format and version.
#[derive(Debug, Deserialize)]
struct Header {
    format: String,
    version: u32,
}

/// What the file holds.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct RigSettings {
    /// The system max level last set; `None`: never set.
    pub(crate) ceiling_dbfs: Option<f64>,
    /// Labelled outputs, sorted by channel.
    pub(crate) outputs: Vec<OutputSetup>,
}

/// Where the rig settings live.
#[derive(Debug)]
pub(crate) struct RigStore {
    path: Option<PathBuf>,
    /// Why the file could not be read; the store is then read-only.
    unreadable: Option<String>,
}

impl RigStore {
    /// Settings kept in memory only (tests, embedded daemons without a config directory).
    pub(crate) fn memory() -> Self {
        Self {
            path: None,
            unreadable: None,
        }
    }

    /// Reads `path`. Missing → nothing set. Another format version → set aside, nothing
    /// set. Unreadable → nothing set and read-only.
    pub(crate) fn open(path: &Path) -> (Self, RigSettings) {
        let mut store = Self {
            path: Some(path.to_owned()),
            unreadable: None,
        };
        let text = match std::fs::read(path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return (store, RigSettings::default());
            }
            Err(e) => {
                store.refuse(format!("cannot read it: {e}"));
                return (store, RigSettings::default());
            }
        };
        if let Ok(h) = serde_json::from_slice::<Header>(&text)
            && h.format == FORMAT
            && h.version != VERSION
        {
            match set_aside(path, &format!("v{}", h.version)) {
                Some(to) => {
                    tracing::warn!(
                        "rig settings {} are format version {} (this ac2d reads {VERSION}); set \
                         aside as {} — the system max level starts at the --max-level bound \
                         and the output labels are empty",
                        path.display(),
                        h.version,
                        to.display()
                    );
                }
                None => store.refuse(format!(
                    "format version {} (this ac2d reads {VERSION}) and it cannot be renamed out \
                     of the way",
                    h.version
                )),
            }
            return (store, RigSettings::default());
        }
        match serde_json::from_slice::<RigFile>(&text) {
            Ok(f) if f.format == FORMAT => {
                let s = RigSettings {
                    ceiling_dbfs: f.ceiling_dbfs,
                    outputs: f.outputs,
                };
                match check(&s) {
                    Ok(()) => (store, s),
                    Err(reason) => {
                        store.refuse(reason);
                        (store, RigSettings::default())
                    }
                }
            }
            Ok(f) => {
                store.refuse(format!("not a rig settings file (format {:?})", f.format));
                (store, RigSettings::default())
            }
            Err(e) => {
                store.refuse(e.to_string());
                (store, RigSettings::default())
            }
        }
    }

    fn refuse(&mut self, reason: String) {
        if let Some(p) = &self.path {
            tracing::error!(
                "rig settings {} are unreadable ({reason}); they will not be written — the \
                 system max level is the --max-level bound until the file is fixed or moved \
                 away and ac2d restarted",
                p.display()
            );
        }
        self.unreadable = Some(reason);
    }

    /// Ok unless the file could not be read at start.
    pub(crate) fn check(&self) -> Result<(), ProtoError> {
        match (&self.unreadable, &self.path) {
            (Some(reason), Some(path)) => Err(perr(
                ErrorCode::Refused,
                format!(
                    "rig settings {} are unreadable ({reason}); they are never overwritten — \
                     fix or move the file away and restart ac2d",
                    path.display()
                ),
            )),
            _ => Ok(()),
        }
    }

    /// Writes `s` atomically. Nothing is written (and the caller changes nothing) when the
    /// store is read-only or the write fails.
    pub(crate) fn persist(&self, s: &RigSettings) -> Result<(), ProtoError> {
        self.check()?;
        let Some(path) = &self.path else {
            return Ok(());
        };
        let file = RigFile {
            format: FORMAT.into(),
            version: VERSION,
            ceiling_dbfs: s.ceiling_dbfs,
            outputs: s.outputs.clone(),
        };
        let text = serde_json::to_vec_pretty(&file)
            .map_err(|e| perr(ErrorCode::Internal, format!("rig settings: {e}")))?;
        ac2_paths::write_private_atomic(path, &text).map_err(|e| {
            perr(
                ErrorCode::Internal,
                format!("cannot write rig settings {}: {e}", path.display()),
            )
        })
    }
}

/// A file a person edited may hold anything: the same rules as the commands.
fn check(s: &RigSettings) -> Result<(), String> {
    if let Some(c) = s.ceiling_dbfs
        && !(c.is_finite() && c <= 0.0)
    {
        return Err(format!(
            "ceiling_dbfs {c} is not a level at or below 0 dBFS"
        ));
    }
    for (i, o) in s.outputs.iter().enumerate() {
        if s.outputs[..i].iter().any(|x| x.channel == o.channel) {
            return Err(format!("output {} is listed twice", o.channel + 1));
        }
        match &o.label {
            Some(l) => ac2_proto::model::check_output_label(l)
                .map_err(|e| format!("output {}: {e}", o.channel + 1))?,
            None => return Err(format!("output {} has no label", o.channel + 1)),
        }
    }
    Ok(())
}

/// Renames `p` to `<p>.<tag>` (the time appended if that exists). Returns where it went.
fn set_aside(p: &Path, tag: &str) -> Option<PathBuf> {
    let name = p.file_name()?.to_string_lossy().into_owned();
    let mut to = p.with_file_name(format!("{name}.{tag}"));
    if to.exists() {
        to = p.with_file_name(format!(
            "{name}.{tag}-{}",
            crate::util::wall_ns() / 1_000_000_000
        ));
    }
    std::fs::rename(p, &to).ok().map(|()| to)
}

/// The system max level the daemon starts with: the one last set, never above `bound`
/// (a later, lower `--max-level` wins), else `bound`.
pub(crate) fn start_ceiling(saved: Option<f64>, bound: f64) -> f64 {
    saved.map_or(bound, |c| c.min(bound))
}

/// `current` with `rows` applied by channel (a `None` label removes the row), sorted.
pub(crate) fn upsert_outputs(current: &[OutputSetup], rows: &[OutputSetup]) -> Vec<OutputSetup> {
    let mut out: Vec<OutputSetup> = current
        .iter()
        .filter(|c| rows.iter().all(|r| r.channel != c.channel))
        .cloned()
        .collect();
    out.extend(rows.iter().filter(|r| r.label.is_some()).cloned());
    out.sort_by_key(|o| o.channel);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(channel: u16, l: &str) -> OutputSetup {
        OutputSetup {
            channel,
            label: Some(l.into()),
        }
    }

    #[test]
    fn roundtrip_missing_and_bound() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("rig.json");
        let (store, s) = RigStore::open(&p);
        assert_eq!(s, RigSettings::default());
        let want = RigSettings {
            ceiling_dbfs: Some(-40.0),
            outputs: vec![label(0, "Main L"), label(3, "Sub")],
        };
        store.persist(&want).expect("write");
        let (_, got) = RigStore::open(&p);
        assert_eq!(got, want);
        assert_eq!(start_ceiling(got.ceiling_dbfs, -10.0), -40.0);
        // A lower bound given at start wins over the value kept.
        assert_eq!(start_ceiling(got.ceiling_dbfs, -50.0), -50.0);
        assert_eq!(start_ceiling(None, -10.0), -10.0);
    }

    #[test]
    fn unreadable_is_never_written_and_other_versions_are_set_aside() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("rig.json");
        std::fs::write(&p, "garbage").expect("write");
        let (store, s) = RigStore::open(&p);
        assert_eq!(s, RigSettings::default());
        assert!(store.persist(&RigSettings::default()).is_err());
        assert_eq!(std::fs::read_to_string(&p).expect("read"), "garbage");

        std::fs::write(
            &p,
            r#"{"format":"ac2-rig-settings","version":99,"ceiling_dbfs":-3.0,"outputs":[]}"#,
        )
        .expect("write");
        let (store, s) = RigStore::open(&p);
        assert_eq!(s, RigSettings::default());
        assert!(!p.exists());
        assert!(dir.path().join("rig.json.v99").exists());
        store.persist(&RigSettings::default()).expect("writable");

        // Values a command would refuse make the file unreadable, not half-applied.
        std::fs::write(
            &p,
            r#"{"format":"ac2-rig-settings","version":1,"ceiling_dbfs":3.0,"outputs":[]}"#,
        )
        .expect("write");
        let (store, s) = RigStore::open(&p);
        assert_eq!(s, RigSettings::default());
        assert!(store.check().is_err());
    }

    #[test]
    fn upsert_sets_replaces_and_clears() {
        let cur = vec![label(0, "Main L"), label(2, "Fill")];
        let got = upsert_outputs(
            &cur,
            &[
                label(1, "Main R"),
                OutputSetup {
                    channel: 2,
                    label: None,
                },
                label(0, "Left"),
            ],
        );
        assert_eq!(got, vec![label(0, "Left"), label(1, "Main R")]);
    }
}
