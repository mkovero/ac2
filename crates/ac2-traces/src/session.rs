//! Session directories: measurements and traces saved by `file.save`, read by `file.load`.
//!
//! ```text
//! <dir>/session.json                 manifest (format, version, measurements, trace metadata)
//! <dir>/traces/<generation>-<id>.csv one ac2 CSV per trace (columns; header repeats metadata)
//! ```
//!
//! The manifest names the trace files it belongs to, and every save writes its trace files
//! under a new generation before it replaces the manifest (write to a temporary file, then
//! rename). A reader therefore sees either the old session or the new one, never a mix,
//! even if a save is interrupted; files of older generations are removed after the
//! manifest is in place.
//!
//! The manifest carries `format: "ac2-session"` and `version`. A file of another version is
//! refused with that version named — there is no migration and no best-effort read.
//!
//! What a session holds: measurement configurations (with their applied delay, tracking,
//! running and frozen flags) and stored traces with all metadata, display edits and slots.
//! Trace columns are saved unsmoothed; a trace's display smoothing is one of its edits
//! (`edit.smoothing`), applied again when the loaded trace is served.
//! It never holds generator state: a loaded session is always disarmed with no owner.
//! Calibrations are the calibration store's, not the session's.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use ac2_proto::GridDef;
use ac2_proto::model::{ImportFormat, ImportRole, MeasConfig, TraceMeta};
use ac2_proto::units::{MeasId, Seconds, WallNs};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::columns::StoredTrace;
use crate::text::{export_csv, import};

/// `format` of every manifest.
pub const FORMAT: &str = "ac2-session";
/// The one manifest version this build reads and writes.
pub const VERSION: u32 = 2;
/// Manifest file name.
pub const MANIFEST: &str = "session.json";
const TRACE_DIR: &str = "traces";

/// A measurement as saved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedMeasurement {
    /// Id at save time.
    pub id: MeasId,
    /// Configuration.
    pub config: MeasConfig,
    /// Running when saved (restarted on load when a session is open).
    pub running: bool,
    /// Frozen.
    pub frozen: bool,
    /// Transfer measurements: the applied delay and whether tracking was on.
    pub delay: Option<SavedDelay>,
}

/// Delay of a saved transfer measurement.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedDelay {
    /// Applied delay.
    pub applied: Seconds,
    /// Tracking on.
    pub tracking: bool,
}

/// A trace entry of the manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedTrace {
    /// Metadata.
    pub meta: TraceMeta,
    /// Grid of the data.
    pub grid: GridDef,
    /// Data file, relative to the session directory.
    pub file: String,
}

/// The manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Always [`FORMAT`].
    pub format: String,
    /// Always [`VERSION`].
    pub version: u32,
    /// When saved.
    pub saved_at: WallNs,
    /// Measurements.
    pub measurements: Vec<SavedMeasurement>,
    /// Traces.
    pub traces: Vec<SavedTrace>,
}

/// A whole session in memory.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    /// When saved.
    pub saved_at: WallNs,
    /// Measurements.
    pub measurements: Vec<SavedMeasurement>,
    /// Traces with data.
    pub traces: Vec<StoredTrace>,
}

/// Session file failures.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SessionError {
    /// No session at that path.
    #[error("no session at {0}")]
    NotFound(PathBuf),
    /// The directory holds something else.
    #[error("{0} is not an ac2 session")]
    NotASession(PathBuf),
    /// Another format version.
    #[error("{path}: session format version {found}; this build reads version {VERSION} only")]
    Version {
        /// Directory.
        path: PathBuf,
        /// Version in the manifest.
        found: u32,
    },
    /// An unusable session name.
    #[error(
        "session name {0:?}: use letters, digits, space, '-', '_' or '.', not starting with '.'"
    )]
    BadName(String),
    /// File system error.
    #[error("{path}: {msg}")]
    Io {
        /// Path.
        path: PathBuf,
        /// Error.
        msg: String,
    },
    /// Present but unreadable.
    #[error("{path}: damaged session: {msg}")]
    Corrupt {
        /// Path.
        path: PathBuf,
        /// What is wrong.
        msg: String,
    },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> SessionError + '_ {
    move |e| SessionError::Io {
        path: path.to_owned(),
        msg: e.to_string(),
    }
}

/// Checks a session name (one directory level under the session directory).
pub fn validate_name(name: &str) -> Result<(), SessionError> {
    let ok = !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name.trim() == name
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(SessionError::BadName(name.to_owned()))
    }
}

/// Writes `bytes` to `path` atomically: a temporary file in the same directory, synced,
/// then renamed over the target.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), SessionError> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    {
        let mut f = fs::File::create(&tmp).map_err(io(&tmp))?;
        f.write_all(bytes).map_err(io(&tmp))?;
        f.sync_all().map_err(io(&tmp))?;
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        SessionError::Io {
            path: path.to_owned(),
            msg: e.to_string(),
        }
    })
}

/// Saves `s` into `dir` (created if missing). Returns the manifest written.
pub fn save(dir: &Path, s: &Session) -> Result<Manifest, SessionError> {
    if dir.exists() && !dir.is_dir() {
        return Err(SessionError::NotASession(dir.to_owned()));
    }
    // Refuse to scatter files into a directory that holds something other than a session.
    if dir.is_dir() && !dir.join(MANIFEST).exists() {
        let has_files = fs::read_dir(dir).map_err(io(dir))?.next().is_some();
        if has_files {
            return Err(SessionError::NotASession(dir.to_owned()));
        }
    }
    let traces_dir = dir.join(TRACE_DIR);
    fs::create_dir_all(&traces_dir).map_err(io(&traces_dir))?;
    let generation = s.saved_at.0;
    let mut saved = Vec::with_capacity(s.traces.len());
    for t in &s.traces {
        let file = format!("{TRACE_DIR}/{generation}-{}.csv", t.meta.id.0);
        write_atomic(&dir.join(&file), export_csv(t).as_bytes())?;
        saved.push(SavedTrace {
            meta: t.meta.clone(),
            grid: t.grid.clone(),
            file,
        });
    }
    let m = Manifest {
        format: FORMAT.into(),
        version: VERSION,
        saved_at: s.saved_at,
        measurements: s.measurements.clone(),
        traces: saved,
    };
    let json = serde_json::to_vec_pretty(&m).map_err(|e| SessionError::Corrupt {
        path: dir.join(MANIFEST),
        msg: e.to_string(),
    })?;
    write_atomic(&dir.join(MANIFEST), &json)?;
    // Older generations are unreferenced now.
    let keep: Vec<String> = m.traces.iter().map(|t| t.file.clone()).collect();
    if let Ok(rd) = fs::read_dir(&traces_dir) {
        for e in rd.flatten() {
            let rel = format!("{TRACE_DIR}/{}", e.file_name().to_string_lossy());
            if !keep.contains(&rel) {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    if let Ok(d) = fs::File::open(dir) {
        // Persists the manifest rename on Unix; directories cannot be opened on Windows.
        let _ = d.sync_all();
    }
    Ok(m)
}

/// Reads only the manifest of `dir`, checking format and version first.
pub fn read_manifest(dir: &Path) -> Result<Manifest, SessionError> {
    let p = dir.join(MANIFEST);
    if !p.exists() {
        return Err(if dir.exists() {
            SessionError::NotASession(dir.to_owned())
        } else {
            SessionError::NotFound(dir.to_owned())
        });
    }
    let bytes = fs::read(&p).map_err(io(&p))?;
    let corrupt = |msg: String| SessionError::Corrupt {
        path: p.clone(),
        msg,
    };
    // Format and version are read loosely first, so a newer file gets "version N" rather
    // than a field-by-field decode error.
    let v: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| corrupt(e.to_string()))?;
    if v.get("format").and_then(|f| f.as_str()) != Some(FORMAT) {
        return Err(SessionError::NotASession(dir.to_owned()));
    }
    let found = v
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| corrupt("no version".into()))?;
    if found != u64::from(VERSION) {
        return Err(SessionError::Version {
            path: dir.to_owned(),
            found: u32::try_from(found).unwrap_or(u32::MAX),
        });
    }
    serde_json::from_value(v).map_err(|e| corrupt(e.to_string()))
}

/// Loads the session in `dir`.
pub fn load(dir: &Path) -> Result<Session, SessionError> {
    let m = read_manifest(dir)?;
    let mut traces = Vec::with_capacity(m.traces.len());
    for t in &m.traces {
        if t.file.contains("..") || Path::new(&t.file).is_absolute() {
            return Err(SessionError::Corrupt {
                path: dir.join(MANIFEST),
                msg: format!("trace file {:?} is outside the session", t.file),
            });
        }
        let p = dir.join(&t.file);
        let bytes = fs::read(&p).map_err(io(&p))?;
        let corrupt = |msg: String| SessionError::Corrupt {
            path: p.clone(),
            msg,
        };
        let imp = import(&bytes, ImportFormat::Ac2Csv, ImportRole::Trace)
            .map_err(|e| corrupt(e.to_string()))?;
        if imp.grid != t.grid || t.grid.id() != t.meta.grid_id {
            return Err(corrupt("data is not on the trace's grid".into()));
        }
        traces.push(StoredTrace {
            meta: t.meta.clone(),
            grid: t.grid.clone(),
            columns: imp.columns,
        });
    }
    Ok(Session {
        saved_at: m.saved_at,
        measurements: m.measurements,
        traces,
    })
}

/// Sessions directly under `root`, by name; directories without a readable manifest of
/// this version are skipped. A missing `root` lists nothing.
pub fn list(root: &Path) -> Result<Vec<(String, PathBuf, Manifest)>, SessionError> {
    let rd = match fs::read_dir(root) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io(root)(e)),
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        if let Ok(m) = read_manifest(&p) {
            out.push((e.file_name().to_string_lossy().into_owned(), p, m));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}
