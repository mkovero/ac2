//! The autosave's SPL logs: one file per meter in `<dir>/spl/` that grows a line per logged
//! second, so that a running meter costs the disk its new rows and nothing else.
//!
//! The write thread copies the rows logged since its last append out of the shared log
//! (holding the lock only for that copy), formats them and appends them every
//! [`FLUSH_EVERY`]; it syncs the files every [`SYNC_EVERY`], when a file is closed and at
//! shutdown. Syncing is what costs flash wear and battery, and what it protects against is
//! a power cut, after which a log loses at most its last few minutes and reads up to its
//! last whole line ([`ac2_traces::spl_log::import_csv`]).
//!
//! A file is written whole only when it starts (a new meter, a loaded session, a new log),
//! after a failed append, and when it has grown [`COMPACT_SLACK`] rows past the retention
//! (once a day of logging), each time renamed into place complete.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ac2_proto::model::SplLogPage;
use ac2_proto::units::MeasId;
use ac2_traces::session::{self, SPL_DIR, SavedMeasurement, SavedSplLogFile};
use ac2_traces::spl_log::{self, SplLogInfo};

use crate::leq_log::{self, SharedLog};

/// How often logged rows are appended to their files. The kernel holds them in memory
/// until it writes them back on its own schedule, so appending more often costs only the
/// system call; a crash of the daemon alone loses what is not appended yet.
pub(crate) const FLUSH_EVERY: Duration = Duration::from_secs(30);
/// How often the appended rows are synced to the disk: at most this much of a log is lost
/// in a power cut.
pub(crate) const SYNC_EVERY: Duration = Duration::from_secs(300);
/// Rows a file may hold past the retention before it is rewritten with the retained rows
/// only: a day of logging, so the rewrite happens once a day and a restore never reads
/// more than three days of rows.
pub(crate) const COMPACT_SLACK: u64 = 24 * 3600;

/// Where a restored log's file stands, so that appending carries on in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Resume {
    /// File, relative to the autosave directory.
    pub(crate) file: String,
    /// Rows it holds.
    pub(crate) rows: u64,
    /// Bytes up to the end of its last whole line.
    pub(crate) complete_len: u64,
}

/// An SPL meter's log for the write thread to keep on disk.
#[derive(Debug, Clone)]
pub(crate) struct LogSpec {
    pub(crate) meas: MeasId,
    pub(crate) log: SharedLog,
    /// The file header's measurement, name, input and mic.
    pub(crate) info: SplLogInfo,
    /// Set for a log restored from the autosave.
    pub(crate) resume: Option<Resume>,
}

struct LogFile {
    log: SharedLog,
    /// The log's id ([`crate::leq_log::LeqLog::id`]) the file holds.
    id: u64,
    info: SplLogInfo,
    /// The file, relative to the directory; `None` until one was written.
    rel: Option<String>,
    /// Open for appending; `None` after a failure (rewritten whole at the next append).
    file: Option<File>,
    /// Rows numbered below this are in the file.
    written: u64,
    rows_in_file: u64,
    unsynced: bool,
}

/// The write thread's SPL log files.
pub(crate) struct SplFiles {
    dir: PathBuf,
    files: BTreeMap<MeasId, LogFile>,
    next_flush: Instant,
    next_sync: Instant,
    /// Bytes written so far (whole files and appends).
    pub(crate) bytes: u64,
    /// A failure was logged and nothing has succeeded since.
    failing: bool,
}

impl SplFiles {
    pub(crate) fn new(dir: PathBuf, now: Instant) -> Self {
        Self {
            dir,
            files: BTreeMap::new(),
            next_flush: now + FLUSH_EVERY,
            next_sync: now + SYNC_EVERY,
            bytes: 0,
            failing: false,
        }
    }

    /// When the next append or sync is due; `None` without logs.
    pub(crate) fn due(&self) -> Option<Instant> {
        (!self.files.is_empty()).then(|| self.next_flush.min(self.next_sync))
    }

    /// Appends and syncs what is due at `now`.
    pub(crate) fn tick(&mut self, now: Instant) {
        if now >= self.next_flush {
            self.append_all();
            self.next_flush = now + FLUSH_EVERY;
        }
        if now >= self.next_sync {
            self.sync_all();
            self.next_sync = now + SYNC_EVERY;
        }
    }

    /// The logs to keep are `specs`: a file for each, carried on where it is the same log,
    /// others closed.
    pub(crate) fn apply(&mut self, specs: Vec<LogSpec>) {
        let gone: Vec<MeasId> = self
            .files
            .keys()
            .filter(|m| specs.iter().all(|s| s.meas != **m))
            .copied()
            .collect();
        for m in gone {
            if let Some(f) = self.files.remove(&m) {
                self.close(f);
            }
        }
        for s in specs {
            let id = leq_log::lock(&s.log).id();
            if let Some(f) = self.files.get_mut(&s.meas)
                && Arc::ptr_eq(&f.log, &s.log)
                && f.id == id
            {
                f.info = s.info;
                continue;
            }
            if let Some(old) = self.files.remove(&s.meas) {
                self.close(old);
            }
            let mut f = LogFile {
                log: s.log,
                id,
                info: s.info,
                rel: None,
                file: None,
                written: 0,
                rows_in_file: 0,
                unsynced: false,
            };
            let resumed = s.resume.is_some_and(|r| self.resume(&mut f, r));
            if !resumed {
                self.start(s.meas, &mut f);
            }
            self.files.insert(s.meas, f);
        }
    }

    /// Carries on appending to a restored log's file: cut back to its last whole line.
    fn resume(&mut self, f: &mut LogFile, r: Resume) -> bool {
        let total = leq_log::lock(&f.log).total();
        if total < r.rows {
            return false;
        }
        let p = self.dir.join(&r.file);
        let opened = OpenOptions::new().append(true).open(&p).and_then(|file| {
            if file.metadata()?.len() < r.complete_len {
                return Err(std::io::Error::other("shorter than when it was read"));
            }
            file.set_len(r.complete_len)?;
            Ok(file)
        });
        match opened {
            Ok(file) => {
                f.file = Some(file);
                f.rel = Some(r.file);
                f.written = r.rows;
                f.rows_in_file = r.rows;
                true
            }
            Err(e) => {
                tracing::warn!(
                    "autosave: SPL log {} cannot be appended to ({e}); writing it anew",
                    p.display()
                );
                false
            }
        }
    }

    /// A new file for the log of `f`, written whole.
    fn start(&mut self, meas: MeasId, f: &mut LogFile) {
        let rel = self.new_name(meas);
        f.rel = None;
        f.file = None;
        self.rewrite(f, rel);
    }

    /// `spl/<meas>-<wall ns>.csv`, not taken.
    fn new_name(&self, meas: MeasId) -> String {
        let mut t = crate::util::wall_ns();
        loop {
            let rel = format!("{SPL_DIR}/{}-{t}.csv", meas.0);
            if !self.dir.join(&rel).exists()
                && self.files.values().all(|f| f.rel.as_ref() != Some(&rel))
            {
                return rel;
            }
            t += 1;
        }
    }

    /// Writes the whole log held to `rel` (renamed into place complete) and opens it for
    /// appending.
    fn rewrite(&mut self, f: &mut LogFile, rel: String) {
        let (id, total, rows) = {
            let l = leq_log::lock(&f.log);
            (l.id(), l.total(), l.rows())
        };
        let text = spl_log::export_csv(&f.info, &rows);
        let p = self.dir.join(&rel);
        let spl_dir = self.dir.join(SPL_DIR);
        let result = fs::create_dir_all(&spl_dir)
            .map_err(|e| e.to_string())
            .and_then(|()| session::write_atomic(&p, text.as_bytes()).map_err(|e| e.to_string()))
            .and_then(|()| {
                OpenOptions::new()
                    .append(true)
                    .open(&p)
                    .map_err(|e| format!("{}: {e}", p.display()))
            });
        match result {
            Ok(file) => {
                self.bytes += text.len() as u64;
                f.id = id;
                f.rel = Some(rel);
                f.file = Some(file);
                f.written = total;
                f.rows_in_file = rows.len() as u64;
                // `write_atomic` synced it.
                f.unsynced = false;
                self.succeeded();
            }
            Err(e) => self.failed(&format!("cannot write SPL log: {e}")),
        }
    }

    fn succeeded(&mut self) {
        if self.failing {
            tracing::info!("autosave: SPL logs are written again");
            self.failing = false;
        }
    }

    fn failed(&mut self, what: &str) {
        if !self.failing {
            tracing::warn!(
                "autosave: {what}; retried every {} s",
                FLUSH_EVERY.as_secs()
            );
            self.failing = true;
        }
    }

    /// Appends every log's new rows.
    pub(crate) fn append_all(&mut self) {
        let metas: Vec<MeasId> = self.files.keys().copied().collect();
        for m in metas {
            if let Some(mut f) = self.files.remove(&m) {
                self.append(m, &mut f);
                self.files.insert(m, f);
            }
        }
    }

    fn append(&mut self, meas: MeasId, f: &mut LogFile) {
        let (id, total, rows) = {
            let l = leq_log::lock(&f.log);
            let rows = if l.id() == f.id {
                l.rows_from(f.written)
            } else {
                Vec::new()
            };
            (l.id(), l.total(), rows)
        };
        if id != f.id {
            // `spl.log_new` replaced the log: the ended one's file is closed as it stands
            // (only the current log is restored) and the new log gets its own.
            if let Some(file) = f.file.take()
                && f.unsynced
            {
                let _ = file.sync_all();
            }
            f.unsynced = false;
            self.start(meas, f);
            return;
        }
        let Some(file) = f.file.as_mut() else {
            match f.rel.clone() {
                Some(rel) => self.rewrite(f, rel),
                None => self.start(meas, f),
            }
            return;
        };
        if rows.is_empty() {
            return;
        }
        let mut text = String::with_capacity(rows.len() * 100);
        for r in &rows {
            spl_log::push_row(&mut text, r);
        }
        match file.write_all(text.as_bytes()) {
            Ok(()) => {
                self.bytes += text.len() as u64;
                f.written = total;
                f.rows_in_file += rows.len() as u64;
                f.unsynced = true;
                self.succeeded();
            }
            Err(e) => {
                // What reached the file may end in part of a line: the next append
                // rewrites the file whole.
                f.file = None;
                self.failed(&format!("cannot append to SPL log: {e}"));
                return;
            }
        }
        if f.rows_in_file > SplLogPage::RETAINED_ROWS as u64 + COMPACT_SLACK
            && let Some(rel) = f.rel.clone()
        {
            self.rewrite(f, rel);
        }
    }

    fn sync_all(&mut self) {
        for f in self.files.values_mut() {
            if f.unsynced
                && let Some(file) = &f.file
            {
                let _ = file.sync_all();
                f.unsynced = false;
            }
        }
    }

    fn close(&mut self, mut f: LogFile) {
        if f.file.is_some() && leq_log::lock(&f.log).id() == f.id {
            let meas = f.info.meas;
            self.append(meas, &mut f);
        }
        if let Some(file) = &f.file
            && f.unsynced
        {
            let _ = file.sync_all();
        }
    }

    /// Shutdown: every log's rows appended and synced.
    pub(crate) fn close_all(&mut self) {
        let files = std::mem::take(&mut self.files);
        for (_, f) in files {
            self.close(f);
        }
    }

    /// The files of the logs of `measurements`, for the manifest.
    pub(crate) fn linked(&self, measurements: &[SavedMeasurement]) -> Vec<SavedSplLogFile> {
        self.files
            .iter()
            .filter(|(m, _)| measurements.iter().any(|sm| sm.id == **m))
            .filter_map(|(m, f)| {
                f.rel.as_ref().map(|rel| SavedSplLogFile {
                    meas: *m,
                    file: rel.clone(),
                })
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn path_of(&self, meas: MeasId) -> Option<PathBuf> {
        self.files
            .get(&meas)
            .and_then(|f| f.rel.as_ref())
            .map(|r| self.dir.join(r))
    }
}
