//! Session archives (`.ac2session`): a session directory (`ac2_traces::session`) as one
//! gzip-compressed tar, so a session can travel as a single file — from a USB stick on the
//! machine running the app to a daemon on another host, and back.
//!
//! An archive holds only the files a session directory can hold, at their places:
//!
//! ```text
//! session.json  session.prev.json  traces/<file>  spl/<file>
//! ```
//!
//! [`pack_dir`] writes them in name order with fixed metadata (mode 0644, time 0), so the same
//! directory always packs to the same bytes. [`unpack`] accepts nothing else: no links, no
//! absolute or `..` paths, no hidden names, so an archive cannot write outside the
//! directory it is unpacked into, and it stops at [`MAX_UNPACKED_BYTES`] and [`MAX_ENTRIES`]
//! (a small archive may expand to a lot). The archive does not look inside `session.json`:
//! a session of another format version packs and unpacks the same (and can then be
//! refused by version, or its traces salvaged).
//!
//! No DSP and no I/O beyond the paths given: the app (which links no DSP) packs and
//! checks archives with it as the daemon does.

#![forbid(unsafe_code)]

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// File name extension of an archive.
pub const EXTENSION: &str = "ac2session";
/// Largest archive accepted, bytes (compressed).
pub const MAX_ARCHIVE_BYTES: u64 = 64 << 20;
/// Largest total of the files an archive unpacks to, bytes.
pub const MAX_UNPACKED_BYTES: u64 = 512 << 20;
/// Most entries an archive may hold.
pub const MAX_ENTRIES: usize = 20_000;
/// The manifest every session directory has.
pub const MANIFEST: &str = "session.json";
const TOP_FILES: [&str; 2] = [MANIFEST, "session.prev.json"];
const SUBDIRS: [&str; 2] = ["traces", "spl"];

/// Why an archive could not be packed or unpacked.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ArchiveError {
    /// The directory has no `session.json`.
    #[error("{0} is not an ac2 session (no session.json)")]
    NotASession(PathBuf),
    /// The archive is larger than allowed.
    #[error("the session archive is {bytes} bytes; at most {max} are accepted")]
    TooLarge {
        /// Size found (for unpacked contents: the size reached when it stopped).
        bytes: u64,
        /// The limit.
        max: u64,
    },
    /// More entries than allowed.
    #[error("the session archive has more than {MAX_ENTRIES} entries")]
    TooManyEntries,
    /// An entry that has no place in a session directory.
    #[error("the session archive holds {0:?}, which is not part of a session")]
    Foreign(String),
    /// No `session.json` in the archive.
    #[error("the archive holds no session.json: not an ac2 session")]
    NoManifest,
    /// Not a gzip-compressed tar, or cut short.
    #[error("not a readable session archive: {0}")]
    Corrupt(String),
    /// The content does not match its SHA-256.
    #[error("the session archive does not match its checksum (damaged in transfer)")]
    Checksum,
    /// The file system refused.
    #[error("{path}: {msg}")]
    Io {
        /// Where.
        path: PathBuf,
        /// What the OS said.
        msg: String,
    },
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> ArchiveError + '_ {
    move |e| ArchiveError::Io {
        path: path.to_owned(),
        msg: e.to_string(),
    }
}

/// SHA-256 of `bytes`, lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Checks `bytes` against the SHA-256 `hex` (case-insensitive).
pub fn verify(bytes: &[u8], hex: &str) -> Result<(), ArchiveError> {
    if sha256_hex(bytes).eq_ignore_ascii_case(hex.trim()) {
        Ok(())
    } else {
        Err(ArchiveError::Checksum)
    }
}

/// Whether `dir` is a session directory (holds a `session.json`).
pub fn is_session_dir(dir: &Path) -> bool {
    dir.join(MANIFEST).is_file()
}

/// Whether `path` names an archive by its extension.
pub fn is_archive_name(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION))
}

/// A file name allowed inside a session directory: plain, not hidden.
fn plain_name(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('.') && !s.contains(['/', '\\', '\0'])
}

/// The session files of `dir` as (archive path, file path), in name order.
fn session_files(dir: &Path) -> Result<Vec<(String, PathBuf)>, ArchiveError> {
    let mut out = Vec::new();
    for f in TOP_FILES {
        let p = dir.join(f);
        if p.is_file() {
            out.push((f.to_owned(), p));
        }
    }
    for sub in SUBDIRS {
        let d = dir.join(sub);
        let rd = match fs::read_dir(&d) {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(io_err(&d)(e)),
        };
        let mut names = Vec::new();
        for e in rd {
            let e = e.map_err(io_err(&d))?;
            let name = e.file_name().to_string_lossy().into_owned();
            // A session holds regular files only; anything else in these folders (a
            // temporary file of an interrupted write, a link) is not part of it.
            let is_file = e.file_type().map_err(io_err(&d))?.is_file();
            if is_file && plain_name(&name) {
                names.push(name);
            }
        }
        names.sort();
        out.extend(names.into_iter().map(|n| (format!("{sub}/{n}"), d.join(n))));
    }
    Ok(out)
}

/// Packs the session directory `dir` into an archive. Refused when it has no
/// `session.json` or packs larger than [`MAX_ARCHIVE_BYTES`].
pub fn pack_dir(dir: &Path) -> Result<Vec<u8>, ArchiveError> {
    if !is_session_dir(dir) {
        return Err(ArchiveError::NotASession(dir.to_owned()));
    }
    let files = session_files(dir)?;
    if files.len() > MAX_ENTRIES {
        return Err(ArchiveError::TooManyEntries);
    }
    let mut tar = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    let mut total = 0u64;
    for (name, path) in &files {
        let data = fs::read(path).map_err(io_err(path))?;
        total += data.len() as u64;
        if total > MAX_UNPACKED_BYTES {
            return Err(ArchiveError::TooLarge {
                bytes: total,
                max: MAX_UNPACKED_BYTES,
            });
        }
        let mut h = tar::Header::new_ustar();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_mtime(0);
        h.set_entry_type(tar::EntryType::Regular);
        tar.append_data(&mut h, name, data.as_slice())
            .map_err(io_err(path))?;
    }
    let gz = tar.into_inner().map_err(io_err(dir))?;
    let bytes = gz.finish().map_err(io_err(dir))?;
    if bytes.len() as u64 > MAX_ARCHIVE_BYTES {
        return Err(ArchiveError::TooLarge {
            bytes: bytes.len() as u64,
            max: MAX_ARCHIVE_BYTES,
        });
    }
    Ok(bytes)
}

/// Where an entry goes inside the session directory, or why it has no place there.
fn entry_place(path: &Path, is_dir: bool) -> Result<Option<PathBuf>, ArchiveError> {
    let foreign = || ArchiveError::Foreign(path.to_string_lossy().into_owned());
    let mut parts = Vec::new();
    for c in path.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_str().ok_or_else(foreign)?.to_owned()),
            Component::CurDir => {}
            _ => return Err(foreign()),
        }
    }
    match (parts.as_slice(), is_dir) {
        ([d], true) if SUBDIRS.contains(&d.as_str()) => Ok(None),
        ([f], false) if TOP_FILES.contains(&f.as_str()) => Ok(Some(PathBuf::from(f))),
        ([d, f], false) if SUBDIRS.contains(&d.as_str()) && plain_name(f) => {
            Ok(Some(Path::new(d).join(f)))
        }
        _ => Err(foreign()),
    }
}

/// Counts what goes through, and stops a stream larger than `max`.
struct Limited<R> {
    inner: R,
    read: u64,
    max: u64,
}

impl<R: Read> Read for Limited<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read += n as u64;
        if self.read > self.max {
            return Err(io::Error::other(LIMIT));
        }
        Ok(n)
    }
}

const LIMIT: &str = "unpacked size limit";

/// A read error: the size limit stopping the stream, or a damaged archive.
fn read_err(e: io::Error) -> ArchiveError {
    if e.get_ref().is_some_and(|i| i.to_string() == LIMIT) {
        ArchiveError::TooLarge {
            bytes: MAX_UNPACKED_BYTES,
            max: MAX_UNPACKED_BYTES,
        }
    } else {
        ArchiveError::Corrupt(e.to_string())
    }
}

/// Unpacks the archive `bytes` into `into`, which must not exist yet (it is created). On an
/// error, whatever was written stays for the caller to remove.
pub fn unpack(bytes: &[u8], into: &Path) -> Result<(), ArchiveError> {
    if bytes.len() as u64 > MAX_ARCHIVE_BYTES {
        return Err(ArchiveError::TooLarge {
            bytes: bytes.len() as u64,
            max: MAX_ARCHIVE_BYTES,
        });
    }
    fs::create_dir(into).map_err(io_err(into))?;
    // The tar framing is counted with the content: a few KiB over the files' total.
    let limited = Limited {
        inner: GzDecoder::new(bytes),
        read: 0,
        max: MAX_UNPACKED_BYTES + (MAX_ENTRIES as u64) * 1024,
    };
    let mut ar = tar::Archive::new(limited);
    let mut entries = 0usize;
    let mut total = 0u64;
    let mut manifest = false;
    let mut made = Vec::new();
    for entry in ar.entries().map_err(read_err)? {
        let mut entry = entry.map_err(read_err)?;
        entries += 1;
        if entries > MAX_ENTRIES {
            return Err(ArchiveError::TooManyEntries);
        }
        let kind = entry.header().entry_type();
        let path = entry
            .path()
            .map_err(|e| ArchiveError::Corrupt(e.to_string()))?
            .into_owned();
        let is_dir = match kind {
            tar::EntryType::Regular => false,
            tar::EntryType::Directory => true,
            _ => {
                return Err(ArchiveError::Foreign(path.to_string_lossy().into_owned()));
            }
        };
        let Some(rel) = entry_place(&path, is_dir)? else {
            continue;
        };
        total += entry.header().size().unwrap_or(0);
        if total > MAX_UNPACKED_BYTES {
            return Err(ArchiveError::TooLarge {
                bytes: total,
                max: MAX_UNPACKED_BYTES,
            });
        }
        if let Some(parent) = rel.parent().filter(|p| !p.as_os_str().is_empty())
            && !made.contains(&parent.to_owned())
        {
            let d = into.join(parent);
            fs::create_dir_all(&d).map_err(io_err(&d))?;
            made.push(parent.to_owned());
        }
        let target = into.join(&rel);
        if target.exists() {
            return Err(ArchiveError::Corrupt(format!(
                "{} appears twice",
                rel.display()
            )));
        }
        let mut f = fs::File::create(&target).map_err(io_err(&target))?;
        io::copy(&mut entry, &mut f).map_err(read_err)?;
        f.flush().map_err(io_err(&target))?;
        manifest |= rel == Path::new(MANIFEST);
    }
    if manifest {
        Ok(())
    } else {
        Err(ArchiveError::NoManifest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(dir: &Path) {
        fs::create_dir_all(dir.join("traces")).expect("dir");
        fs::create_dir_all(dir.join("spl")).expect("dir");
        fs::write(
            dir.join(MANIFEST),
            br#"{"format":"ac2-session","version":20}"#,
        )
        .expect("w");
        fs::write(dir.join("traces/1-abc.csv"), b"# ac2 trace\n1,2\n").expect("w");
        fs::write(dir.join("spl/Main.csv"), b"t,la\n").expect("w");
        // Not part of a session: skipped by pack.
        fs::write(dir.join("traces/.tmp-1"), b"x").expect("w");
        fs::write(dir.join("notes.txt"), b"x").expect("w");
    }

    fn tar_gz(entries: &[(&str, tar::EntryType, &[u8])]) -> Vec<u8> {
        let mut t = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        for (name, kind, data) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(*kind);
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            // Bypass the builder's own path checks: hostile names go in raw.
            let raw = &mut h.as_old_mut().name;
            raw[..name.len()].copy_from_slice(name.as_bytes());
            if *kind == tar::EntryType::Symlink {
                h.set_link_name("/etc/passwd").expect("link");
            }
            h.set_cksum();
            t.append(&h, *data).expect("append");
        }
        t.into_inner().expect("tar").finish().expect("gz")
    }

    #[test]
    fn a_session_round_trips_and_packs_reproducibly() {
        let tmp = tempfile::tempdir().expect("tmp");
        let src = tmp.path().join("show");
        session(&src);
        let a = pack_dir(&src).expect("pack");
        assert_eq!(a, pack_dir(&src).expect("pack again"));
        let out = tmp.path().join("out");
        unpack(&a, &out).expect("unpack");
        assert_eq!(
            fs::read(out.join("traces/1-abc.csv")).expect("r"),
            b"# ac2 trace\n1,2\n"
        );
        assert!(out.join("spl/Main.csv").is_file());
        assert!(!out.join("notes.txt").exists());
        assert!(!out.join("traces/.tmp-1").exists());
        verify(&a, &sha256_hex(&a)).expect("checksum");
        assert_eq!(verify(&a, &"0".repeat(64)), Err(ArchiveError::Checksum));
    }

    #[test]
    fn a_folder_without_a_manifest_is_not_a_session() {
        let tmp = tempfile::tempdir().expect("tmp");
        assert!(matches!(
            pack_dir(tmp.path()),
            Err(ArchiveError::NotASession(_))
        ));
        let a = tar_gz(&[("traces/a.csv", tar::EntryType::Regular, b"x")]);
        assert_eq!(
            unpack(&a, &tmp.path().join("o")),
            Err(ArchiveError::NoManifest)
        );
    }

    #[test]
    fn nothing_escapes_the_target_directory() {
        let tmp = tempfile::tempdir().expect("tmp");
        for (i, (name, kind)) in [
            ("../evil", tar::EntryType::Regular),
            ("/etc/evil", tar::EntryType::Regular),
            ("traces/../../evil", tar::EntryType::Regular),
            ("traces/link", tar::EntryType::Symlink),
            ("other/x.csv", tar::EntryType::Regular),
            ("traces/.hidden", tar::EntryType::Regular),
            ("traces/sub/x", tar::EntryType::Regular),
        ]
        .into_iter()
        .enumerate()
        {
            let a = tar_gz(&[
                (MANIFEST, tar::EntryType::Regular, b"{}"),
                (name, kind, b"x"),
            ]);
            let r = unpack(&a, &tmp.path().join(format!("o{i}")));
            assert!(matches!(r, Err(ArchiveError::Foreign(_))), "{name}: {r:?}");
        }
        assert!(!tmp.path().join("evil").exists());
    }

    #[test]
    fn garbage_and_oversize_are_refused() {
        let tmp = tempfile::tempdir().expect("tmp");
        assert!(matches!(
            unpack(b"not an archive", &tmp.path().join("a")),
            Err(ArchiveError::Corrupt(_))
        ));
        let big = vec![0u8; MAX_ARCHIVE_BYTES as usize + 1];
        assert!(matches!(
            unpack(&big, &tmp.path().join("b")),
            Err(ArchiveError::TooLarge { .. })
        ));
    }

    #[test]
    fn a_bomb_stops_at_the_unpacked_limit() {
        // Zeros compress about 1000:1: a few hundred KiB claim more than the limit.
        struct Zeros(u64);
        impl Read for Zeros {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let n = (buf.len() as u64).min(self.0) as usize;
                buf[..n].fill(0);
                self.0 -= n as u64;
                Ok(n)
            }
        }
        let mut t = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::best()));
        let mut h = tar::Header::new_ustar();
        h.set_size(2);
        h.set_mode(0o644);
        t.append_data(&mut h, MANIFEST, &b"{}"[..]).expect("m");
        let size = MAX_UNPACKED_BYTES + 1;
        let mut h = tar::Header::new_ustar();
        h.set_size(size);
        h.set_mode(0o644);
        t.append_data(&mut h, "traces/big.csv", Zeros(size))
            .expect("big");
        let a = t.into_inner().expect("tar").finish().expect("gz");
        assert!((a.len() as u64) < MAX_ARCHIVE_BYTES);
        let tmp = tempfile::tempdir().expect("tmp");
        assert!(matches!(
            unpack(&a, &tmp.path().join("o")),
            Err(ArchiveError::TooLarge { .. })
        ));
    }
}
