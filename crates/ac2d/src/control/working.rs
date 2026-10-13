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
        let hash = self.content_hash();
        self.mark_saved_as(origin, saved_at, hash);
    }

    /// [`Self::mark_saved`] for content of hash `saved_hash`: a save written off the control
    /// thread wrote the content as it was when the save began, and edits made since stay
    /// unsaved.
    pub(super) fn mark_saved_as(
        &mut self,
        origin: Option<SessionOrigin>,
        saved_at: WallNs,
        saved_hash: u64,
    ) {
        self.set_working(Persist {
            origin,
            saved_hash,
            saved_at: Some(saved_at),
        });
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

    /// The working state as the recovery copy would keep it, when it has unsaved changes
    /// (it is about to be replaced); `None` when it has none. Captured here, on the control
    /// thread; [`Kept::stage`] writes it, [`Control::keep`] puts it in place.
    pub(super) fn kept(&self) -> Result<Option<Kept>, ProtoError> {
        if !self.is_dirty() {
            return Ok(None);
        }
        let data = self.session_data(true)?;
        let st = self.store.state();
        Ok(Some(Kept {
            info: Recoverable {
                replaced_at: WallNs(wall_ns()),
                origin: self.working.origin.clone(),
                measurements: u32::try_from(st.measurements.len()).unwrap_or(u32::MAX),
                traces: u32::try_from(st.traces.len()).unwrap_or(u32::MAX),
            },
            working: self.working.clone(),
            data: Box::new(data),
            dir: self.autosave_dir().map(|a| replaced_dir(&a)),
        }))
    }

    /// Makes a staged copy the recovery copy: on disk beside the autosave when it was
    /// written there, in memory when there is no autosave or the disk refused.
    pub(super) fn keep(&mut self, s: Staged) {
        let Staged { kept, tmp } = s;
        let copy = match (kept.dir, tmp) {
            (Some(dir), Some(Ok(tmp))) => match install_replaced(&dir, &tmp) {
                Ok(()) => Copy::Disk(dir),
                Err(e) => {
                    tracing::warn!("the replaced working state is kept in memory only: {e}");
                    remove_dir_off_thread(tmp);
                    Copy::Memory(kept.data)
                }
            },
            (Some(dir), Some(Err(e))) => {
                tracing::warn!(
                    "the replaced working state is kept in memory only: {}: {e}",
                    dir.display()
                );
                Copy::Memory(kept.data)
            }
            _ => Copy::Memory(kept.data),
        };
        let info = kept.info;
        tracing::info!(
            "kept {} ({} measurements, {} traces) for file.recover",
            origin_label(info.origin.as_ref()),
            info.measurements,
            info.traces
        );
        self.recover = Some(Recover {
            info,
            working: kept.working,
            copy,
        });
    }

    /// [`Self::kept`], written and kept here and now (`file.recover`, which swaps two
    /// copies the daemon holds already).
    fn keep_replaced(&mut self) -> Result<(), ProtoError> {
        if let Some(k) = self.kept()? {
            self.keep(k.stage());
        }
        Ok(())
    }

    /// The content hash and the saved hash: equal pairs mean the same content in the same
    /// saved/unsaved standing, which a replace begun on one pair may commit on.
    pub(super) fn standing(&self) -> (u64, u64) {
        (self.content_hash(), self.working.saved_hash)
    }

    /// `file.recover`: swaps the working state with the recovery copy.
    pub(super) fn file_recover(&mut self, client: &ClientId) -> Result<ReplyBody, ProtoError> {
        self.file_idle("file.recover")?;
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

/// The working state a replace keeps for `file.recover` (`Control::kept`).
pub(crate) struct Kept {
    info: Recoverable,
    working: Persist,
    data: Box<SessionData>,
    /// `<autosave>.replaced`; `None`: kept in memory only.
    dir: Option<PathBuf>,
}

/// A [`Kept`] whose disk copy is written beside its place, not yet in it: a replace refused
/// after the write must leave the recovery copy it had.
pub(crate) struct Staged {
    kept: Kept,
    /// The written copy, or why it could not be written; `None` without a place on disk.
    tmp: Option<Result<PathBuf, String>>,
}

impl Kept {
    /// Writes the copy beside its place. Disk work: a worker thread does it for a replace
    /// that runs off the control thread.
    pub(crate) fn stage(self) -> Staged {
        let tmp = self
            .dir
            .as_ref()
            .map(|d| stage_replaced(d, &self.data, &self.working, self.info.replaced_at));
        Staged { kept: self, tmp }
    }
}

impl Staged {
    /// The replace was refused: the written copy goes, the recovery copy in place stays.
    pub(crate) fn discard(self) {
        if let Some(Ok(tmp)) = self.tmp {
            remove_dir_off_thread(tmp);
        }
    }
}

/// Writes the recovery copy whole beside `dir` (`<dir>.tmp`); returns where.
fn stage_replaced(
    dir: &Path,
    data: &SessionData,
    working: &Persist,
    replaced_at: WallNs,
) -> Result<PathBuf, String> {
    let tmp = sibling(dir, "tmp");
    remove_dir(&tmp);
    session::save(&tmp, data).map_err(|e| e.to_string())?;
    write_json(
        &tmp.join(WORKING_FILE),
        &ReplacedFile {
            working: working.clone(),
            replaced_at,
        },
    )?;
    Ok(tmp)
}

/// Puts the copy written at `tmp` in `dir`'s place. Only renames here, on the control
/// thread: the copy it replaces is moved aside and removed on a thread of its own.
fn install_replaced(dir: &Path, tmp: &Path) -> Result<(), String> {
    let old = sibling(dir, &format!("old-{}", wall_ns()));
    match fs::rename(dir, &old) {
        Ok(()) => remove_dir_off_thread(old),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    }
    fs::rename(tmp, dir).map_err(|e| format!("{}: {e}", dir.display()))
}

/// `<dir>.<suffix>`.
fn sibling(dir: &Path, suffix: &str) -> PathBuf {
    let name = dir
        .file_name()
        .map_or_else(|| "replaced".into(), |n| n.to_string_lossy().into_owned());
    dir.with_file_name(format!("{name}.{suffix}"))
}

/// Removes `p` on a thread of its own: a whole session's files are not the control
/// thread's to wait for.
pub(crate) fn remove_dir_off_thread(p: PathBuf) {
    let r = std::thread::Builder::new()
        .name("ac2d-remove".into())
        .spawn({
            let p = p.clone();
            move || remove_dir(&p)
        });
    if r.is_err() {
        remove_dir(&p);
    }
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
