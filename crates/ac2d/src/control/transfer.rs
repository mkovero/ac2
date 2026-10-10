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
use ac2_traces::session;

use super::Control;
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
    /// A fresh staging directory path (not created).
    pub(super) fn stage_dir(&mut self, tag: &str) -> Result<PathBuf, ProtoError> {
        let root = self.s.session_dir.join(STAGING);
        fs::create_dir_all(&root)
            .map_err(|e| perr(ErrorCode::Internal, format!("{}: {e}", root.display())))?;
        self.transfers.staged = self.transfers.staged.wrapping_add(1);
        let p = root.join(format!(
            "{tag}-{}-{}",
            std::process::id(),
            self.transfers.staged
        ));
        remove_dir(&p);
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
        if let Err(e) = ac2_archive::unpack(data, &dir) {
            remove_dir(&dir);
            return Err(archive_err(e));
        }
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

    /// `file.import`: stores a complete upload as a named session without opening it.
    pub(super) fn file_import(
        &mut self,
        client: &ClientId,
        id: TransferId,
        name: &str,
        overwrite: bool,
    ) -> Result<ReplyBody, ProtoError> {
        session::validate_name(name).map_err(session_err)?;
        let target = self.s.session_dir.join(name);
        if !overwrite && let Some(p) = session::peek(&target) {
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
        let (_, staged) = self.unpack_upload(client, id)?;
        // Only a session this build reads is stored by name: one of another version would
        // not list. Its traces can be salvaged instead.
        let m = match session::read_manifest(&staged) {
            Ok(m) => m,
            Err(e) => {
                remove_dir(&staged);
                return Err(session_err(e));
            }
        };
        let old = if target.exists() {
            let old = self.stage_dir("replaced")?;
            fs::rename(&target, &old)
                .map_err(|e| perr(ErrorCode::Internal, format!("{}: {e}", target.display())))?;
            Some(old)
        } else {
            None
        };
        if let Err(e) =
            fs::create_dir_all(&self.s.session_dir).and_then(|()| fs::rename(&staged, &target))
        {
            if let Some(old) = &old {
                let _ = fs::rename(old, &target);
            }
            remove_dir(&staged);
            return Err(perr(
                ErrorCode::Internal,
                format!("{}: {e}", target.display()),
            ));
        }
        if let Some(old) = old {
            remove_dir(&old);
        }
        self.transfers.remove(id);
        tracing::info!("session {name:?} imported from an upload");
        Ok(ReplyBody::SessionFile(info(name, &target, &m)))
    }

    pub(super) fn file_download_begin(
        &mut self,
        client: &ClientId,
        r: Option<&SessionRef>,
    ) -> Result<ReplyBody, ProtoError> {
        let (stem, data, working) = match r {
            None => {
                let dir = self.stage_dir("download")?;
                let data = self.session_data(true)?;
                let at = data.saved_at;
                let hash = self.content_hash();
                let packed = session::save(&dir, &data)
                    .map_err(session_err)
                    .and_then(|_| ac2_archive::pack_dir(&dir).map_err(archive_err));
                remove_dir(&dir);
                let stem = match &self.working.origin {
                    None => "untitled".to_owned(),
                    Some(SessionOrigin::Name { name }) => name.clone(),
                    Some(SessionOrigin::Path { path } | SessionOrigin::Client { path }) => {
                        let f = file_part(path);
                        f.strip_suffix(".ac2session").unwrap_or(f).to_owned()
                    }
                };
                (stem, packed?, Some((hash, at)))
            }
            Some(SessionRef::Upload { .. }) => {
                return Err(perr(
                    ErrorCode::Invalid,
                    "an upload is on the client already",
                ));
            }
            Some(r) => {
                let (name, dir) = self.session_path(r)?;
                session::read_manifest(&dir).map_err(session_err)?;
                (
                    name,
                    ac2_archive::pack_dir(&dir).map_err(archive_err)?,
                    None,
                )
            }
        };
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
