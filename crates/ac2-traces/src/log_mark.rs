//! The high-water mark beside an SPL log (`<log>.mark`, `docs/design/protocol-next.md` §7):
//! the highest run started and an anchor tying a second index of a run to a reading of the
//! boot clock, the index reserved ahead of the last row served. A daemon that starts again
//! takes its first index above the mark, so a `(run, second)` a client was served is never
//! reused by a different row, whether or not that row reached the disk.
//!
//! ```json
//! {"format": "ac2-log-mark", "version": 1, "highest_run": 3,
//!  "anchor": {"boot_id": "6f1c…", "run": 3, "second": 86400, "boot_ms": 912345678}}
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::session::{SessionError, write_atomic};

/// `format` of every mark file.
pub const FORMAT: &str = "ac2-log-mark";
/// The version this build writes and reads.
pub const VERSION: u32 = 1;

/// Where a run's indices stand on the boot clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    /// The boot the reading is of: the kernel's boot id on Linux, a random id per daemon
    /// process elsewhere (a new process counts as a new boot there).
    pub boot_id: String,
    /// The run.
    pub run: u32,
    /// A second index of the run: reserved ahead of the last row served, or that row
    /// itself after a clean stop.
    pub second: u64,
    /// The boot clock's reading where second `second` ends, ms since boot (suspend
    /// included).
    pub boot_ms: u64,
}

impl Anchor {
    /// The boot clock's reading where index `second` of the same run ends, ms: within a run the
    /// index advances with the audio clock, which stays within 2 s of the boot clock (a
    /// larger drift ends the run).
    pub fn boot_ms_at(&self, second: u64) -> u64 {
        let back = self.second.saturating_sub(second) * 1000;
        let ahead = second.saturating_sub(self.second) * 1000;
        (self.boot_ms + ahead).saturating_sub(back)
    }
}

/// A log's high-water mark.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogMark {
    /// Always [`FORMAT`].
    pub format: String,
    /// Always [`VERSION`].
    pub version: u32,
    /// The highest run started.
    pub highest_run: u32,
    /// Where the indices stand.
    pub anchor: Anchor,
}

impl LogMark {
    /// A mark of this version.
    pub fn new(highest_run: u32, anchor: Anchor) -> Self {
        Self {
            format: FORMAT.to_owned(),
            version: VERSION,
            highest_run,
            anchor,
        }
    }

    /// Whether the mark lies at or above a log whose last row is `(second, run)`: one
    /// below it was written before rows it does not cover, so it bounds nothing.
    pub fn covers(&self, second: u64, run: u32) -> bool {
        self.highest_run >= run
            && (self.anchor.run > run || (self.anchor.run == run && self.anchor.second >= second))
    }
}

/// The mark file of the log at `log`: `<log>.mark` beside it.
pub fn path_of(log: &Path) -> PathBuf {
    let mut name = log.file_name().unwrap_or_default().to_os_string();
    name.push(".mark");
    log.with_file_name(name)
}

/// The mark's file name of a log named `rel` (a session-relative path).
pub fn rel_of(rel: &str) -> String {
    format!("{rel}.mark")
}

/// Writes `m` to `path` whole (temporary file, synced, renamed, directory synced).
pub fn write(path: &Path, m: &LogMark) -> Result<(), SessionError> {
    let json = serde_json::to_vec(m).map_err(|e| SessionError::Corrupt {
        path: path.to_owned(),
        msg: e.to_string(),
    })?;
    write_atomic(path, &json)
}

/// What reading a mark found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkRead {
    /// A mark of this version.
    Read(LogMark),
    /// No mark file.
    Missing,
    /// A file that is not a mark of this version, and why.
    Unreadable(String),
}

/// Reads the mark at `path`.
pub fn read(path: &Path) -> MarkRead {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return MarkRead::Missing,
        Err(e) => return MarkRead::Unreadable(e.to_string()),
    };
    match serde_json::from_slice::<LogMark>(&bytes) {
        Ok(m) if m.format == FORMAT && m.version == VERSION => MarkRead::Read(m),
        Ok(m) => MarkRead::Unreadable(format!("{} version {}", m.format, m.version)),
        Err(e) => MarkRead::Unreadable(e.to_string()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn mark() -> LogMark {
        LogMark::new(
            3,
            Anchor {
                boot_id: "b".into(),
                run: 3,
                second: 1600,
                boot_ms: 900_000,
            },
        )
    }

    #[test]
    fn a_mark_round_trips_and_covers_what_lies_below_it() {
        let dir = tempfile::tempdir().unwrap();
        let p = path_of(&dir.path().join("1-4.csv"));
        assert_eq!(p.file_name().unwrap(), "1-4.csv.mark");
        assert_eq!(read(&p), MarkRead::Missing);
        write(&p, &mark()).unwrap();
        assert_eq!(read(&p), MarkRead::Read(mark()));
        std::fs::write(&p, b"{\"format\":\"ac2-log-mark\"").unwrap();
        assert!(matches!(read(&p), MarkRead::Unreadable(_)));
        let m = mark();
        assert!(m.covers(1600, 3) && m.covers(1, 3) && m.covers(99_999, 2));
        assert!(!m.covers(1601, 3) && !m.covers(1, 4));
        assert_eq!(m.anchor.boot_ms_at(1000), 300_000);
        assert_eq!(m.anchor.boot_ms_at(1601), 901_000);
    }
}
