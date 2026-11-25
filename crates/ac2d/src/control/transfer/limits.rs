//! What transfers may take: their number, their lifetime and the disk they spool to.

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Free bytes on the file system holding a path ([`ac2_paths::free_space`] on a running
/// daemon); a test sets a figure to reach the refusals without filling a disk.
#[derive(Clone)]
pub struct FreeSpace(Arc<FreeSpaceFn>);

type FreeSpaceFn = dyn Fn(&Path) -> io::Result<u64> + Send + Sync;

impl FreeSpace {
    /// The host's file systems.
    pub fn host() -> Self {
        Self(Arc::new(ac2_paths::free_space))
    }

    /// Asks `f`.
    pub fn with(f: impl Fn(&Path) -> io::Result<u64> + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    pub(crate) fn of(&self, path: &Path) -> io::Result<u64> {
        (self.0)(path)
    }
}

impl std::fmt::Debug for FreeSpace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FreeSpace")
    }
}

/// Limits of the session transfers (`file.upload_*`, `file.download_*`, `trace.export`).
#[derive(Clone, Debug)]
pub struct TransferLimits {
    /// Longest a transfer lives however often it is touched
    /// ([`TransferLimits::LIFETIME`]).
    pub lifetime: Duration,
    /// What an upload leaves free on the session directory's file system: the logs and the
    /// autosave write there, and no upload may take their room ([`TransferLimits::RESERVE`]).
    pub reserve: u64,
    /// What a download (or a chunk of an upload under way) leaves free: a meter whose card
    /// is nearly full must still be able to hand its session off
    /// ([`TransferLimits::PACK_RESERVE`]).
    pub pack_reserve: u64,
    /// How free space is read.
    pub free_space: FreeSpace,
}

impl TransferLimits {
    /// 1 GiB, the largest archive, in an hour is about 2.4 Mbit/s: slower than any link a
    /// session goes over, yet a client trickling a chunk now and then gives its slots back.
    pub const LIFETIME: Duration = Duration::from_secs(60 * 60);
    /// 1 GiB.
    pub const RESERVE: u64 = 1 << 30;
    /// 64 MiB.
    pub const PACK_RESERVE: u64 = 64 << 20;
}

impl Default for TransferLimits {
    fn default() -> Self {
        Self {
            lifetime: Self::LIFETIME,
            reserve: Self::RESERVE,
            pack_reserve: Self::PACK_RESERVE,
            free_space: FreeSpace::host(),
        }
    }
}

/// The free space a disk write checks against, cloned into a worker.
#[derive(Clone, Debug)]
pub(crate) struct Disk {
    pub(crate) free_space: FreeSpace,
    /// Bytes kept free.
    pub(crate) keep: u64,
}

impl Disk {
    /// Bytes that may be written under `dir` and still leave [`Self::keep`] free.
    pub(crate) fn room(&self, dir: &Path) -> io::Result<u64> {
        Ok(self.free_space.of(dir)?.saturating_sub(self.keep))
    }
}
