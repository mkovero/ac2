//! A transfer's bytes on disk: an upload written chunk by chunk as it arrives, a download
//! packed once and read chunk by chunk. The file goes with its transfer, however that ends.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// A spool file, removed when dropped.
#[derive(Debug)]
pub(crate) struct Spool {
    path: PathBuf,
    /// Closed before the file is removed: Windows refuses to delete an open file.
    file: Option<File>,
    len: u64,
}

impl Spool {
    /// A new, empty spool file at `path`; one left there is an error, never reused.
    pub(crate) fn create(path: PathBuf) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok(Self {
            path,
            file: Some(file),
            len: 0,
        })
    }

    /// The finished file at `from`, moved to `path` and owned from now on (removed when
    /// this goes). On an error `from` is removed.
    pub(crate) fn adopt(from: &Path, path: PathBuf) -> io::Result<Self> {
        let opened = fs::rename(from, &path).and_then(|()| File::open(&path));
        match opened {
            Ok(file) => {
                let len = file.metadata()?.len();
                Ok(Self {
                    path,
                    file: Some(file),
                    len,
                })
            }
            Err(e) => {
                remove(from);
                remove(&path);
                Err(e)
            }
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn len(&self) -> u64 {
        self.len
    }

    fn file(&mut self) -> io::Result<&mut File> {
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("the spool file is closed"))
    }

    /// Appends `data` at the end.
    pub(crate) fn append(&mut self, data: &[u8]) -> io::Result<()> {
        let end = self.len;
        let f = self.file()?;
        f.seek(SeekFrom::Start(end))?;
        if let Err(e) = f.write_all(data) {
            // A partial write leaves bytes past `len`: cut back, so the spool holds exactly
            // what was acknowledged.
            let _ = f.set_len(end);
            return Err(e);
        }
        self.len = end + data.len() as u64;
        Ok(())
    }

    /// Up to `max` bytes from `offset`.
    pub(crate) fn read_at(&mut self, offset: u64, max: usize) -> io::Result<Vec<u8>> {
        let len = self.len;
        let n = usize::try_from(len.saturating_sub(offset)).map_or(max, |n| n.min(max));
        let f = self.file()?;
        f.seek(SeekFrom::Start(offset))?;
        let mut out = vec![0u8; n];
        f.read_exact(&mut out)?;
        Ok(out)
    }

    /// Whether the bytes at `offset` are `data`: a chunk sent again after its reply was
    /// lost.
    pub(crate) fn holds(&mut self, offset: u64, data: &[u8]) -> io::Result<bool> {
        if offset + data.len() as u64 > self.len {
            return Ok(false);
        }
        Ok(self.read_at(offset, data.len())? == data)
    }
}

impl Drop for Spool {
    fn drop(&mut self) {
        drop(self.file.take());
        remove(&self.path);
    }
}

/// Removes file `p`; one already gone is fine.
pub(crate) fn remove(p: &Path) {
    if let Err(e) = fs::remove_file(p)
        && e.kind() != io::ErrorKind::NotFound
    {
        tracing::warn!("{}: not removed: {e}", p.display());
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn a_spool_appends_reads_back_and_goes_with_its_owner() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("up-1.part");
        let mut s = Spool::create(p.clone()).unwrap();
        assert!(Spool::create(p.clone()).is_err(), "never reused");
        s.append(b"hello ").unwrap();
        s.append(b"world").unwrap();
        assert_eq!(s.len(), 11);
        assert_eq!(s.read_at(6, 100).unwrap(), b"world");
        assert!(s.holds(6, b"world").unwrap());
        assert!(!s.holds(6, b"wordl").unwrap());
        assert!(!s.holds(8, b"world").unwrap());
        drop(s);
        assert!(!p.exists());
    }

    #[test]
    fn an_adopted_file_moves_and_goes_with_its_owner() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("pack.part");
        fs::write(&from, b"archive").unwrap();
        let to = dir.path().join("down-2.part");
        let mut s = Spool::adopt(&from, to.clone()).unwrap();
        assert!(!from.exists());
        assert_eq!(s.len(), 7);
        assert_eq!(s.read_at(0, 3).unwrap(), b"arc");
        drop(s);
        assert!(!to.exists());
        assert!(Spool::adopt(&from, to).is_err());
    }
}
