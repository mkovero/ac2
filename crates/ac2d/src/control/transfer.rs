//! Files in transit (`file.upload_*`, `file.download_*`, `file.import`, `trace.export`): a
//! session crosses machines as one `.ac2session` file (`ac2_archive`), a trace file as
//! itself, in chunks that fit a ctrl message.
//!
//! Every transfer is spooled to disk ([`spool`]) in the staging directory under the session
//! directory (`.incoming/`, never listed: names starting with `.` are not session names): an
//! upload as `up-<transfer>.part`, hashed as it arrives, a download as
//! `down-<transfer>.part`, packed before it is offered. On the same file system as the
//! sessions, so `file.import` is a rename, and not in a RAM-backed temporary directory a
//! big session could fill. Nothing is held in memory but a snapshot ([`snapshot`]), which
//! is held apart from these and outside their quota.
//!
//! A peer (a key and its name, or a local connection: what the dedup window keys on) holds
//! at most [`MAX_PEER_TRANSFERS`], the daemon [`MAX_TRANSFERS`]; viewers together hold at most
//! half of them, so an operator always has room. A transfer ends [`IDLE`] after its last
//! chunk — the daemon has no signal for a ctrl connection's end, so this is what frees a
//! vanished client's slots — or at its lifetime ([`TransferLimits::lifetime`]), whichever
//! comes first, however often it is touched; on `file.upload_abort` or `file.download_end`;
//! when its peer's role is taken down; or when it is used up. Its spool file goes with it,
//! and what a previous run left in the staging directory is cleared at start.
//!
//! An upload leaves [`TransferLimits::reserve`] free on the session directory's file system
//! (counting what the uploads under way still have to write), and so does unpacking one; a
//! download leaves [`TransferLimits::pack_reserve`].

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ac2_archive::{ArchiveError, Hasher, MAX_ARCHIVE_BYTES};
use ac2_proto::model::{SessionOrigin, SessionRef, Transfer};
use ac2_proto::units::{Blob, TransferId, WallNs};
use ac2_proto::{ProtoError, ReplyBody};
use ac2_traces::session::{self, Manifest};

use super::Control;
use super::file_jobs::{Cancel, FileDone, Pending, Work};
use super::files::{info, session_err};
use crate::route::Peer;
use crate::util::{internal, invalid, missing, refused};

mod limits;
mod snapshot;
mod spool;
mod unpack;

pub(crate) use limits::Disk;
pub use limits::{FreeSpace, TransferLimits};
pub(crate) use spool::Spool;
pub(crate) use unpack::{
    Packed, UploadSource, exists, import_upload, pack_session, prepare_stage, remove_dir,
};

/// Largest chunk, bytes: well inside a 4 MiB ctrl message with its framing.
pub(crate) const CHUNK_BYTES: u32 = 1 << 20;
/// Transfers held at once by the daemon.
pub(crate) const MAX_TRANSFERS: usize = 8;
/// Transfers one peer holds at once.
pub(crate) const MAX_PEER_TRANSFERS: usize = 2;
/// A transfer nobody touched for this long is dropped.
pub(crate) const IDLE: std::time::Duration = std::time::Duration::from_secs(120);
/// Transfers viewers may hold together: the rest stay an operator's.
pub(crate) const MAX_VIEWER_TRANSFERS: usize = MAX_TRANSFERS / 2;
/// Staging directory under the session directory.
pub(crate) const STAGING: &str = ".incoming";

#[derive(Debug)]
enum Kind {
    Upload {
        /// The last component of the client's path: the rest names directories on a
        /// machine every other client would read it from.
        file: String,
        bytes: u64,
        sha256: String,
        spool: Spool,
        /// Until the last byte arrived and matched `sha256`.
        hasher: Option<Hasher>,
        /// A file job reads it: kept past its idle time until the job answers.
        busy: bool,
    },
    Download {
        spool: Spool,
        /// A working-state download: the content hash and time it was packed at.
        working: Option<(u64, WallNs)>,
    },
}

#[derive(Debug)]
struct Xfer {
    client: Peer,
    /// A viewer's: counted against [`MAX_VIEWER_TRANSFERS`]. A role change drops the
    /// transfer, so the role it began with is the role it has.
    viewer: bool,
    born: Instant,
    last: Instant,
    kind: Kind,
}

impl Xfer {
    fn busy(&self) -> bool {
        matches!(self.kind, Kind::Upload { busy: true, .. })
    }
}

/// The transfers under way.
#[derive(Debug)]
pub(crate) struct Transfers {
    next: u32,
    map: HashMap<TransferId, Xfer>,
    staged: u32,
    pub(crate) limits: TransferLimits,
    snapshots: snapshot::Snapshots,
}

impl Default for Transfers {
    fn default() -> Self {
        Self::new(TransferLimits::default())
    }
}

fn gone(t: TransferId) -> ProtoError {
    missing(format!(
        "no transfer {t} (finished, aborted, or dropped after 2 min without a chunk or at \
         its lifetime)"
    ))
}

/// A refusal for a full disk, or an internal error: either way the transfer is gone.
fn disk_err(what: &str, e: &std::io::Error) -> ProtoError {
    if e.kind() == std::io::ErrorKind::StorageFull {
        refused(format!("{what}: the daemon's disk is full"))
    } else {
        internal(format!("{what}: {e}"))
    }
}

/// The last component of a client's path, for names and messages.
pub(crate) fn file_part(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(path)
}

/// Most characters of a client's file name the daemon keeps.
const MAX_FILE_CHARS: usize = 255;

/// The last component of a path a client sent, as the daemon keeps it (in the state, in a
/// log line): one line, bounded.
pub(crate) fn client_file(path: &str) -> String {
    ac2_proto::printable::printable(file_part(path), MAX_FILE_CHARS)
}

impl Transfers {
    pub(crate) fn new(limits: TransferLimits) -> Self {
        Self {
            next: 0,
            map: HashMap::new(),
            staged: 0,
            limits,
            snapshots: snapshot::Snapshots::default(),
        }
    }

    /// A transfer id neither table holds: snapshot transfers share the id space, as they
    /// share `file.download_chunk` and `file.download_end`.
    fn next_id(&mut self) -> TransferId {
        self.next = self.next.wrapping_add(1).max(1);
        TransferId(self.next)
    }

    /// The daemon's periodic tick: transfers idle or past their lifetime go with their
    /// spool files and held snapshots, whether or not any client sends a transfer command.
    pub(crate) fn tick(&mut self, now: Instant) {
        self.prune(now);
        self.snapshots.tick(now);
    }

    fn prune(&mut self, now: Instant) {
        let lifetime = self.limits.lifetime;
        self.map.retain(|id, x| {
            if x.busy() {
                return true;
            }
            let idle = now.duration_since(x.last) >= IDLE;
            let old = now.duration_since(x.born) >= lifetime;
            if idle || old {
                tracing::info!(
                    "transfer {id} dropped: {}",
                    if idle { "idle" } else { "at its lifetime" }
                );
            }
            !idle && !old
        });
    }

    /// A new transfer's id, when `client` and the daemon have a slot for it.
    fn admit(&mut self, client: &Peer) -> Result<TransferId, ProtoError> {
        self.prune(Instant::now());
        if self.map.len() >= MAX_TRANSFERS {
            return Err(refused(format!(
                "{} transfers are under way, all this daemon holds; try again when one is \
                 done",
                self.map.len()
            )));
        }
        let own = self.map.values().filter(|x| x.client == *client).count();
        if own >= MAX_PEER_TRANSFERS {
            return Err(refused(format!(
                "this client holds {own} transfers, all one client may hold; finish or \
                 abort one first"
            )));
        }
        Ok(self.next_id())
    }

    fn insert(&mut self, id: TransferId, client: &Peer, viewer: bool, kind: Kind) {
        let now = Instant::now();
        self.map.insert(
            id,
            Xfer {
                client: client.clone(),
                viewer,
                born: now,
                last: now,
                kind,
            },
        );
    }

    fn get(&mut self, client: &Peer, id: TransferId) -> Result<&mut Xfer, ProtoError> {
        self.prune(Instant::now());
        match self.map.get_mut(&id) {
            Some(x) if x.client == *client => {
                x.last = Instant::now();
                Ok(x)
            }
            _ => Err(gone(id)),
        }
    }

    /// A complete upload's file name and spool file.
    fn complete(&mut self, client: &Peer, id: TransferId) -> Result<&mut Xfer, ProtoError> {
        let x = self.get(client, id)?;
        match &x.kind {
            Kind::Upload { bytes, spool, .. } if spool.len() == *bytes => Ok(x),
            Kind::Upload { bytes, spool, .. } => Err(invalid(format!(
                "upload {id} is incomplete: {} of {bytes} bytes received",
                spool.len()
            ))),
            Kind::Download { .. } => Err(invalid(format!("transfer {id} is a download"))),
        }
    }

    /// A complete upload's file name and spool file, kept for a file job to read until
    /// [`Self::release`]: not dropped as idle meanwhile, and no `file.upload_abort` removes
    /// it under the job.
    pub(crate) fn claim(
        &mut self,
        client: &Peer,
        id: TransferId,
    ) -> Result<(String, PathBuf), ProtoError> {
        match &mut self.complete(client, id)?.kind {
            Kind::Upload {
                file, spool, busy, ..
            } => {
                *busy = true;
                Ok((file.clone(), spool.path().to_owned()))
            }
            Kind::Download { .. } => Err(internal("a download")),
        }
    }

    /// The job reading upload `id` answered: it idles from now.
    pub(crate) fn release(&mut self, id: TransferId) {
        if let Some(x) = self.map.get_mut(&id) {
            x.last = Instant::now();
            if let Kind::Upload { busy, .. } = &mut x.kind {
                *busy = false;
            }
        }
    }

    /// A complete upload's file name and whole content: for a trace file, whose readers
    /// take it whole.
    pub(crate) fn upload_bytes(
        &mut self,
        client: &Peer,
        id: TransferId,
    ) -> Result<(String, Vec<u8>), ProtoError> {
        match &mut self.complete(client, id)?.kind {
            Kind::Upload {
                file, spool, bytes, ..
            } => {
                let usize_bytes = usize::try_from(*bytes).unwrap_or(usize::MAX);
                let data = spool
                    .read_at(0, usize_bytes)
                    .map_err(|e| internal(format!("upload {id}: {e}")))?;
                Ok((file.clone(), data))
            }
            Kind::Download { .. } => Err(internal("a download")),
        }
    }

    /// Bytes the uploads under way have still to write.
    fn outstanding(&self) -> u64 {
        self.map
            .values()
            .map(|x| match &x.kind {
                Kind::Upload { bytes, spool, .. } => bytes.saturating_sub(spool.len()),
                Kind::Download { .. } => 0,
            })
            .sum()
    }

    /// How many transfers viewers hold; viewer `key` alone when given.
    pub(crate) fn held_by_viewers(&mut self, key: Option<&ac2_zmq::PublicKey>) -> usize {
        self.prune(Instant::now());
        self.map
            .values()
            .filter(|x| {
                x.viewer && key.is_none_or(|k| x.client.curve().is_some_and(|(_, c)| c == k))
            })
            .count()
    }

    /// Drops every transfer of a network key whose role was taken down.
    pub(crate) fn forget_key(&mut self, key: &ac2_zmq::PublicKey) {
        self.map
            .retain(|_, x| x.client.curve().is_none_or(|(_, k)| k != key));
        self.snapshots.forget_key(key);
    }

    pub(crate) fn remove(&mut self, id: TransferId) {
        self.map.remove(&id);
    }
}

impl Control {
    /// The staging directory.
    pub(super) fn staging(&self) -> PathBuf {
        self.s.session_dir.join(STAGING)
    }

    /// A fresh staging path; nothing on disk changes ([`prepare_stage`] makes room for
    /// it).
    pub(super) fn stage_path(&mut self, tag: &str) -> PathBuf {
        self.transfers.staged = self.transfers.staged.wrapping_add(1);
        self.staging().join(format!(
            "{tag}-{}-{}",
            std::process::id(),
            self.transfers.staged
        ))
    }

    /// Removes what an earlier run left in the staging directory: spool files, unpacked
    /// uploads, sessions half deleted; a session half replaced by an import is put back
    /// first.
    pub(super) fn clear_staging(&self) {
        unpack::restore_replaced(&self.staging(), &self.s.session_dir);
        remove_dir(&self.staging());
    }

    /// What unpacking an upload must leave free.
    pub(super) fn unpack_disk(&self) -> Disk {
        Disk {
            free_space: self.transfers.limits.free_space.clone(),
            keep: self.transfers.limits.reserve,
        }
    }

    /// What packing a download must leave free.
    pub(super) fn pack_disk(&self) -> Disk {
        Disk {
            free_space: self.transfers.limits.free_space.clone(),
            keep: self.transfers.limits.pack_reserve,
        }
    }

    /// The staging directory, made.
    fn made_staging(&self) -> Result<PathBuf, ProtoError> {
        let dir = self.staging();
        fs::create_dir_all(&dir).map_err(|e| internal(format!("{STAGING}: {e}")))?;
        Ok(dir)
    }

    pub(super) fn file_upload_begin(
        &mut self,
        client: &Peer,
        file: String,
        bytes: u64,
        sha256: String,
    ) -> Result<ReplyBody, ProtoError> {
        if bytes > MAX_ARCHIVE_BYTES {
            return Err(invalid(
                ArchiveError::TooLarge {
                    bytes,
                    max: MAX_ARCHIVE_BYTES,
                }
                .to_string(),
            ));
        }
        if sha256.len() != 64 || !sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(invalid("sha256 must be 64 hex digits"));
        }
        let sha256 = sha256.to_ascii_lowercase();
        // An empty upload is complete at once and no chunk ever reaches the check: its sum
        // is checked here.
        if bytes == 0 && sha256 != Hasher::default().hex() {
            return Err(invalid(ArchiveError::Checksum.to_string()));
        }
        let file_name = client_file(&file);
        let transfer = self.transfers.admit(client)?;
        let dir = self.made_staging()?;
        let free = self
            .transfers
            .limits
            .free_space
            .of(&dir)
            .map_err(|e| internal(format!("free space of {STAGING}: {e}")))?;
        let reserve = self.transfers.limits.reserve;
        let room = free.saturating_sub(self.transfers.outstanding());
        if room < bytes.saturating_add(reserve) {
            return Err(refused(format!(
                "no room for {bytes} bytes: the daemon's disk has {free} bytes free, \
                 uploads under way take {} of them, and {reserve} must stay free",
                free - room
            )));
        }
        let spool = Spool::create(dir.join(format!("up-{transfer}.part")))
            .map_err(|e| disk_err("upload", &e))?;
        self.transfers.insert(
            transfer,
            client,
            false,
            Kind::Upload {
                file: file_name.clone(),
                bytes,
                sha256: sha256.clone(),
                spool,
                hasher: Some(Hasher::default()),
                busy: false,
            },
        );
        Ok(ReplyBody::Transfer(Transfer {
            transfer,
            file_name,
            bytes,
            sha256,
            chunk_bytes: CHUNK_BYTES,
        }))
    }

    pub(super) fn file_upload_chunk(
        &mut self,
        client: &Peer,
        id: TransferId,
        offset: u64,
        data: &Blob,
    ) -> Result<ReplyBody, ProtoError> {
        let rev = self.store.rev();
        let free_space = self.transfers.limits.free_space.clone();
        let floor = self.transfers.limits.pack_reserve;
        let x = self.transfers.get(client, id)?;
        match append_chunk(&mut x.kind, id, offset, &data.0, &free_space, floor) {
            Ok(()) => Ok(ReplyBody::Ack { rev }),
            Err(Chunk::Refused(e)) => Err(e),
            Err(Chunk::Dropped(e)) => {
                self.transfers.remove(id);
                Err(e)
            }
        }
    }

    /// `file.upload_abort`: the upload and its spool file go now.
    pub(super) fn file_upload_abort(
        &mut self,
        client: &Peer,
        id: TransferId,
    ) -> Result<ReplyBody, ProtoError> {
        let x = self.transfers.get(client, id)?;
        match &x.kind {
            Kind::Download { .. } => {
                return Err(invalid(format!(
                    "transfer {id} is a download: end it with file.download_end"
                )));
            }
            Kind::Upload { busy: true, .. } => {
                return Err(refused(format!(
                    "upload {id} is being read by a file operation; it ends with it"
                )));
            }
            Kind::Upload { .. } => {}
        }
        self.transfers.remove(id);
        tracing::info!("upload {id} aborted by {}", client.id().0);
        Ok(ReplyBody::Ack {
            rev: self.store.rev(),
        })
    }

    /// `file.import`: stores a complete upload as a named session without opening it. The
    /// unpacking and renames run on a worker ([`super::file_jobs`]).
    pub(super) fn begin_import(
        &mut self,
        client: &Peer,
        id: TransferId,
        name: String,
        overwrite: bool,
    ) -> Result<(Pending, Work), ProtoError> {
        session::validate_name(&name).map_err(session_err)?;
        let target = self.s.session_dir.join(&name);
        let archive = self.transfers.claim(client, id)?.1;
        let src = UploadSource {
            archive,
            stage: self.stage_path("upload"),
            disk: self.unpack_disk(),
        };
        let aside = self.stage_path(unpack::REPLACED).join(&name);
        let root = self.s.session_dir.clone();
        let (n, t) = (name.clone(), target.clone());
        let work: Work = Box::new(move |cancel: &Cancel| {
            FileDone::Import(import_upload(
                &src, &n, &t, &root, &aside, overwrite, cancel,
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
    /// session), packed to the staging directory by a worker.
    pub(super) fn begin_download(
        &mut self,
        client: &Peer,
        r: Option<&SessionRef>,
    ) -> Result<(Pending, Work), ProtoError> {
        let disk = self.pack_disk();
        let (stem, work, working): (String, Work, _) = match r {
            None => {
                let dir = self.stage_path("download");
                let to = self.stage_path("pack");
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
                let work: Work = Box::new(move |cancel: &Cancel| {
                    let packed = cancel
                        .check()
                        .and_then(|()| prepare_stage(&dir))
                        .and_then(|()| session::save(&dir, &data).map_err(session_err))
                        .and_then(|_| cancel.check())
                        .and_then(|()| pack_session(&dir, &to, &disk));
                    remove_dir(&dir);
                    FileDone::Download(packed)
                });
                (stem, work, Some((hash, at)))
            }
            Some(SessionRef::Upload { .. }) => {
                return Err(invalid("an upload is on the client already"));
            }
            Some(r) => {
                let (name, dir) = self.session_path(client, r)?;
                let to = self.stage_path("pack");
                let work: Work = Box::new(move |cancel: &Cancel| {
                    FileDone::Download(
                        cancel
                            .check()
                            .and_then(|()| session::read_manifest(&dir).map_err(session_err))
                            .and_then(|_| pack_session(&dir, &to, &disk)),
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
        client: &Peer,
        stem: &str,
        working: Option<(u64, WallNs)>,
        packed: Result<Packed, ProtoError>,
    ) -> Result<ReplyBody, ProtoError> {
        let p = packed?;
        let file_name = format!("{stem}.{}", ac2_archive::EXTENSION);
        let viewer = self.peer_role(client) == Some(ac2_proto::model::ClientRole::Viewer);
        let transfer = self.transfers.admit(client)?;
        let spool = Spool::adopt(
            &p.path,
            self.staging().join(format!("down-{transfer}.part")),
        )
        .map_err(|e| disk_err("download", &e))?;
        self.transfers
            .insert(transfer, client, viewer, Kind::Download { spool, working });
        Ok(ReplyBody::Transfer(Transfer {
            transfer,
            file_name,
            bytes: p.bytes,
            sha256: p.sha256.clone(),
            chunk_bytes: CHUNK_BYTES,
        }))
    }

    /// Spools `data` for `client` to fetch in chunks (`file.download_chunk`), as
    /// `file_name`.
    pub(super) fn offer_download(
        &mut self,
        client: &Peer,
        file_name: String,
        data: &[u8],
    ) -> Result<ReplyBody, ProtoError> {
        let bytes = data.len() as u64;
        let viewer = self.peer_role(client) == Some(ac2_proto::model::ClientRole::Viewer);
        let transfer = self.transfers.admit(client)?;
        let dir = self.made_staging()?;
        let room = self
            .pack_disk()
            .room(&dir)
            .map_err(|e| internal(format!("free space of {STAGING}: {e}")))?;
        if room < bytes {
            return Err(refused(format!(
                "no room for {bytes} bytes: the daemon's disk is nearly full"
            )));
        }
        let mut spool = Spool::create(dir.join(format!("down-{transfer}.part")))
            .map_err(|e| disk_err("download", &e))?;
        spool.append(data).map_err(|e| disk_err("download", &e))?;
        self.transfers.insert(
            transfer,
            client,
            viewer,
            Kind::Download {
                spool,
                working: None,
            },
        );
        Ok(ReplyBody::Transfer(Transfer {
            transfer,
            file_name,
            bytes,
            sha256: ac2_archive::sha256_hex(data),
            chunk_bytes: CHUNK_BYTES,
        }))
    }

    pub(super) fn file_download_chunk(
        &mut self,
        client: &Peer,
        id: TransferId,
        offset: u64,
    ) -> Result<ReplyBody, ProtoError> {
        if let Some(r) = self.transfers.snapshots.chunk(client, id, offset) {
            return r;
        }
        let x = self.transfers.get(client, id)?;
        let Kind::Download { spool, .. } = &mut x.kind else {
            return Err(invalid(format!("transfer {id} is an upload")));
        };
        if offset > spool.len() {
            return Err(invalid(format!(
                "download {id} is {} bytes; offset {offset}",
                spool.len()
            )));
        }
        spool
            .read_at(offset, CHUNK_BYTES as usize)
            .map(|b| ReplyBody::Chunk(Blob(b)))
            .map_err(|e| internal(format!("download {id}: {e}")))
    }

    pub(super) fn file_download_end(
        &mut self,
        client: &Peer,
        id: TransferId,
        saved_as: Option<String>,
    ) -> Result<ReplyBody, ProtoError> {
        if let Some(r) = self
            .transfers
            .snapshots
            .end(client, id, saved_as.as_deref())
        {
            return r.map(|()| ReplyBody::Ack {
                rev: self.store.rev(),
            });
        }
        let x = self.transfers.get(client, id)?;
        let Kind::Download { working, .. } = &x.kind else {
            return Err(invalid(format!("transfer {id} is an upload")));
        };
        let working = *working;
        self.transfers.remove(id);
        if let (Some(path), Some((hash, at))) = (saved_as, working) {
            // The origin is mirrored to every client: the file's name only, not where on the
            // writing client's machine it lies.
            let path = client_file(&path);
            tracing::info!("working state exported by {} to {path:?}", client.id().0);
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

/// Why a chunk was not taken: the upload stays (`Refused`) or is gone (`Dropped`).
enum Chunk {
    Refused(ProtoError),
    Dropped(ProtoError),
}

/// Writes chunk `data` at `offset` of upload `kind`, the last one checked against its
/// SHA-256.
fn append_chunk(
    kind: &mut Kind,
    id: TransferId,
    offset: u64,
    data: &[u8],
    free_space: &FreeSpace,
    floor: u64,
) -> Result<(), Chunk> {
    let Kind::Upload {
        bytes,
        sha256,
        spool,
        hasher,
        ..
    } = kind
    else {
        return Err(Chunk::Refused(invalid(format!(
            "transfer {id} is a download"
        ))));
    };
    let len = data.len() as u64;
    // The offset is the client's: no sum with it may wrap.
    let Some(end) = offset.checked_add(len) else {
        return Err(Chunk::Refused(invalid(format!(
            "upload {id}: a chunk at {offset} ends past any file"
        ))));
    };
    // A chunk sent again after its reply was lost, past the dedup window: the bytes are
    // there already.
    if len > 0 && end == spool.len() {
        return match spool.holds(offset, data) {
            Ok(true) => Ok(()),
            Ok(false) => Err(Chunk::Refused(invalid(format!(
                "upload {id}: a chunk at {offset} that differs from the bytes received there"
            )))),
            Err(e) => Err(Chunk::Refused(internal(format!("upload {id}: {e}")))),
        };
    }
    if offset != spool.len() {
        return Err(Chunk::Refused(invalid(format!(
            "upload {id}: chunk at {offset}, expected at {} (chunks go in order)",
            spool.len()
        ))));
    }
    if data.len() > CHUNK_BYTES as usize || end > *bytes {
        return Err(Chunk::Refused(invalid(format!(
            "upload {id}: the chunk goes past the size announced ({bytes} bytes)"
        ))));
    }
    // Whatever else writes to the disk meanwhile (another upload, the logs), the last of it
    // is never taken by this one.
    match free_space.of(spool.path()) {
        Ok(free) if free < len.saturating_add(floor) => {
            return Err(Chunk::Dropped(refused(format!(
                "upload {id} dropped: the daemon's disk is nearly full"
            ))));
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("free space of {STAGING}: {e}"),
    }
    spool
        .append(data)
        .map_err(|e| Chunk::Dropped(disk_err(&format!("upload {id} dropped"), &e)))?;
    if let Some(h) = hasher.as_mut() {
        h.update(data);
    }
    if spool.len() == *bytes && hasher.take().map(Hasher::hex).as_deref() != Some(sha256.as_str()) {
        return Err(Chunk::Dropped(invalid(ArchiveError::Checksum.to_string())));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
