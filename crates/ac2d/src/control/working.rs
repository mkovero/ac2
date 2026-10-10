//! The working state's session (`State.working`): which session the measurements and
//! traces belong to, whether they have unsaved changes, and the recovery copy of a working
//! state that `file.new` / `file.load` replaced (`docs/design/sessions.md`).
//!
//! Unsaved changes are a comparison of content hashes: the hash of the saved content when
//! the working state was last saved or opened, and the hash now. Equal content hashes the
//! same however it was reached, so an edit undone by hand is clean again. The hash covers
//! what a session holds that the operator edits — measurement configurations, delays, trace
//! metadata — and leaves out what moves by itself: running or stopped, a tracking delay's
//! applied value, the SPL logs (they grow every second).
//!
//! With an autosave, `working.json` beside its manifest keeps the origin and the saved
//! hash, so a restart restores "belongs to X, unsaved changes" with the content; the
//! recovery copy is a session directory `<autosave>.replaced` with its own `working.json`.
//! Without an autosave both live in memory only.

use std::fs;
use std::path::{Path, PathBuf};

use ac2_proto::event::Change;
use ac2_proto::model::{Recoverable, SessionOrigin, WorkingSession};
use ac2_proto::units::{ClientId, Seconds, WallNs};
use ac2_proto::{ErrorCode, ErrorDetail, ProtoError, ReplyBody};
use ac2_traces::session::{self, Session as SessionData};
use serde::{Deserialize, Serialize};

use super::Control;
use crate::util::{perr, perr_detail, wall_ns};

/// Beside an autosave's (or a recovery copy's) manifest.
pub(crate) const WORKING_FILE: &str = "working.json";

/// What the working state belongs to, as kept on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Persist {
    pub(crate) origin: Option<SessionOrigin>,
    /// Content hash at the last save or open.
    pub(crate) saved_hash: u64,
    pub(crate) saved_at: Option<WallNs>,
}

impl Persist {
    /// Untitled: clean while empty.
    pub(crate) fn untitled() -> Self {
        Self {
            origin: None,
            saved_hash: hash_of(&[], &[]),
            saved_at: None,
        }
    }
}

/// A recovery copy's `working.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplacedFile {
    working: Persist,
    replaced_at: WallNs,
}

/// Where the recovery copy is.
#[derive(Debug)]
enum Copy {
    Disk(PathBuf),
    Memory(Box<SessionData>),
}

/// The working state a `file.new` / `file.load` replaced while it had unsaved changes.
#[derive(Debug)]
pub(crate) struct Recover {
    info: Recoverable,
    working: Persist,
    copy: Copy,
}

/// FNV-1a, 64 bits: fixed across builds, so a hash written before a restart compares
/// with one computed after it.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Hash of the edited content (module docs).
fn hash_of(
    measurements: &[session::SavedMeasurement],
    traces: &[ac2_proto::model::TraceMeta],
) -> u64 {
    let ms: Vec<session::SavedMeasurement> = measurements
        .iter()
        .map(|m| {
            let mut m = m.clone();
            m.running = false;
            if let Some(d) = m.delay.as_mut()
                && d.tracking
            {
                d.applied = Seconds(0.0);
            }
            m
        })
        .collect();
    match serde_json::to_vec(&(ms, traces)) {
        Ok(b) => fnv1a(&b),
        // Unencodable content (a NaN) never matches a saved hash: shown as unsaved.
        Err(_) => u64::MAX,
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(p: &Path) -> Option<T> {
    let b = fs::read(p).ok()?;
    match serde_json::from_slice(&b) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!("{}: not read: {e}", p.display());
            None
        }
    }
}

fn write_json<T: Serialize>(p: &Path, v: &T) -> Result<(), String> {
    let b = serde_json::to_vec_pretty(v).map_err(|e| e.to_string())?;
    if let Some(d) = p.parent() {
        fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    session::write_atomic(p, &b).map_err(|e| e.to_string())
}

fn remove_dir(p: &Path) {
    if let Err(e) = fs::remove_dir_all(p)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("{}: not removed: {e}", p.display());
    }
}

/// `<autosave>.replaced`.
fn replaced_dir(autosave: &Path) -> PathBuf {
    let name = autosave
        .file_name()
        .map_or_else(|| "autosave".into(), |n| n.to_string_lossy().into_owned());
    autosave.with_file_name(format!("{name}.replaced"))
}

/// How an origin reads in messages.
pub(crate) fn origin_label(o: Option<&SessionOrigin>) -> String {
    match o {
        None => "the untitled session".into(),
        Some(SessionOrigin::Name { name }) => format!("session {name:?}"),
        Some(SessionOrigin::Path { path } | SessionOrigin::Client { path }) => {
            format!("session {path:?}")
        }
    }
}

impl Control {
    fn autosave_dir(&self) -> Option<PathBuf> {
        self.autosave.as_ref().map(|a| a.dir.clone())
    }

    /// Hash of the working state's edited content now.
    pub(super) fn content_hash(&self) -> u64 {
        hash_of(&self.saved_measurements(), &self.store.state().traces)
    }

    /// Recomputes `working` after a change to the measurements or traces.
    pub(super) fn working_changed(&mut self) {
        let w = WorkingSession {
            origin: self.working.origin.clone(),
            dirty: self.content_hash() != self.working.saved_hash,
            saved_at: self.working.saved_at,
            recover: self.recover.as_ref().map(|r| r.info.clone()),
        };
        if self.store.state().working != w {
            self.commit(Change::Working(w));
        }
    }

    /// The working state now belongs to `p`.
    pub(super) fn set_working(&mut self, p: Persist) {
        self.working = p;
        if let Some(dir) = self.autosave_dir() {
            let f = dir.join(WORKING_FILE);
            // No file reads back as untitled: an empty start leaves no autosave behind.
            let nothing = self.working == Persist::untitled() && !f.exists();
            if !nothing && let Err(e) = write_json(&f, &self.working) {
                tracing::warn!("{}: not written: {e}", f.display());
            }
        }
        self.working_changed();
    }

    /// The working state was saved to, or opened from, `origin` (saved at `saved_at`), and
    /// is what it holds now.
    pub(super) fn mark_saved(&mut self, origin: Option<SessionOrigin>, saved_at: WallNs) {
        let p = Persist {
            origin,
            saved_hash: self.content_hash(),
            saved_at: Some(saved_at),
        };
        self.set_working(p);
    }

    fn is_dirty(&self) -> bool {
        self.content_hash() != self.working.saved_hash
    }

    /// Refuses to replace unsaved changes unless the client said to discard them.
    pub(super) fn guard_unsaved(&self, discard: bool) -> Result<(), ProtoError> {
        if discard || !self.is_dirty() {
            return Ok(());
        }
        Err(perr_detail(
            ErrorCode::Conflict,
            format!(
                "{} has unsaved changes: save it first, or discard them",
                origin_label(self.working.origin.as_ref())
            ),
            ErrorDetail::UnsavedChanges {
                origin: self.working.origin.clone(),
            },
        ))
    }

    /// Keeps the working state as the recovery copy when it has unsaved changes (it is
    /// about to be replaced). On disk beside the autosave when there is one; in memory when
    /// there is none or the disk refuses.
    pub(super) fn keep_replaced(&mut self) -> Result<(), ProtoError> {
        if !self.is_dirty() {
            return Ok(());
        }
        let data = self.session_data(true)?;
        let st = self.store.state();
        let info = Recoverable {
            replaced_at: WallNs(wall_ns()),
            origin: self.working.origin.clone(),
            measurements: u32::try_from(st.measurements.len()).unwrap_or(u32::MAX),
            traces: u32::try_from(st.traces.len()).unwrap_or(u32::MAX),
        };
        let working = self.working.clone();
        let copy = match self.autosave_dir() {
            Some(a) => {
                let dir = replaced_dir(&a);
                match write_replaced(&dir, &data, &working, info.replaced_at) {
                    Ok(()) => Copy::Disk(dir),
                    Err(e) => {
                        tracing::warn!(
                            "the replaced working state is kept in memory only: {}: {e}",
                            dir.display()
                        );
                        Copy::Memory(Box::new(data))
                    }
                }
            }
            None => Copy::Memory(Box::new(data)),
        };
        tracing::info!(
            "kept {} ({} measurements, {} traces) for file.recover",
            origin_label(info.origin.as_ref()),
            info.measurements,
            info.traces
        );
        self.recover = Some(Recover {
            info,
            working,
            copy,
        });
        Ok(())
    }

    /// `file.new`.
    pub(super) fn file_new(
        &mut self,
        client: &ClientId,
        discard: bool,
    ) -> Result<ReplyBody, ProtoError> {
        self.guard_unsaved(discard)?;
        self.keep_replaced()?;
        let empty = SessionData {
            saved_at: WallNs(wall_ns()),
            measurements: Vec::new(),
            spl_logs: Vec::new(),
            traces: Vec::new(),
        };
        self.replace_session(Some(client), empty)?;
        self.set_working(Persist::untitled());
        tracing::info!("new untitled session");
        Ok(ReplyBody::Ack {
            rev: self.store.rev(),
        })
    }

    /// `file.recover`: swaps the working state with the recovery copy.
    pub(super) fn file_recover(&mut self, client: &ClientId) -> Result<ReplyBody, ProtoError> {
        let Some(r) = self.recover.take() else {
            return Err(perr(
                ErrorCode::NotFound,
                "no replaced working state to recover",
            ));
        };
        let data = match &r.copy {
            Copy::Memory(d) => Ok(d.as_ref().clone()),
            Copy::Disk(dir) => session::load(dir).map_err(|e| e.to_string()),
        };
        let data = match data {
            Ok(d) => d,
            Err(e) => {
                let msg = format!("the replaced working state cannot be read: {e}");
                self.recover = Some(r);
                return Err(perr(ErrorCode::Internal, msg));
            }
        };
        // From here the copy is the data read: the disk copy is overwritten next with the
        // state it replaces (when that has unsaved changes).
        let r = Recover {
            copy: Copy::Memory(Box::new(data.clone())),
            ..r
        };
        if let Err(e) = self.keep_replaced() {
            self.recover = Some(r);
            return Err(e);
        }
        let kept = self.recover.take();
        if let Err(e) = self.replace_session(Some(client), data) {
            // Nothing changed but the copy's place: it stays recoverable from memory.
            self.recover = Some(r);
            return Err(e);
        }
        self.recover = kept;
        if self.recover.is_none()
            && let Some(a) = self.autosave_dir()
        {
            remove_dir(&replaced_dir(&a));
        }
        tracing::info!("recovered {}", origin_label(r.working.origin.as_ref()));
        self.set_working(r.working);
        Ok(ReplyBody::Ack {
            rev: self.store.rev(),
        })
    }

    /// At start, after the autosave was (or was not) restored: what the working state
    /// belongs to, and the recovery copy on disk.
    pub(super) fn start_working(&mut self, restored: bool) {
        let Some(dir) = self.autosave_dir() else {
            self.working_changed();
            return;
        };
        let p = if restored {
            read_json::<Persist>(&dir.join(WORKING_FILE)).unwrap_or_else(Persist::untitled)
        } else {
            Persist::untitled()
        };
        let rd = replaced_dir(&dir);
        if rd.join(session::MANIFEST).is_file()
            && let Some(f) = read_json::<ReplacedFile>(&rd.join(WORKING_FILE))
            && let Ok(m) = session::read_manifest(&rd)
        {
            self.recover = Some(Recover {
                info: Recoverable {
                    replaced_at: f.replaced_at,
                    origin: f.working.origin.clone(),
                    measurements: u32::try_from(m.measurements.len()).unwrap_or(u32::MAX),
                    traces: u32::try_from(m.traces.len()).unwrap_or(u32::MAX),
                },
                working: f.working,
                copy: Copy::Disk(rd),
            });
        }
        self.set_working(p);
    }
}

/// Writes the recovery copy to `dir` whole (beside it first, then renamed into place).
fn write_replaced(
    dir: &Path,
    data: &SessionData,
    working: &Persist,
    replaced_at: WallNs,
) -> Result<(), String> {
    let name = dir
        .file_name()
        .map_or_else(|| "replaced".into(), |n| n.to_string_lossy().into_owned());
    let tmp = dir.with_file_name(format!("{name}.tmp"));
    remove_dir(&tmp);
    session::save(&tmp, data).map_err(|e| e.to_string())?;
    write_json(
        &tmp.join(WORKING_FILE),
        &ReplacedFile {
            working: working.clone(),
            replaced_at,
        },
    )?;
    remove_dir(dir);
    fs::rename(&tmp, dir).map_err(|e| format!("{}: {e}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untitled_is_the_empty_hash_and_fixed() {
        assert_eq!(Persist::untitled().saved_hash, hash_of(&[], &[]));
        // A fixed algorithm: a hash written before a restart compares after it.
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
