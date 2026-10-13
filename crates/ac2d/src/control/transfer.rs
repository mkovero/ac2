//! Session archives in transit (`file.upload_*`, `file.download_*`, `file.import`): a
//! session crosses machines as one `.ac2session` file (`ac2_archive`), in chunks that fit a
//! ctrl message.
//!
//! Transfers are held in memory (at most [`MAX_TRANSFERS`] × 64 MiB) and belong to the
//! client that began them. An archive is unpacked into a staging directory under the
//! session directory (`.incoming/`, never listed: names starting with `.` are not session
//! names) — on the same file system as the sessions, so `file.import` is a rename, and not
//! in a RAM-backed temporary directory a big session could fill.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ac2_archive::{ArchiveError, MAX_ARCHIVE_BYTES};
use ac2_proto::model::{SessionOrigin, SessionRef, Transfer};
use ac2_proto::units::{Blob, ClientId, TransferId, WallNs};
use ac2_proto::{ErrorCode, ErrorDetail, ProtoError, ReplyBody};
use ac2_traces::session::{self, Manifest};

use super::Control;
use super::file_jobs::{FileDone, Pending, Work};
use super::files::{info, session_err};
use crate::util::{perr, perr_detail};

/// Largest chunk, bytes: well inside a 4 MiB ctrl message with its framing.
pub(crate) const CHUNK_BYTES: u32 = 1 << 20;
/// Transfers held at once (each up to 64 MiB in memory).
pub(crate) const MAX_TRANSFERS: usize = 4;
/// A transfer nobody touched for this long is dropped.
pub(crate) const IDLE: Duration = Duration::from_secs(120);
/// Staging directory under the session directory.
const STAGING: &str = ".incoming";

#[derive(Debug)]
enum Kind {
    Upload {
        /// The client's path of it.
        file: String,
        bytes: u64,
        sha256: String,
        data: Vec<u8>,
    },
    Download {
        data: Vec<u8>,
        /// A working-state download: the content hash and time it was packed at.
        working: Option<(u64, WallNs)>,
    },
}

#[derive(Debug)]
struct Xfer {
    client: ClientId,
    last: Instant,
    kind: Kind,
}

/// The transfers under way.
#[derive(Debug, Default)]
pub(crate) struct Transfers {
    next: u32,
    map: HashMap<TransferId, Xfer>,
    staged: u32,
}

fn archive_err(e: ArchiveError) -> ProtoError {
    let msg = e.to_string();
    match e {
        ArchiveError::Io { .. } => perr(ErrorCode::Internal, msg),
        _ => perr(ErrorCode::Invalid, msg),
    }
}

fn gone(t: TransferId) -> ProtoError {
    perr(
        ErrorCode::NotFound,
        format!("no transfer {t} (finished, or dropped after 2 min without a chunk)"),
    )
}

pub(crate) fn remove_dir(p: &Path) {
    if let Err(e) = fs::remove_dir_all(p)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("{}: not removed: {e}", p.display());
    }
}

/// The last component of a client's path, for names and messages.
pub(crate) fn file_part(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(path)
}

impl Transfers {
    fn prune(&mut self, now: Instant) {
        self.map.retain(|id, x| {
            let keep = now.duration_since(x.last) < IDLE;
            if !keep {
                tracing::info!("transfer {id} dropped: idle");
            }
            keep
        });
    }

    fn add(&mut self, client: &ClientId, kind: Kind) -> Result<TransferId, ProtoError> {
        let now = Instant::now();
        self.prune(now);
        if self.map.len() >= MAX_TRANSFERS {
            return Err(perr(
                ErrorCode::Refused,
                format!("{MAX_TRANSFERS} transfers are under way; try again when one is done"),
            ));
        }
        self.next = self.next.wrapping_add(1).max(1);
        let id = TransferId(self.next);
        self.map.insert(
            id,
            Xfer {
                client: client.clone(),
                last: now,
                kind,
            },
        );
        Ok(id)
    }

    fn get(&mut self, client: &ClientId, id: TransferId) -> Result<&mut Xfer, ProtoError> {
        self.prune(Instant::now());
        match self.map.get_mut(&id) {
            Some(x) if x.client == *client => {
                x.last = Instant::now();
                Ok(x)
            }
            _ => Err(gone(id)),
        }
    }

    /// A complete upload's client path and bytes.
    pub(crate) fn upload(
        &mut self,
        client: &ClientId,
        id: TransferId,
    ) -> Result<(String, &[u8]), ProtoError> {
        match &self.get(client, id)?.kind {
            Kind::Upload {
                file, bytes, data, ..
            } if data.len() as u64 == *bytes => Ok((file.clone(), data.as_slice())),
            Kind::Upload { bytes, data, .. } => Err(perr(
                ErrorCode::Invalid,
                format!(
                    "upload {id} is incomplete: {} of {bytes} bytes received",
                    data.len()
                ),
            )),
            Kind::Download { .. } => Err(perr(
                ErrorCode::Invalid,
                format!("transfer {id} is a download"),
            )),
        }
    }

    pub(crate) fn remove(&mut self, id: TransferId) {
        self.map.remove(&id);
    }
}

impl Control {
    /// A fresh staging directory path; nothing on disk changes ([`prepare_stage`] makes
    /// room for it).
    pub(super) fn stage_path(&mut self, tag: &str) -> PathBuf {
        self.transfers.staged = self.transfers.staged.wrapping_add(1);
        self.s.session_dir.join(STAGING).join(format!(
            "{tag}-{}-{}",
            std::process::id(),
            self.transfers.staged
        ))
    }

    /// A fresh staging directory path, made room for (not created).
    pub(super) fn stage_dir(&mut self, tag: &str) -> Result<PathBuf, ProtoError> {
        let p = self.stage_path(tag);
        prepare_stage(&p)?;
        Ok(p)
    }

    /// Removes what an earlier run left in the staging directory.
    pub(super) fn clear_staging(&self) {
        remove_dir(&self.s.session_dir.join(STAGING));
    }

    /// Unpacks complete upload `id` into a new staging directory; returns the client's path
    /// of it and the directory.
    pub(super) fn unpack_upload(
        &mut self,
        client: &ClientId,
        id: TransferId,
    ) -> Result<(String, PathBuf), ProtoError> {
        let dir = self.stage_dir("upload")?;
        let (file, data) = self.transfers.upload(client, id)?;
        unpack_into(data, &dir)?;
        Ok((file, dir))
    }

    pub(super) fn file_upload_begin(
        &mut self,
        client: &ClientId,
        file: String,
        bytes: u64,
        sha256: String,
    ) -> Result<ReplyBody, ProtoError> {
        if bytes > MAX_ARCHIVE_BYTES {
            return Err(perr(
                ErrorCode::Invalid,
                ArchiveError::TooLarge {
                    bytes,
                    max: MAX_ARCHIVE_BYTES,
                }
                .to_string(),
            ));
        }
        if sha256.len() != 64 || !sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(perr(ErrorCode::Invalid, "sha256 must be 64 hex digits"));
        }
        let file_name = file_part(&file).to_owned();
        let transfer = self.transfers.add(
            client,
            Kind::Upload {
                file,
                bytes,
                sha256: sha256.to_ascii_lowercase(),
                data: Vec::with_capacity(usize::try_from(bytes).unwrap_or(0)),
            },
        )?;
        Ok(ReplyBody::Transfer(Transfer {
            transfer,
            file_name,
            bytes,
            sha256: sha256.to_ascii_lowercase(),
            chunk_bytes: CHUNK_BYTES,
        }))
    }

    pub(super) fn file_upload_chunk(
        &mut self,
        client: &ClientId,
        id: TransferId,
        offset: u64,
        data: &Blob,
    ) -> Result<ReplyBody, ProtoError> {
        let rev = self.store.rev();
        let x = self.transfers.get(client, id)?;
        let Kind::Upload {
            bytes,
            sha256,
            data: got,
            ..
        } = &mut x.kind
        else {
            return Err(perr(
                ErrorCode::Invalid,
                format!("transfer {id} is a download"),
            ));
        };
        if offset != got.len() as u64 {
            return Err(perr(
                ErrorCode::Invalid,
                format!(
                    "upload {id}: chunk at {offset}, expected at {} (chunks go in order)",
                    got.len()
                ),
            ));
        }
        if data.0.len() > CHUNK_BYTES as usize || offset + data.0.len() as u64 > *bytes {
            return Err(perr(
                ErrorCode::Invalid,
                format!("upload {id}: the chunk goes past the size announced ({bytes} bytes)"),
            ));
        }
        got.extend_from_slice(&data.0);
        if got.len() as u64 == *bytes && ac2_archive::verify(got, sha256).is_err() {
            self.transfers.remove(id);
            return Err(perr(ErrorCode::Invalid, ArchiveError::Checksum.to_string()));
        }
        Ok(ReplyBody::Ack { rev })
    }

    /// `file.import`: stores a complete upload as a named session without opening it. The
    /// unpacking and renames run on a worker ([`super::file_jobs`]).
    pub(super) fn begin_import(
        &mut self,
        client: &ClientId,
        id: TransferId,
        name: String,
        overwrite: bool,
    ) -> Result<(Pending, Work), ProtoError> {
        session::validate_name(&name).map_err(session_err)?;
        let target = self.s.session_dir.join(&name);
        let bytes = self.transfers.upload(client, id)?.1.to_vec();
        let staged = self.stage_path("upload");
        let aside = self.stage_path("replaced");
        let root = self.s.session_dir.clone();
        let (n, t) = (name.clone(), target.clone());
        let work: Work = Box::new(move || {
            FileDone::Import(import_upload(
                &bytes, &n, &t, &root, &staged, &aside, overwrite,
            ))
        });
        Ok((
            Pending::Import {
                transfer: id,
                name,
                target,
            },
            work,
        ))
    }

    pub(super) fn finish_import(
        &mut self,
        id: TransferId,
        name: &str,
        target: &Path,
        stored: Result<Manifest, ProtoError>,
    ) -> Result<ReplyBody, ProtoError> {
        let m = stored?;
        self.transfers.remove(id);
        tracing::info!("session {name:?} imported from an upload");
        Ok(ReplyBody::SessionFile(info(name, target, &m)))
    }

    /// `file.download_begin`: the working state's content as of now (or a stored
    /// session), packed by a worker.
    pub(super) fn begin_download(
        &mut self,
        client: &ClientId,
        r: Option<&SessionRef>,
    ) -> Result<(Pending, Work), ProtoError> {
        let (stem, work, working): (String, Work, _) = match r {
            None => {
                let dir = self.stage_path("download");
                let data = self.session_data(true)?;
                let at = data.saved_at;
                let hash = self.content_hash();
                let stem = match &self.working.origin {
                    None => "untitled".to_owned(),
                    Some(SessionOrigin::Name { name }) => name.clone(),
                    Some(SessionOrigin::Path { path } | SessionOrigin::Client { path }) => {
                        let f = file_part(path);
                        f.strip_suffix(".ac2session").unwrap_or(f).to_owned()
                    }
                };
                let work: Work = Box::new(move || {
                    let packed = prepare_stage(&dir).and_then(|()| {
                        session::save(&dir, &data)
                            .map_err(session_err)
                            .and_then(|_| ac2_archive::pack_dir(&dir).map_err(archive_err))
                    });
                    remove_dir(&dir);
                    FileDone::Download(packed)
                });
                (stem, work, Some((hash, at)))
            }
            Some(SessionRef::Upload { .. }) => {
                return Err(perr(
                    ErrorCode::Invalid,
                    "an upload is on the client already",
                ));
            }
            Some(r) => {
                let (name, dir) = self.session_path(r)?;
                let work: Work = Box::new(move || {
                    FileDone::Download(
                        session::read_manifest(&dir)
                            .map_err(session_err)
                            .and_then(|_| ac2_archive::pack_dir(&dir).map_err(archive_err)),
                    )
                });
                (name, work, None)
            }
        };
        Ok((
            Pending::Download {
                client: client.clone(),
                stem,
                working,
            },
            work,
        ))
    }

    pub(super) fn finish_download(
        &mut self,
        client: &ClientId,
        stem: &str,
        working: Option<(u64, WallNs)>,
        packed: Result<Vec<u8>, ProtoError>,
    ) -> Result<ReplyBody, ProtoError> {
        let data = packed?;
        let bytes = data.len() as u64;
        let sha256 = ac2_archive::sha256_hex(&data);
        let transfer = self
            .transfers
            .add(client, Kind::Download { data, working })?;
        Ok(ReplyBody::Transfer(Transfer {
            transfer,
            file_name: format!("{stem}.{}", ac2_archive::EXTENSION),
            bytes,
            sha256,
            chunk_bytes: CHUNK_BYTES,
        }))
    }

    pub(super) fn file_download_chunk(
        &mut self,
        client: &ClientId,
        id: TransferId,
        offset: u64,
    ) -> Result<ReplyBody, ProtoError> {
        let x = self.transfers.get(client, id)?;
        let Kind::Download { data, .. } = &x.kind else {
            return Err(perr(
                ErrorCode::Invalid,
                format!("transfer {id} is an upload"),
            ));
        };
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        if start > data.len() {
            return Err(perr(
                ErrorCode::Invalid,
                format!("download {id} is {} bytes; offset {offset}", data.len()),
            ));
        }
        let end = (start + CHUNK_BYTES as usize).min(data.len());
        Ok(ReplyBody::Chunk(Blob(data[start..end].to_vec())))
    }

    pub(super) fn file_download_end(
        &mut self,
        client: &ClientId,
        id: TransferId,
        saved_as: Option<String>,
    ) -> Result<ReplyBody, ProtoError> {
        let x = self.transfers.get(client, id)?;
        let Kind::Download { working, .. } = &x.kind else {
            return Err(perr(
                ErrorCode::Invalid,
                format!("transfer {id} is an upload"),
            ));
        };
        let working = *working;
        self.transfers.remove(id);
        if let (Some(path), Some((hash, at))) = (saved_as, working) {
            tracing::info!("working state exported by {} to {path:?}", client.0);
            self.set_working(super::working::Persist {
                origin: Some(SessionOrigin::Client { path }),
                saved_hash: hash,
                saved_at: Some(at),
            });
        }
        Ok(ReplyBody::Ack {
            rev: self.store.rev(),
        })
    }
}

/// Makes room for staging directory `p`: its parent made, anything left at `p` removed.
pub(super) fn prepare_stage(p: &Path) -> Result<(), ProtoError> {
    if let Some(root) = p.parent() {
        fs::create_dir_all(root)
            .map_err(|e| perr(ErrorCode::Internal, format!("{}: {e}", root.display())))?;
    }
    remove_dir(p);
    Ok(())
}

/// Unpacks archive `bytes` into staging directory `dir`; nothing is left there on an
/// error.
pub(super) fn unpack_into(bytes: &[u8], dir: &Path) -> Result<(), ProtoError> {
    prepare_stage(dir)?;
    ac2_archive::unpack(bytes, dir).map_err(|e| {
        remove_dir(dir);
        archive_err(e)
    })
}

/// `file.import`'s disk work: unpacks into `staged`, then renames it to `target` (an
/// existing session moved to `aside` first, and back if the rename fails).
fn import_upload(
    bytes: &[u8],
    name: &str,
    target: &Path,
    root: &Path,
    staged: &Path,
    aside: &Path,
    overwrite: bool,
) -> Result<Manifest, ProtoError> {
    if !overwrite && let Some(p) = session::peek(target) {
        return Err(exists(name, p.saved_at.unwrap_or_default()));
    }
    if !overwrite && target.exists() {
        return Err(perr(
            ErrorCode::Conflict,
            format!(
                "{} exists and is not a session of this version",
                target.display()
            ),
        ));
    }
    unpack_into(bytes, staged)?;
    // Only a session this build reads is stored by name: one of another version would
    // not list. Its traces can be salvaged instead.
    let m = match session::read_manifest(staged) {
        Ok(m) => m,
        Err(e) => {
            remove_dir(staged);
            return Err(session_err(e));
        }
    };
    let old = if target.exists() {
        if let Err(e) = prepare_stage(aside) {
            remove_dir(staged);
            return Err(e);
        }
        if let Err(e) = fs::rename(target, aside) {
            remove_dir(staged);
            return Err(perr(
                ErrorCode::Internal,
                format!("{}: {e}", target.display()),
            ));
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
        return Err(perr(
            ErrorCode::Internal,
            format!("{}: {e}", target.display()),
        ));
    }
    if let Some(old) = old {
        remove_dir(old);
    }
    Ok(m)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_part_takes_the_last_component() {
        assert_eq!(
            file_part("/run/media/foh/STICK/show.ac2session"),
            "show.ac2session"
        );
        assert_eq!(file_part("C:\\shows\\fri\\"), "fri");
        assert_eq!(file_part("show"), "show");
    }

    #[test]
    fn transfers_are_bounded_and_per_client() {
        let mut t = Transfers::default();
        let a = ClientId("a".into());
        let kind = || Kind::Download {
            data: vec![1, 2, 3],
            working: None,
        };
        let ids: Vec<_> = (0..MAX_TRANSFERS)
            .map(|_| t.add(&a, kind()).expect("room"))
            .collect();
        assert_eq!(
            t.add(&a, kind()).map_err(|e| e.code),
            Err(ErrorCode::Refused)
        );
        let b = ClientId("b".into());
        assert_eq!(
            t.get(&b, ids[0]).map(|_| ()).map_err(|e| e.code),
            Err(ErrorCode::NotFound)
        );
        if let Some(old) = Instant::now().checked_sub(IDLE) {
            t.map.get_mut(&ids[0]).expect("x").last = old;
            t.prune(Instant::now());
            assert!(t.get(&a, ids[0]).is_err());
            assert!(t.add(&a, kind()).is_ok());
        }
    }
}
