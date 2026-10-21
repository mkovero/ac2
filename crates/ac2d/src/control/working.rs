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
//!
//! None of it touches the disk on the control thread. `working.json` is written, and the
//! recovery copy put in place or removed, by the autosave's write thread
//! ([`crate::autosave::Autosaver::run`]): one thread in the order asked, so the last state
//! set is the one on disk, and shutdown waits for it. The recovery copy is written beside
//! its place by a file worker ([`Kept::stage`]); until the write thread reports it in
//! place, the copy is read from memory.

use std::fs;
use std::path::{Path, PathBuf};

use ac2_proto::event::Change;
use ac2_proto::model::{Recoverable, SessionOrigin, WorkingSession};
use ac2_proto::units::{ClientId, Seconds, WallNs};
use ac2_proto::{ErrorCode, ErrorDetail, ProtoError, ReplyBody};
use ac2_traces::session::{self, Session as SessionData};
use serde::{Deserialize, Serialize};

use super::file_jobs::{FileDone, Pending, ReplaceRead, Work};
use super::{Control, ControlMsg};
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
    /// Written beside `dir`, being put in place by the autosave's write thread (asked under
    /// `token`); read from `data` until it reports.
    Installing {
        token: u64,
        dir: PathBuf,
        data: Box<SessionData>,
    },
}

impl Copy {
    /// Whether `<autosave>.replaced` holds (or is about to hold) this copy.
    fn on_disk(&self) -> bool {
        !matches!(self, Self::Memory(_))
    }
}

/// Where `file.recover`'s worker reads the recovery copy from.
enum RecoverFrom {
    Memory(Box<SessionData>),
    Disk(PathBuf),
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
            recover: self.replaced_working.as_ref().map(|r| r.info.clone()),
        };
        if self.store.state().working != w {
            self.commit(Change::Working(w));
        }
    }

    /// The working state now belongs to `p`.
    pub(super) fn set_working(&mut self, p: Persist) {
        self.working = p;
        if let Some(a) = self.autosave.as_ref() {
            let f = a.dir.join(WORKING_FILE);
            let p = self.working.clone();
            // On the write thread (module docs): writes land in the order set.
            if !a.run(Box::new(move || write_working(&f, &p))) {
                tracing::warn!("{WORKING_FILE}: not written: the autosave writer is gone");
            }
        }
        self.working_changed();
    }

    /// Asks the write thread to remove the recovery copy on disk, after anything asked
    /// of it before (an install of the copy among them).
    fn remove_replaced(&self) {
        if let Some(a) = self.autosave.as_ref() {
            let dir = replaced_dir(&a.dir);
            if !a.run(Box::new(move || retire(&dir))) {
                tracing::warn!("the replaced working state on disk is not removed");
            }
        }
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
        let had_disk = self
            .replaced_working
            .as_ref()
            .is_some_and(|r| r.copy.on_disk());
        let Staged { kept, tmp } = s;
        let copy = match (kept.dir, tmp) {
            (Some(dir), Some(Ok(tmp))) => self.install(dir, tmp, kept.data),
            (Some(dir), Some(Err(e))) => {
                tracing::warn!(
                    "the replaced working state is kept in memory only: {}: {e}",
                    dir.display()
                );
                Copy::Memory(kept.data)
            }
            _ => Copy::Memory(kept.data),
        };
        // A copy kept in memory only must not leave the one it replaces to come back
        // after a restart.
        if had_disk && !copy.on_disk() {
            self.remove_replaced();
        }
        let info = kept.info;
        tracing::info!(
            "kept {} ({} measurements, {} traces) for file.recover",
            origin_label(info.origin.as_ref()),
            info.measurements,
            info.traces
        );
        self.replaced_working = Some(Recover {
            info,
            working: kept.working,
            copy,
        });
    }

    /// Asks the write thread to put the copy written at `tmp` in `dir`'s place.
    fn install(&mut self, dir: PathBuf, tmp: PathBuf, data: Box<SessionData>) -> Copy {
        let token = self.next_token;
        self.next_token += 1;
        let to = self.s.to_self.clone();
        let (d, t) = (dir.clone(), tmp.clone());
        let queued = self.autosave.as_ref().is_some_and(|a| {
            a.run(Box::new(move || {
                let result = install_replaced(&d, &t);
                if result.is_err() {
                    remove_dir_off_thread(t);
                }
                let _ = to.send(ControlMsg::RecoverInstalled { token, result });
            }))
        });
        if queued {
            Copy::Installing { token, dir, data }
        } else {
            tracing::warn!(
                "the replaced working state is kept in memory only: the autosave writer is gone"
            );
            remove_dir_off_thread(tmp);
            Copy::Memory(data)
        }
    }

    /// The write thread reported on the install asked under `token`.
    pub(super) fn recover_installed(&mut self, token: u64, result: Result<(), String>) {
        let Some(r) = self.replaced_working.take() else {
            return;
        };
        let copy = match r.copy {
            Copy::Installing {
                token: t,
                dir,
                data,
            } if t == token => match result {
                Ok(()) => Copy::Disk(dir),
                Err(e) => {
                    tracing::warn!("the replaced working state is kept in memory only: {e}");
                    Copy::Memory(data)
                }
            },
            // A copy that has been replaced or recovered since.
            other => other,
        };
        self.replaced_working = Some(Recover { copy, ..r });
    }

    /// The content hash and the saved hash: equal pairs mean the same content in the same
    /// saved/unsaved standing, which a replace begun on one pair may commit on.
    pub(super) fn standing(&self) -> (u64, u64) {
        (self.content_hash(), self.working.saved_hash)
    }

    /// `file.recover`: swaps the working state with the recovery copy. A worker reads the
    /// copy and writes the working state it replaces (when that has unsaved changes);
    /// [`Self::finish_recover`] commits.
    pub(super) fn begin_recover(
        &mut self,
        client: &ClientId,
    ) -> Result<(Pending, Work), ProtoError> {
        let Some(r) = self.replaced_working.as_ref() else {
            return Err(perr(
                ErrorCode::NotFound,
                "no replaced working state to recover",
            ));
        };
        let src = match &r.copy {
            Copy::Memory(d) | Copy::Installing { data: d, .. } => RecoverFrom::Memory(d.clone()),
            Copy::Disk(dir) => RecoverFrom::Disk(dir.clone()),
        };
        let kept = self.kept()?;
        let work: Work = Box::new(move || {
            let read = match src {
                RecoverFrom::Memory(d) => Ok(*d),
                RecoverFrom::Disk(dir) => session::load(&dir).map_err(|e| {
                    perr(
                        ErrorCode::Internal,
                        format!("the replaced working state cannot be read: {e}"),
                    )
                }),
            };
            // The copy read stays recoverable when it cannot replace the working state.
            let kept = if read.is_ok() {
                kept.map(Kept::stage)
            } else {
                None
            };
            FileDone::Replace(ReplaceRead {
                read: read.map(|d| (None, d)),
                kept,
            })
        });
        Ok((
            Pending::Recover {
                client: client.clone(),
                standing: self.standing(),
            },
            work,
        ))
    }

    /// Commits a `file.recover` its worker read: only onto the working state it began on
    /// (`super::file_jobs`).
    pub(super) fn finish_recover(
        &mut self,
        client: &ClientId,
        standing: (u64, u64),
        r: ReplaceRead,
    ) -> Result<ReplyBody, ProtoError> {
        let ReplaceRead { read, kept } = r;
        let checked = read.and_then(|(_, data)| {
            if self.standing() != standing {
                return Err(perr(
                    ErrorCode::Conflict,
                    "the working state changed while the replaced one was being read; \
                     nothing was recovered: ask again",
                ));
            }
            match self.replaced_working.take() {
                Some(old) => Ok((old, data)),
                None => Err(perr(
                    ErrorCode::NotFound,
                    "no replaced working state to recover",
                )),
            }
        });
        let (old, data) = match checked {
            Ok(x) => x,
            Err(e) => {
                if let Some(k) = kept {
                    k.discard();
                }
                return Err(e);
            }
        };
        if let Err(e) = self.replace_session(Some(client), data) {
            // Nothing changed: the copy stays recoverable where it is.
            self.replaced_working = Some(old);
            if let Some(k) = kept {
                k.discard();
            }
            return Err(e);
        }
        if let Some(k) = kept {
            self.keep(k);
        }
        let now_on_disk = self
            .replaced_working
            .as_ref()
            .is_some_and(|r| r.copy.on_disk());
        if old.copy.on_disk() && !now_on_disk {
            self.remove_replaced();
        }
        tracing::info!("recovered {}", origin_label(old.working.origin.as_ref()));
        self.set_working(old.working);
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
            let f = dir.join(WORKING_FILE);
            match fs::read(&f) {
                Err(_) => Persist::untitled(),
                Ok(b) => serde_json::from_slice::<Persist>(&b).unwrap_or_else(|e| {
                    let d = crate::damage::Damage {
                        kind: ac2_proto::model::StateFile::Working,
                        set_aside_as: ac2_paths::set_aside_damaged(&f).ok(),
                        path: f,
                        reason: format!("{e}; the session is untitled"),
                    };
                    d.log();
                    self.note_damage(d);
                    Persist::untitled()
                }),
            }
        } else {
            Persist::untitled()
        };
        let rd = replaced_dir(&dir);
        remove_leftovers(&rd);
        if rd.join(session::MANIFEST).is_file()
            && let Some(f) = read_json::<ReplacedFile>(&rd.join(WORKING_FILE))
            && let Ok(m) = session::read_manifest(&rd)
        {
            self.replaced_working = Some(Recover {
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

/// Writes the recovery copy whole beside `dir` (`<dir>.tmp-<ns>`); returns where. A name
/// of its own: the write thread may still be installing the copy staged before it.
fn stage_replaced(
    dir: &Path,
    data: &SessionData,
    working: &Persist,
    replaced_at: WallNs,
) -> Result<PathBuf, String> {
    let tmp = sibling(dir, &format!("tmp-{}", wall_ns()));
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

/// Puts the copy written at `tmp` in `dir`'s place (on the autosave's write thread).
///
/// The copy in place is renamed aside to a fresh name first, then the new one renamed to
/// `dir`, then the old removed on a thread of its own. Renaming a directory over an
/// existing one is not portable: POSIX replaces only an empty directory, Windows refuses
/// any existing destination. Each rename here goes to a name that does not exist, which
/// every OS does. A crash between the two loses only the recovery copy; [`remove_leftovers`]
/// clears what it left at the next start.
fn install_replaced(dir: &Path, tmp: &Path) -> Result<(), String> {
    set_aside(dir)?;
    fs::rename(tmp, dir).map_err(|e| format!("{}: {e}", dir.display()))
}

/// Renames `dir` aside to `<dir>.old-<ns>` and removes it there on a thread of its own;
/// returns the error when it cannot be moved (none when it is not there).
fn set_aside(dir: &Path) -> Result<(), String> {
    let old = sibling(dir, &format!("old-{}", wall_ns()));
    match fs::rename(dir, &old) {
        Ok(()) => {
            remove_dir_off_thread(old);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", dir.display())),
    }
}

/// Removes the recovery copy at `dir` (on the autosave's write thread).
fn retire(dir: &Path) {
    if let Err(e) = set_aside(dir) {
        tracing::warn!("not removed: {e}");
    }
}

/// Writes `working.json` (on the autosave's write thread).
fn write_working(f: &Path, p: &Persist) {
    // No file reads back as untitled: an empty start leaves no autosave behind.
    if *p == Persist::untitled() && !f.exists() {
        return;
    }
    if let Err(e) = write_json(f, p) {
        tracing::warn!("{}: not written: {e}", f.display());
    }
}

/// At start, before any request: removes the staged and set-aside copies
/// (`<dir>.tmp-*`, `<dir>.old-*`) a run that ended mid-install left beside `dir`.
fn remove_leftovers(dir: &Path) {
    let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) else {
        return;
    };
    let name = name.to_string_lossy();
    let (tmp, old) = (format!("{name}.tmp"), format!("{name}.old-"));
    let Ok(rd) = fs::read_dir(parent) else {
        return;
    };
    for e in rd.flatten() {
        let n = e.file_name();
        let n = n.to_string_lossy();
        if n.starts_with(&tmp) || n.starts_with(&old) {
            remove_dir_off_thread(e.path());
        }
    }
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
