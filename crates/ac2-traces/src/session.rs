//! Session directories: measurements and traces saved by `file.save`, read by `file.load`.
//!
//! ```text
//! <dir>/session.json                 manifest (format, version, measurements, trace metadata)
//! <dir>/session.prev.json            the autosave's previous manifest ([`save_autosave`])
//! <dir>/traces/<id>-<hash>.csv       one ac2 CSV per trace (columns; header repeats metadata)
//! <dir>/spl/<name>.csv               one per-second log per SPL meter ([`crate::spl_log`])
//! <dir>/spl/<name>.bands.csv         its band meter's per-second log ([`crate::band_log`])
//! ```
//!
//! A sweep trace's CSV holds all of it: distortion curves as columns, analysis facts in its
//! header and the impulse response as a second table ([`crate::text`]); a transfer trace
//! captured with an impulse response keeps it the same way. A mic curve applied
//! to a trace after capture keeps its points in the manifest (`mic_curve_points`), so the
//! trace reads the same after the calibration store changed.
//!
//! The manifest names the files it belongs to. A trace file is named by a hash of its
//! content, so it never changes once written: a save writes only the trace files that are
//! not there yet, then replaces the manifest (write to a temporary file, then rename). A
//! reader therefore sees either the old session or the new one, never a mix, even if a save
//! is interrupted; files no manifest names are removed after the manifest is in place.
//!
//! The autosave keeps its SPL logs as files that grow a line at a time (the daemon appends
//! to them between saves); its manifest names them and a save leaves them alone. A log's
//! last line may therefore be cut short by a power cut, which a load reads past
//! ([`spl_log::import_csv`]).
//!
//! The manifest carries `format: "ac2-session"` and `version`. A file of another version is
//! refused with that version named — there is no migration and no best-effort read.
//!
//! What a session holds: measurement configurations (with their applied delay, tracking
//! and running flags), each SPL meter's per-second log (so its Leq windows carry on
//! after a load or a daemon restart) and stored traces with all metadata, display edits
//! and slots.
//! Trace columns are saved unsmoothed and uncorrected; a trace's display smoothing
//! (`edit.smoothing`) and applied mic curve (`mic_curve`) are applied again when the loaded
//! trace is served.
//! It never holds generator state: a loaded session is always disarmed with no owner.
//! Calibrations are the calibration store's, not the session's.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use ac2_proto::GridDef;
use ac2_proto::model::{ImportFormat, ImportRole, MeasConfig, SplLogRow, TraceKind, TraceMeta};
use ac2_proto::units::{MeasId, Seconds, WallNs};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::band_log::{self, BandLogRow};
use crate::columns::StoredTrace;
use crate::spl_log::{self, SplLogInfo};
use crate::text::{export_csv, import};

/// `format` of every manifest.
pub const FORMAT: &str = "ac2-session";
/// The one manifest version this build reads and writes.
pub const VERSION: u32 = 19;
/// Manifest file name.
pub const MANIFEST: &str = "session.json";
/// The autosave's previous manifest, beside [`MANIFEST`].
pub const PREV_MANIFEST: &str = "session.prev.json";
const TRACE_DIR: &str = "traces";
/// Subdirectory of the SPL logs.
pub const SPL_DIR: &str = "spl";

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
    /// Transfer measurements: the applied delay and whether tracking was on.
    pub delay: Option<SavedDelay>,
}

/// Delay of a saved transfer measurement.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedDelay {
    /// Applied delay.
    pub applied: Seconds,
    /// What `delay.nudge` steps added to the arrival
    /// ([`ac2_proto::model::DelayState::nudged`]).
    pub nudged: Seconds,
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
    /// Points (Hz, dB) of the mic curve applied after capture (`meta.mic_curve`).
    pub mic_curve_points: Option<Vec<[f64; 2]>>,
}

/// An SPL log entry of the manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedSplLogFile {
    /// SPL measurement.
    pub meas: MeasId,
    /// Log file, relative to the session directory.
    pub file: String,
    /// The band meter's per-second log beside it ([`crate::band_log`]), if one was kept.
    pub bands: Option<String>,
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
    /// Per-second logs of the SPL meters.
    pub spl_logs: Vec<SavedSplLogFile>,
    /// Traces.
    pub traces: Vec<SavedTrace>,
}

/// One SPL meter's log in memory.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedSplLog {
    /// Measurement, name, input, mic (the file's header).
    pub info: SplLogInfo,
    /// Rows, oldest first.
    pub rows: Vec<SplLogRow>,
    /// The band meter's rows, oldest first (empty when it never ran).
    pub bands: Vec<BandLogRow>,
}

/// Where a loaded SPL log came from, for appending to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplLogOnDisk {
    /// SPL measurement.
    pub meas: MeasId,
    /// Log file, relative to the session directory.
    pub file: String,
    /// Rows read.
    pub rows: u64,
    /// Bytes up to the end of the last whole line.
    pub complete_len: u64,
    /// Where its band log was read from.
    pub bands: Option<BandLogOnDisk>,
}

/// Where a loaded band log came from, for appending to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BandLogOnDisk {
    /// Log file, relative to the session directory.
    pub file: String,
    /// Rows read.
    pub rows: u64,
    /// Bytes up to the end of the last whole line.
    pub complete_len: u64,
}

/// A whole session in memory.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    /// When saved.
    pub saved_at: WallNs,
    /// Measurements.
    pub measurements: Vec<SavedMeasurement>,
    /// SPL logs (of measurements in `measurements`).
    pub spl_logs: Vec<SavedSplLog>,
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

/// FNV-1a, 64 bits: names a trace file by its content. Only equal content must give equal
/// names (a collision among a session's few traces is out of reach at 64 bits), and the
/// algorithm is fixed so that names stay the same across builds.
fn content_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// How [`save_into`] treats the SPL logs and the manifest it replaces.
enum Mode<'a> {
    /// Every log written out whole; the old manifest goes.
    Whole,
    /// The logs are `linked` files kept up by the caller; the old manifest becomes
    /// [`PREV_MANIFEST`] and its files stay.
    Autosave { linked: &'a [SavedSplLogFile] },
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
    save_into(dir, s, &Mode::Whole)
}

/// Saves `s` into the autosave directory `dir` in place: the SPL logs are the `linked`
/// files the caller appends to (`s.spl_logs` are written whole as usual), the manifest
/// replaced becomes [`PREV_MANIFEST`], and only files neither manifest names are removed.
/// A save of what is already there writes the manifest and nothing else.
pub fn save_autosave(
    dir: &Path,
    s: &Session,
    linked: &[SavedSplLogFile],
) -> Result<Manifest, SessionError> {
    if dir.exists() && !dir.is_dir() {
        return Err(SessionError::NotASession(dir.to_owned()));
    }
    save_into(dir, s, &Mode::Autosave { linked })
}

fn save_into(dir: &Path, s: &Session, mode: &Mode<'_>) -> Result<Manifest, SessionError> {
    let traces_dir = dir.join(TRACE_DIR);
    fs::create_dir_all(&traces_dir).map_err(io(&traces_dir))?;
    let spl_dir = dir.join(SPL_DIR);
    fs::create_dir_all(&spl_dir).map_err(io(&spl_dir))?;
    let generation = s.saved_at.0;
    let mut logs = Vec::with_capacity(s.spl_logs.len());
    for l in &s.spl_logs {
        let file = format!("{SPL_DIR}/{generation}-{}.csv", l.info.meas.0);
        write_atomic(
            &dir.join(&file),
            spl_log::export_csv(&l.info, &l.rows).as_bytes(),
        )?;
        let bands = if l.bands.is_empty() {
            None
        } else {
            let file = format!("{SPL_DIR}/{generation}-{}.bands.csv", l.info.meas.0);
            write_atomic(
                &dir.join(&file),
                band_log::export_csv(&l.info, &l.bands).as_bytes(),
            )?;
            Some(file)
        };
        logs.push(SavedSplLogFile {
            meas: l.info.meas,
            file,
            bands,
        });
    }
    if let Mode::Autosave { linked } = mode {
        logs.extend(linked.iter().cloned());
    }
    let mut saved = Vec::with_capacity(s.traces.len());
    for t in &s.traces {
        let csv = export_csv(t);
        let file = format!(
            "{TRACE_DIR}/{}-{:016x}.csv",
            t.meta.id.0,
            content_hash(csv.as_bytes())
        );
        let p = dir.join(&file);
        // A file of that name was renamed into place whole, with this content.
        if !p.is_file() {
            write_atomic(&p, csv.as_bytes())?;
        }
        saved.push(SavedTrace {
            meta: t.meta.clone(),
            grid: t.grid.clone(),
            file,
            mic_curve_points: t.mic_curve.as_ref().map(crate::mic::points),
        });
    }
    let m = Manifest {
        format: FORMAT.into(),
        version: VERSION,
        saved_at: s.saved_at,
        measurements: s.measurements.clone(),
        spl_logs: logs,
        traces: saved,
    };
    let json = serde_json::to_vec_pretty(&m).map_err(|e| SessionError::Corrupt {
        path: dir.join(MANIFEST),
        msg: e.to_string(),
    })?;
    let cur = dir.join(MANIFEST);
    let mut keep = files_of(&m);
    if let Mode::Autosave { .. } = mode {
        let prev = dir.join(PREV_MANIFEST);
        // A rename, not a copy: the previous manifest is not written again. Should the
        // write below not complete, the previous one is what a restore finds.
        if cur.is_file() {
            fs::rename(&cur, &prev).map_err(io(&prev))?;
        }
        if let Ok(pm) = read_manifest_named(dir, PREV_MANIFEST) {
            keep.extend(files_of(&pm));
        }
    }
    write_atomic(&cur, &json)?;
    for (sub, d) in [(TRACE_DIR, &traces_dir), (SPL_DIR, &spl_dir)] {
        if let Ok(rd) = fs::read_dir(d) {
            for e in rd.flatten() {
                let rel = format!("{sub}/{}", e.file_name().to_string_lossy());
                if !keep.contains(&rel) {
                    let _ = fs::remove_file(e.path());
                }
            }
        }
    }
    if let Ok(d) = fs::File::open(dir) {
        // Persists the manifest rename on Unix; directories cannot be opened on Windows.
        let _ = d.sync_all();
    }
    Ok(m)
}

/// The files a manifest names.
fn files_of(m: &Manifest) -> Vec<String> {
    m.traces
        .iter()
        .map(|t| t.file.clone())
        .chain(m.spl_logs.iter().map(|l| l.file.clone()))
        .chain(m.spl_logs.iter().filter_map(|l| l.bands.clone()))
        .collect()
}

/// Reads only the manifest of `dir`, checking format and version first.
pub fn read_manifest(dir: &Path) -> Result<Manifest, SessionError> {
    read_manifest_named(dir, MANIFEST)
}

/// Reads the manifest `name` ([`MANIFEST`] or [`PREV_MANIFEST`]) of `dir`, checking format
/// and version first.
pub fn read_manifest_named(dir: &Path, name: &str) -> Result<Manifest, SessionError> {
    let p = dir.join(name);
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
    load_named(dir, MANIFEST).map(|(s, _)| s)
}

/// Loads the session the manifest `name` of `dir` describes, with where each SPL log was
/// read from.
pub fn load_named(dir: &Path, name: &str) -> Result<(Session, Vec<SplLogOnDisk>), SessionError> {
    let m = read_manifest_named(dir, name)?;
    let mut traces = Vec::with_capacity(m.traces.len());
    for t in &m.traces {
        let f = &t.file;
        if f.contains("..") || Path::new(f).is_absolute() {
            return Err(SessionError::Corrupt {
                path: dir.join(name),
                msg: format!("trace file {f:?} is outside the session"),
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
        if (t.meta.kind == TraceKind::Sweep) != imp.sweep.is_some() {
            return Err(corrupt(
                "a sweep trace without its distortion, analysis facts or impulse response".into(),
            ));
        }
        let mic_curve = match (&t.meta.mic_curve, &t.mic_curve_points) {
            (Some(mc), Some(p)) => {
                Some(
                    crate::mic::correction(p, mc.f_norm.0).map_err(|e| SessionError::Corrupt {
                        path: dir.join(name),
                        msg: format!("trace {}: mic curve: {e}", t.meta.id),
                    })?,
                )
            }
            (None, None) => None,
            _ => {
                return Err(SessionError::Corrupt {
                    path: dir.join(name),
                    msg: format!("trace {}: mic curve and its points disagree", t.meta.id),
                });
            }
        };
        traces.push(StoredTrace {
            meta: t.meta.clone(),
            grid: t.grid.clone(),
            columns: imp.columns,
            sweep: imp.sweep,
            ir: imp.ir,
            mic_curve,
        });
    }
    let mut spl_logs = Vec::with_capacity(m.spl_logs.len());
    let mut on_disk = Vec::with_capacity(m.spl_logs.len());
    for l in &m.spl_logs {
        let f = &l.file;
        if f.contains("..") || Path::new(f).is_absolute() {
            return Err(SessionError::Corrupt {
                path: dir.join(name),
                msg: format!("SPL log file {f:?} is outside the session"),
            });
        }
        let Some(sm) = m.measurements.iter().find(|sm| sm.id == l.meas) else {
            return Err(SessionError::Corrupt {
                path: dir.join(name),
                msg: format!(
                    "SPL log of measurement {} which is not in the session",
                    l.meas
                ),
            });
        };
        let ac2_proto::model::MeasKind::Spl { config } = &sm.config.kind else {
            return Err(SessionError::Corrupt {
                path: dir.join(name),
                msg: format!(
                    "SPL log of measurement {}, which is not an SPL meter",
                    l.meas
                ),
            });
        };
        let p = dir.join(f);
        let bytes = fs::read(&p).map_err(io(&p))?;
        let read = spl_log::import_csv(&bytes).map_err(|e| SessionError::Corrupt {
            path: p.clone(),
            msg: e.to_string(),
        })?;
        let (bands, bands_on_disk) = match &l.bands {
            None => (Vec::new(), None),
            Some(bf) => {
                if bf.contains("..") || Path::new(bf).is_absolute() {
                    return Err(SessionError::Corrupt {
                        path: dir.join(name),
                        msg: format!("band log file {bf:?} is outside the session"),
                    });
                }
                let bp = dir.join(bf);
                let bytes = fs::read(&bp).map_err(io(&bp))?;
                let read = band_log::import_csv(&bytes).map_err(|e| SessionError::Corrupt {
                    path: bp.clone(),
                    msg: e.to_string(),
                })?;
                let on = BandLogOnDisk {
                    file: bf.clone(),
                    rows: read.rows.len() as u64,
                    complete_len: read.complete_len as u64,
                };
                (read.rows, Some(on))
            }
        };
        on_disk.push(SplLogOnDisk {
            meas: l.meas,
            file: f.clone(),
            rows: read.rows.len() as u64,
            complete_len: read.complete_len as u64,
            bands: bands_on_disk,
        });
        spl_logs.push(SavedSplLog {
            info: SplLogInfo {
                meas: l.meas,
                name: sm.config.name.clone(),
                input: config.input,
                mic: None,
            },
            rows: read.rows,
            bands,
        });
    }
    Ok((
        Session {
            saved_at: m.saved_at,
            measurements: m.measurements,
            spl_logs,
            traces,
        },
        on_disk,
    ))
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
