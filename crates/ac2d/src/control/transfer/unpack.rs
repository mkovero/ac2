//! A worker's disk work on archives: an upload's spool file unpacked into the staging
//! directory (stored as a session by `file.import`), a session packed into a spool file
//! for a download. Each streams: neither holds an archive in memory.

use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use ac2_archive::ArchiveError;
use ac2_proto::units::WallNs;
use ac2_proto::{ErrorCode, ErrorDetail, ProtoError};
use ac2_traces::session::{self, Manifest};

use super::super::file_jobs::Cancel;
use super::super::files::session_err;
use super::{Disk, STAGING};
use crate::util::{internal, invalid, perr, perr_detail, refused};

fn archive_err(e: ArchiveError) -> ProtoError {
    let msg = e.to_string();
    match e {
        ArchiveError::Io { .. } => internal(msg),
        ArchiveError::NoRoom { .. } => refused(msg),
        _ => invalid(msg),
    }
}

pub(crate) fn remove_dir(p: &Path) {
    if let Err(e) = fs::remove_dir_all(p)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("{}: not removed: {e}", p.display());
    }
}

/// Makes room for staging path `p`: its parent made, anything left at `p` removed.
pub(crate) fn prepare_stage(p: &Path) -> Result<(), ProtoError> {
    if let Some(root) = p.parent() {
        fs::create_dir_all(root).map_err(|e| internal(format!("{STAGING}: {e}")))?;
    }
    remove_dir(p);
    Ok(())
}

/// A complete upload's archive, to unpack into `stage`.
pub(crate) struct UploadSource {
    pub(crate) archive: PathBuf,
    pub(crate) stage: PathBuf,
    pub(crate) disk: Disk,
}

impl UploadSource {
    /// Unpacks the archive into its staging directory, leaving the disk its reserve;
    /// nothing is left there on an error.
    pub(crate) fn unpack(&self) -> Result<(), ProtoError> {
        let dir = &self.stage;
        prepare_stage(dir)?;
        let parent = dir.parent().unwrap_or(dir);
        let room = self
            .disk
            .room(parent)
            .map_err(|e| internal(format!("free space of {STAGING}: {e}")))?;
        let f = fs::File::open(&self.archive)
            .map_err(|e| internal(format!("the upload's spool file: {e}")))?;
        ac2_archive::unpack_from(BufReader::new(f), dir, room).map_err(|e| {
            remove_dir(dir);
            archive_err(e)
        })
    }
}

/// A session packed into a spool file.
#[derive(Debug)]
pub(crate) struct Packed {
    pub(crate) path: PathBuf,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
}

/// A packed file nobody took (its requester's role changed, its job was cut, no slot was
/// left) is removed with it; one a download adopted has moved away already.
impl Drop for Packed {
    fn drop(&mut self) {
        super::spool::remove(&self.path);
    }
}

/// Packs session directory `dir` into the file `to`, after checking that the disk can take
/// it ([`ac2_archive::session_bytes`], an upper bound) and keep what `disk` keeps; nothing
/// is left at `to` on an error.
pub(crate) fn pack_session(dir: &Path, to: &Path, disk: &Disk) -> Result<Packed, ProtoError> {
    let parent = to.parent().unwrap_or(to);
    fs::create_dir_all(parent).map_err(|e| internal(format!("{STAGING}: {e}")))?;
    let need = ac2_archive::session_bytes(dir).map_err(archive_err)?;
    let room = disk
        .room(parent)
        .map_err(|e| internal(format!("free space of {STAGING}: {e}")))?;
    if room < need {
        return Err(refused(format!(
            "no room to pack {need} bytes: the daemon's disk is nearly full"
        )));
    }
    let p = fs::File::create(to)
        .map_err(|e| internal(format!("{STAGING}: {e}")))
        .and_then(|f| {
            let mut w = std::io::BufWriter::new(f);
            let p = ac2_archive::pack_dir_to(dir, &mut w).map_err(archive_err)?;
            w.into_inner()
                .map_err(|e| internal(format!("{STAGING}: {}", e.error())))?;
            Ok(p)
        });
    match p {
        Ok(p) => Ok(Packed {
            path: to.to_owned(),
            bytes: p.bytes,
            sha256: p.sha256,
        }),
        Err(e) => {
            super::spool::remove(to);
            Err(e)
        }
    }
}

/// Tag of the staging folder `file.import` parks a session it replaces in, as
/// `<tag>-…/<name>`: the name kept so that a stop between the two renames can put it back
/// ([`restore_replaced`]).
pub(crate) const REPLACED: &str = "replaced";

/// `file.import`'s disk work: unpacks into the staging directory, then renames it to
/// `target` (an existing session moved to `aside` first, and back if the rename fails).
pub(crate) fn import_upload(
    src: &UploadSource,
    name: &str,
    target: &Path,
    root: &Path,
    aside: &Path,
    overwrite: bool,
    cancel: &Cancel,
) -> Result<Manifest, ProtoError> {
    let staged = &src.stage;
    if !overwrite && let Some(p) = session::peek(target) {
        return Err(exists(name, p.saved_at.unwrap_or_default()));
    }
    if !overwrite && target.exists() {
        return Err(perr(
            ErrorCode::Conflict,
            format!(
                "{name:?} exists in the session directory and is not a session of this version"
            ),
        ));
    }
    // A folder without a session manifest is someone's own: an import replaces sessions
    // only, never what it cannot tell is one.
    if fs::symlink_metadata(target).is_ok() && !ac2_archive::is_session_dir(target) {
        return Err(perr(
            ErrorCode::Conflict,
            format!("{name:?} exists in the session directory and is not a session: not replaced"),
        ));
    }
    src.unpack()?;
    // Only a session this build reads is stored by name: one of another version would
    // not list. Its traces can be salvaged instead.
    let m = match session::read_manifest(staged) {
        Ok(m) => m,
        Err(e) => {
            remove_dir(staged);
            return Err(session_err(e));
        }
    };
    // The last moment before the session directory changes: a requester whose role was
    // taken down meanwhile imports nothing.
    if let Err(e) = cancel.check() {
        remove_dir(staged);
        return Err(e);
    }
    let old = if target.exists() {
        if let Err(e) = prepare_stage(aside) {
            remove_dir(staged);
            return Err(e);
        }
        if let Err(e) = fs::rename(target, aside) {
            remove_dir(staged);
            return Err(internal(format!("session {name:?}: {e}")));
        }
        Some(aside)
    } else {
        None
    };
    if let Err(e) = fs::create_dir_all(root).and_then(|()| fs::rename(staged, target)) {
        if let Some(old) = old {
            let _ = fs::rename(old, target);
        }
        remove_dir(staged);
        return Err(internal(format!("session {name:?}: {e}")));
    }
    if let Some(old) = old {
        // The parked session with the folder that names it.
        remove_dir(old.parent().unwrap_or(old));
    }
    Ok(m)
}

/// Before the staging directory `staging` is cleared at start: what `file.import` had
/// parked to replace it, and whose name in `root` is still free (the stop came before the
/// new one was renamed in), goes back, session or not, since clearing the staging would
/// delete its only copy. One whose name was taken was replaced and goes with the staging.
pub(crate) fn restore_replaced(staging: &Path, root: &Path) {
    let Ok(rd) = fs::read_dir(staging) else {
        return;
    };
    let prefix = format!("{REPLACED}-");
    for parked in rd.flatten() {
        if !parked.file_name().to_string_lossy().starts_with(&prefix) {
            continue;
        }
        for s in fs::read_dir(parked.path()).into_iter().flatten().flatten() {
            let name = s.file_name().to_string_lossy().into_owned();
            let to = root.join(&name);
            if session::validate_name(&name).is_err() || fs::symlink_metadata(&to).is_ok()
            {
                continue;
            }
            match fs::rename(s.path(), &to) {
                Ok(()) => {
                    if let Err(e) = ac2_paths::sync_dir(root) {
                        tracing::warn!("{}: not synced: {e}", root.display());
                    }
                    tracing::warn!(
                        "{name:?} was being replaced by an import when the daemon stopped: \
                         put back"
                    );
                }
                Err(e) => tracing::warn!("session {name:?}: not put back: {e}"),
            }
        }
    }
}

/// `conflict` / `session_exists`.
pub(crate) fn exists(name: &str, saved_at: WallNs) -> ProtoError {
    perr_detail(
        ErrorCode::Conflict,
        format!("a session named {name:?} exists; overwrite it to replace it"),
        ErrorDetail::SessionExists {
            name: name.to_owned(),
            saved_at,
        },
    )
}
