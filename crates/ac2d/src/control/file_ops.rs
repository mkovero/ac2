//! Removing and renaming what the daemon keeps on disk: `file.delete` and `file.rename` for
//! saved sessions (by name only, in the session directory), `rec.delete` for recordings.
//!
//! A name is one plain component ([`session::validate_name`]: no separator, no `..`, not
//! hidden, no Windows device name), and what it names must be the real thing in the
//! daemon's own directory: a session folder holding `session.json`, a recording's regular
//! files — never a link, so nothing outside the directory is reached through one.
//!
//! Each is complete or not done after a power cut:
//! - A session is deleted by renaming its folder into the staging directory (one atomic
//!   step on the same file system, the directory synced) and then removing it there; what
//!   a cut leaves half removed is in the staging directory, cleared at start. It never
//!   lists half deleted, and never fails to load half deleted.
//! - A recording is deleted by renaming its sidecar to a hidden `.<name>.ac2rec.json.deleting`
//!   (it leaves the list in that step), then removing its audio and the marker. A marker
//!   found at start ([`finish_deletes`]) is finished then.
//! - A rename is one rename of the folder, the directory synced; it never replaces: a
//!   name taken is `conflict`. (On Unix a directory renamed over another replaces it only
//!   when that one is empty, never a session; Windows never replaces a directory.)
//!
//! `file.delete` and `file.rename` are file jobs ([`super::file_jobs`]): one at a time with
//! every other file operation, so never under a load, save, import or download reading or
//! writing a session. The session the working state belongs to is not deleted (save it
//! under another name, or start a new one, first); renamed, the working state follows it.
//! `rec.delete` is refused for the recording being written or replayed. All three are
//! operators' commands and audited.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use ac2_proto::model::{RecordingRef, SessionOrigin};
use ac2_proto::{ErrorCode, ProtoError, ReplyBody};
use ac2_traces::raw;
use ac2_traces::session::{self, Manifest};

use super::Control;
use super::file_jobs::{Cancel, FileDone, Pending, Work};
use super::files::{info, session_err};
use super::generator::describe;
use super::transfer::{exists, prepare_stage, remove_dir};
use crate::route::Peer;
use crate::util::{internal, invalid, missing, perr, refused};

/// Suffix of a recording's sidecar while it is being deleted.
const DELETING: &str = ".deleting";

/// What `name` in `root` is: a session folder (not a link), or why not.
fn session_dir(root: &Path, name: &str) -> Result<PathBuf, ProtoError> {
    session::validate_name(name).map_err(session_err)?;
    let dir = root.join(name);
    match fs::symlink_metadata(&dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            Err(missing(format!("no session named {name:?}")))
        }
        Err(e) => Err(internal(format!("session {name:?}: {e}"))),
        Ok(m) if !m.file_type().is_dir() => Err(invalid(format!(
            "{name:?} in the session directory is not a session folder"
        ))),
        Ok(_) if !ac2_archive::is_session_dir(&dir) => Err(invalid(format!(
            "{name:?} in the session directory is not a session (no session.json)"
        ))),
        Ok(_) => Ok(dir),
    }
}

/// A file system refusal, named by the session rather than its host path.
fn fs_err(name: &str, e: &io::Error) -> ProtoError {
    match e.kind() {
        // Windows will not move a folder a program holds a file of open.
        io::ErrorKind::PermissionDenied | io::ErrorKind::ResourceBusy => refused(format!(
            "session {name:?} is in use on the daemon's host: {e}"
        )),
        _ => internal(format!("session {name:?}: {e}")),
    }
}

fn sync(dir: &Path) {
    if let Err(e) = ac2_paths::sync_dir(dir) {
        tracing::warn!("{}: not synced: {e}", dir.display());
    }
}

/// `file.delete`'s disk work: the folder moved into the staging directory, then removed.
fn delete_session(
    root: &Path,
    name: &str,
    aside: &Path,
    cancel: &Cancel,
) -> Result<(), ProtoError> {
    let dir = session_dir(root, name)?;
    cancel.check()?;
    prepare_stage(aside)?;
    fs::rename(&dir, aside).map_err(|e| fs_err(name, &e))?;
    sync(root);
    remove_dir(aside);
    Ok(())
}

/// Whether `a` and `b` are one directory entry: a rename that only changes the case of a
/// name, on a file system that ignores case.
fn same_entry(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (fs::symlink_metadata(a), fs::symlink_metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
            _ => false,
        }
    }
    // NTFS ignores case: two names that differ only in it are one entry.
    #[cfg(not(unix))]
    {
        let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_lowercase());
        name(a) == name(b)
    }
}

/// `file.rename`'s disk work.
fn rename_session(
    root: &Path,
    name: &str,
    to: &str,
    cancel: &Cancel,
) -> Result<Manifest, ProtoError> {
    let from = session_dir(root, name)?;
    session::validate_name(to).map_err(session_err)?;
    // Only a session this build reads: one of another version does not list, so it would
    // vanish from view under its new name as under its old.
    session::read_manifest(&from).map_err(session_err)?;
    let target = root.join(to);
    if fs::symlink_metadata(&target).is_ok() && !same_entry(&from, &target) {
        return Err(match session::peek(&target) {
            Some(p) => exists(to, p.saved_at.unwrap_or_default()),
            None => perr(
                ErrorCode::Conflict,
                format!("{to:?} is taken in the session directory"),
            ),
        });
    }
    cancel.check()?;
    fs::rename(&from, &target).map_err(|e| fs_err(name, &e))?;
    sync(root);
    session::read_manifest(&target).map_err(session_err)
}

/// The recording files of `name` in `dir`: its sidecar, which must be a regular file, and
/// its audio file, which must be one when it is there.
fn recording_files(dir: &Path, name: &str) -> Result<(PathBuf, PathBuf), ProtoError> {
    let side = raw::sidecar_path(dir, name);
    let audio = raw::audio_path(dir, name);
    match fs::symlink_metadata(&side) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(missing(format!("no recording named {name:?}")));
        }
        Err(e) => return Err(internal(format!("recording {name:?}: {e}"))),
        Ok(m) if !m.file_type().is_file() => {
            return Err(invalid(format!(
                "recording {name:?}: its sidecar is not a file"
            )));
        }
        Ok(_) => {}
    }
    match fs::symlink_metadata(&audio) {
        Ok(m) if !m.file_type().is_file() => Err(invalid(format!(
            "recording {name:?}: its audio is not a file"
        ))),
        _ => Ok((side, audio)),
    }
}

fn remove_file(p: &Path) -> io::Result<()> {
    match fs::remove_file(p) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Deletes recording `name` in `dir`: sidecar renamed to its marker (gone from the list),
/// then the audio and the marker removed.
fn delete_recording(dir: &Path, name: &str) -> Result<(), ProtoError> {
    let (side, audio) = recording_files(dir, name)?;
    let marker = marker_of(dir, name);
    let err = |e: io::Error| match e.kind() {
        io::ErrorKind::PermissionDenied | io::ErrorKind::ResourceBusy => refused(format!(
            "recording {name:?} is in use on the daemon's host: {e}"
        )),
        _ => internal(format!("recording {name:?}: {e}")),
    };
    fs::rename(&side, &marker).map_err(err)?;
    sync(dir);
    // The marker stays when the audio cannot go yet (a file open on Windows): the next
    // start finishes the delete.
    remove_file(&audio)
        .and_then(|()| remove_file(&marker))
        .map_err(err)?;
    sync(dir);
    Ok(())
}

fn marker_of(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!(".{name}{}{DELETING}", raw::SIDECAR_SUFFIX))
}

/// Finishes the recording deletes a stopped daemon left in `dir`.
pub(super) fn finish_deletes(dir: &Path) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let file = e.file_name();
        let Some(name) = file
            .to_str()
            .and_then(|f| f.strip_prefix('.'))
            .and_then(|f| f.strip_suffix(DELETING))
            .and_then(|f| f.strip_suffix(raw::SIDECAR_SUFFIX))
        else {
            continue;
        };
        if raw::validate_name(name).is_err() {
            continue;
        }
        let done = remove_file(&raw::audio_path(dir, name))
            .and_then(|()| remove_file(&marker_of(dir, name)));
        match done {
            Ok(()) => tracing::info!("recording {name:?}: its delete finished"),
            Err(e) => tracing::warn!("recording {name:?}: its delete is not finished: {e}"),
        }
    }
}

impl Control {
    /// Whether the working state belongs to the saved session `name`: by the folder on
    /// disk, not the spelling, since a file system that ignores case, or a path through a
    /// link, names one folder in several ways.
    fn is_origin(&self, name: &str) -> bool {
        let dir = self.s.session_dir.join(name);
        let origin = match &self.working.origin {
            Some(SessionOrigin::Name { name: n }) => self.s.session_dir.join(n),
            Some(SessionOrigin::Path { path }) => PathBuf::from(path),
            Some(SessionOrigin::Client { .. }) | None => return false,
        };
        origin == dir || same_entry(&origin, &dir)
    }

    pub(super) fn begin_delete(
        &mut self,
        client: &Peer,
        name: String,
    ) -> Result<(Pending, Work), ProtoError> {
        session::validate_name(&name).map_err(session_err)?;
        if self.is_origin(&name) {
            return Err(refused(format!(
                "session {name:?} is the open session: save it under another name or start \
                 a new session first"
            )));
        }
        let root = self.s.session_dir.clone();
        let aside = self.stage_path("deleted");
        let n = name.clone();
        let work: Work = Box::new(move |cancel: &Cancel| {
            FileDone::Delete(delete_session(&root, &n, &aside, cancel))
        });
        Ok((
            Pending::Delete {
                client: client.clone(),
                name,
            },
            work,
        ))
    }

    pub(super) fn finish_delete(
        &mut self,
        client: &Peer,
        name: &str,
        done: Result<(), ProtoError>,
    ) -> Result<ReplyBody, ProtoError> {
        done?;
        tracing::info!(
            target: crate::AUDIT_TARGET,
            "session {name:?} deleted by {}",
            describe(client)
        );
        Ok(ReplyBody::Ack {
            rev: self.store.rev(),
        })
    }

    pub(super) fn begin_rename(
        &mut self,
        client: &Peer,
        name: String,
        to: String,
    ) -> Result<(Pending, Work), ProtoError> {
        session::validate_name(&name).map_err(session_err)?;
        session::validate_name(&to).map_err(session_err)?;
        if name == to {
            return Err(invalid(format!("session {name:?} already has that name")));
        }
        let root = self.s.session_dir.clone();
        let (n, t) = (name.clone(), to.clone());
        let work: Work = Box::new(move |cancel: &Cancel| {
            FileDone::Rename(rename_session(&root, &n, &t, cancel))
        });
        Ok((
            Pending::Rename {
                client: client.clone(),
                name,
                to,
            },
            work,
        ))
    }

    pub(super) fn finish_rename(
        &mut self,
        client: &Peer,
        name: &str,
        to: &str,
        done: Result<Manifest, ProtoError>,
    ) -> Result<ReplyBody, ProtoError> {
        let m = done?;
        tracing::info!(
            target: crate::AUDIT_TARGET,
            "session {name:?} renamed to {to:?} by {}",
            describe(client)
        );
        if self.is_origin(name) {
            self.set_working(super::working::Persist {
                origin: Some(SessionOrigin::Name {
                    name: to.to_owned(),
                }),
                ..self.working.clone()
            });
        }
        let dir = self.s.session_dir.join(to);
        Ok(ReplyBody::SessionFile(info(to, &dir, &m)))
    }

    /// `rec.delete`.
    pub(super) fn rec_delete(
        &mut self,
        client: &Peer,
        r: &RecordingRef,
    ) -> Result<ReplyBody, ProtoError> {
        let (dir, name) = self.recording_path(client, r)?;
        raw::validate_name(&name).map_err(|e| invalid(e.to_string()))?;
        // Compared as the file system resolves them: a local client may name the
        // directory through a link.
        let real = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_owned());
        let audio = real(&raw::audio_path(&dir, &name));
        let is = |p: &str| real(Path::new(p)) == audio;
        if self.recording.is_some()
            && self
                .store
                .state()
                .recording
                .as_ref()
                .is_some_and(|r| is(&r.path))
        {
            return Err(refused(format!(
                "recording {name:?} is being recorded: stop it first"
            )));
        }
        let replaying = self
            .stream
            .as_ref()
            .and_then(|rt| rt.open.replay.as_ref())
            .is_some_and(|p| is(&p.path));
        if replaying {
            return Err(refused(format!(
                "recording {name:?} is being replayed: close the audio I/O first"
            )));
        }
        delete_recording(&dir, &name)?;
        tracing::info!(
            target: crate::AUDIT_TARGET,
            "recording {name:?} deleted by {}",
            describe(client)
        );
        Ok(ReplyBody::Ack {
            rev: self.store.rev(),
        })
    }
}
