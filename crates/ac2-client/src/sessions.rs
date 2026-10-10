//! Sessions on this machine's file system, moved to and from a daemon that may be on
//! another host (`docs/design/sessions.md`): a session folder or `.ac2session` archive is
//! read here and uploaded in chunks; the working state (or a named session) is downloaded
//! in chunks, checked against its SHA-256 and written here.
//!
//! Used by the CLI and the app alike, so both move sessions the same way.

use std::path::{Path, PathBuf};

use ac2_archive::{ArchiveError, MAX_ARCHIVE_BYTES};
use ac2_proto::model::{SessionRef, Transfer};
use ac2_proto::units::{Blob, TransferId};
use ac2_proto::{Command, ReplyBody};

use crate::{Client, ClientError, expect_body};

/// A session on this machine: a session folder, or an archive file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalSession {
    /// A folder holding `session.json`.
    Folder(PathBuf),
    /// An `.ac2session` file.
    Archive(PathBuf),
}

impl LocalSession {
    /// What `path` is: a session folder or an archive (by its extension, or any file).
    /// `None` when it is neither (a folder without `session.json`, or missing).
    pub fn at(path: &Path) -> Option<Self> {
        if path.is_dir() {
            ac2_archive::is_session_dir(path).then(|| Self::Folder(path.to_owned()))
        } else if path.is_file() {
            Some(Self::Archive(path.to_owned()))
        } else {
            None
        }
    }

    /// The path.
    pub fn path(&self) -> &Path {
        match self {
            Self::Folder(p) | Self::Archive(p) => p,
        }
    }

    /// The archive bytes: the file read, or the folder packed.
    pub fn archive(&self) -> Result<Vec<u8>, ClientError> {
        match self {
            Self::Folder(p) => ac2_archive::pack_dir(p).map_err(ClientError::Archive),
            Self::Archive(p) => {
                let len = std::fs::metadata(p)
                    .map_err(|source| ClientError::Io {
                        path: p.clone(),
                        source,
                    })?
                    .len();
                if len > MAX_ARCHIVE_BYTES {
                    return Err(ClientError::Archive(ArchiveError::TooLarge {
                        bytes: len,
                        max: MAX_ARCHIVE_BYTES,
                    }));
                }
                std::fs::read(p).map_err(|source| ClientError::Io {
                    path: p.clone(),
                    source,
                })
            }
        }
    }
}

/// Uploads `local` to the daemon; returns the complete upload, for `file.load`,
/// `file.salvage` or `file.import`. `progress` gets (bytes sent, total) after each chunk.
pub async fn upload(
    c: &Client,
    local: &LocalSession,
    mut progress: impl FnMut(u64, u64),
) -> Result<TransferId, ClientError> {
    let bytes = local.archive()?;
    let t = c
        .call(Command::FileUploadBegin {
            file: local.path().to_string_lossy().into_owned(),
            bytes: bytes.len() as u64,
            sha256: ac2_archive::sha256_hex(&bytes),
        })
        .await?;
    let t = expect_body!("file.upload_begin", t, ReplyBody::Transfer(t) => t)?;
    let step = (t.chunk_bytes as usize).max(1);
    let mut offset = 0usize;
    // An empty archive cannot be valid, but the daemon says so, not this loop.
    while offset < bytes.len() {
        let end = (offset + step).min(bytes.len());
        c.call(Command::FileUploadChunk {
            transfer: t.transfer,
            offset: offset as u64,
            data: Blob(bytes[offset..end].to_vec()),
        })
        .await?;
        offset = end;
        progress(offset as u64, bytes.len() as u64);
    }
    Ok(t.transfer)
}

/// Downloads `session` (`None`: the working state) and checks it; the transfer stays open
/// for [`finish_download`].
pub async fn download(
    c: &Client,
    session: Option<SessionRef>,
    mut progress: impl FnMut(u64, u64),
) -> Result<(Transfer, Vec<u8>), ClientError> {
    let t = c.call(Command::FileDownloadBegin { session }).await?;
    let t = expect_body!("file.download_begin", t, ReplyBody::Transfer(t) => t)?;
    let mut data = Vec::with_capacity(usize::try_from(t.bytes).unwrap_or(0));
    while (data.len() as u64) < t.bytes {
        let r = c
            .call(Command::FileDownloadChunk {
                transfer: t.transfer,
                offset: data.len() as u64,
            })
            .await?;
        let chunk = expect_body!("file.download_chunk", r, ReplyBody::Chunk(b) => b)?;
        if chunk.0.is_empty() {
            break;
        }
        data.extend_from_slice(&chunk.0);
        progress(data.len() as u64, t.bytes);
    }
    if data.len() as u64 != t.bytes {
        return Err(ClientError::Archive(ArchiveError::Corrupt(format!(
            "download ended at {} of {} bytes",
            data.len(),
            t.bytes
        ))));
    }
    ac2_archive::verify(&data, &t.sha256).map_err(ClientError::Archive)?;
    Ok((t, data))
}

/// Ends a download; `saved_as`: where a working-state archive was written.
pub async fn finish_download(
    c: &Client,
    transfer: TransferId,
    saved_as: Option<&Path>,
) -> Result<(), ClientError> {
    c.call(Command::FileDownloadEnd {
        transfer,
        saved_as: saved_as.map(|p| p.to_string_lossy().into_owned()),
    })
    .await?;
    Ok(())
}

/// Where an export goes: `path` itself, or the suggested name inside it when it is a
/// folder; `.ac2session` is added to a name without it.
pub fn export_target(path: &Path, suggested: &str) -> PathBuf {
    let p = if path.is_dir() {
        path.join(suggested)
    } else {
        path.to_owned()
    };
    if ac2_archive::is_archive_name(&p) {
        p
    } else {
        let mut s = p.into_os_string();
        s.push(".");
        s.push(ac2_archive::EXTENSION);
        PathBuf::from(s)
    }
}

/// Downloads `session` (`None`: the working state) to `path` (a file, or a folder for the
/// suggested name), written whole before it replaces anything there. A working-state export
/// tells the daemon, so the working state then belongs to that file. Returns the file and
/// its size.
pub async fn export(
    c: &Client,
    session: Option<SessionRef>,
    path: &Path,
    progress: impl FnMut(u64, u64),
) -> Result<(PathBuf, u64), ClientError> {
    let working = session.is_none();
    let (t, data) = download(c, session, progress).await?;
    let target = export_target(path, &t.file_name);
    let written =
        ac2_paths::write_private_atomic(&target, &data).map_err(|source| ClientError::Io {
            path: target.clone(),
            source,
        });
    let saved = written.is_ok() && working;
    let end = finish_download(c, t.transfer, saved.then_some(target.as_path())).await;
    written?;
    end?;
    Ok((target, t.bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_names_get_the_extension() {
        let tmp = std::env::temp_dir();
        assert_eq!(
            export_target(&tmp, "friday.ac2session"),
            tmp.join("friday.ac2session")
        );
        assert_eq!(
            export_target(Path::new("/nonexistent/x/show"), "y.ac2session"),
            PathBuf::from("/nonexistent/x/show.ac2session")
        );
        assert_eq!(
            export_target(Path::new("/nonexistent/x/show.AC2SESSION"), "y.ac2session"),
            PathBuf::from("/nonexistent/x/show.AC2SESSION")
        );
    }
}
