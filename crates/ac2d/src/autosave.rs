//! Autosave of the measurements and traces, in the session file format
//! (`ac2_traces::session`), so that a daemon restart (a new `--max-level`, a crash, a reboot)
//! does not lose them.
//!
//! ```text
//! <dir>/session.json        the current autosave's manifest
//! <dir>/session.prev.json   the one before it, kept as a backup
//! <dir>/traces/, <dir>/spl/ the files either names
//! <dir>.v<N>                an autosave of session format N that this build cannot read, set aside
//! <dir>.damaged             an unreadable autosave, set aside
//! <dir>.unrestored          the autosave a `--no-restore` start did not load
//! ```
//!
//! The directory is updated in place ([`session::save_autosave`]): a write adds the trace
//! files that are not there yet (named by their content, so an unchanged trace is never
//! written again), then the current manifest becomes the previous one and the new manifest
//! is renamed into place. A failed or interrupted write therefore leaves the last good
//! manifest, and a damaged current manifest falls back to the previous one.
//!
//! Each SPL meter's log is a file the write thread appends to ([`spl_files`]); the
//! manifest names it and a log's growth is not a change to write. What a running meter
//! costs the disk is its new rows.
//!
//! Writes run on their own thread: a session with long sweeps holds megabytes of impulse
//! response, and the control thread must keep answering while that reaches the disk.

mod spl_files;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ac2_proto::model::TraceMeta;
use ac2_proto::units::MeasId;
use ac2_traces::session::{
    self, MANIFEST, PREV_MANIFEST, SavedMeasurement, SavedSplLogFile, Session as SessionData,
    SessionError, SplLogOnDisk,
};

use self::spl_files::SplFiles;
pub(crate) use self::spl_files::{LogSpec, Resume};
use crate::control::ControlMsg;

/// Quiet time after the last change before a write: a burst of edits (a slot moved, a
/// rename, a delay typed) becomes one write.
pub(crate) const DEBOUNCE: Duration = Duration::from_millis(1500);
/// Longest a change waits while changes keep coming (delay tracking).
pub(crate) const MAX_WAIT: Duration = Duration::from_secs(10);
/// Wait before writing again after a failed write.
pub(crate) const RETRY: Duration = Duration::from_secs(10);

/// What a write would put in the manifest: trace data never changes without its metadata
/// being committed again, and an SPL log's file only grows until another log (another id)
/// replaces it, so equal fingerprints mean equal autosaves.
pub(crate) type Fingerprint = (Vec<SavedMeasurement>, Vec<TraceMeta>, Vec<(MeasId, u64)>);

/// `<dir><suffix>` beside `dir`.
fn sibling(dir: &Path, prefix: &str, suffix: &str) -> PathBuf {
    let name = dir
        .file_name()
        .map_or_else(|| "autosave".into(), |n| n.to_string_lossy().into_owned());
    dir.with_file_name(format!("{prefix}{name}{suffix}"))
}

fn remove_dir(p: &Path) -> Result<(), SessionError> {
    match fs::remove_dir_all(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(SessionError::Io {
            path: p.to_owned(),
            msg: e.to_string(),
        }),
    }
}

fn rename(from: &Path, to: &Path) -> Result<(), SessionError> {
    fs::rename(from, to).map_err(|e| SessionError::Io {
        path: to.to_owned(),
        msg: e.to_string(),
    })
}

/// Writes `data` as the current autosave in `dir`, naming the `linked` SPL log files; the
/// current manifest becomes the previous one.
pub(crate) fn write(
    dir: &Path,
    data: &SessionData,
    linked: &[SavedSplLogFile],
) -> Result<(), SessionError> {
    session::save_autosave(dir, data, linked)?;
    #[cfg(unix)]
    if let Some(parent) = dir.parent()
        && let Ok(d) = fs::File::open(parent)
    {
        // Persists a newly created directory.
        let _ = d.sync_all();
    }
    Ok(())
}

/// Renames `p` to `<p>.<tag>` (with the time appended if that exists), keeping it for the
/// operator to inspect or load by path. Returns where it went.
pub(crate) fn set_aside(p: &Path, tag: &str) -> Option<PathBuf> {
    let mut to = sibling(p, "", &format!(".{tag}"));
    if to.exists() {
        to = sibling(
            p,
            "",
            &format!(".{tag}-{}", crate::util::wall_ns() / 1_000_000_000),
        );
    }
    match fs::rename(p, &to) {
        Ok(()) => Some(to),
        Err(e) => {
            tracing::warn!("autosave: cannot set {} aside: {e}", p.display());
            None
        }
    }
}

/// Moves the autosave out of the way for a `--no-restore` start: to `<dir>.unrestored`,
/// replacing an older one, so that the work is still on disk (`session load <path>`).
pub(crate) fn skip_restore(dir: &Path) {
    if !dir.exists() {
        return;
    }
    let to = sibling(dir, "", ".unrestored");
    match remove_dir(&to).and_then(|()| rename(dir, &to)) {
        Ok(()) => tracing::info!(
            "autosave not restored (--no-restore); it is kept in {}",
            to.display()
        ),
        Err(e) => tracing::warn!("autosave not restored (--no-restore); {e}"),
    }
}

/// An autosave read back.
pub(crate) struct Restored {
    pub(crate) data: SessionData,
    /// The manifest it came from.
    pub(crate) from: PathBuf,
    /// Where its SPL logs were read from, to append to them.
    pub(crate) logs: Vec<SplLogOnDisk>,
}

fn warn_aside(e: &SessionError, p: &Path, to: Option<PathBuf>) {
    tracing::warn!(
        "autosave not restored: {e}; set aside as {}",
        to.map_or_else(|| p.display().to_string(), |t| t.display().to_string())
    );
}

/// The autosave in `dir`: its current manifest, else the previous one. An autosave of
/// another session format version is set aside, as is an unreadable one, with a warning;
/// neither is deleted.
pub(crate) fn restore(dir: &Path) -> Option<Restored> {
    let (cur, prev) = (dir.join(MANIFEST), dir.join(PREV_MANIFEST));
    if !cur.exists() && !prev.exists() {
        // Nothing, or the SPL logs of a first write that never completed.
        return None;
    }
    let e = match session::load_named(dir, MANIFEST) {
        Ok((data, logs)) => {
            return Some(Restored {
                data,
                from: cur,
                logs,
            });
        }
        Err(e @ SessionError::Version { found, .. }) => {
            warn_aside(&e, dir, set_aside(dir, &format!("v{found}")));
            return None;
        }
        Err(e) => e,
    };
    match session::load_named(dir, PREV_MANIFEST) {
        Ok((data, logs)) => {
            tracing::warn!(
                "autosave: {e}; restoring the previous one ({})",
                prev.display()
            );
            if cur.exists() {
                // Kept for inspection; a manifest of that name is not read.
                let _ = fs::rename(&cur, dir.join("session.damaged.json"));
            }
            Some(Restored {
                data,
                from: prev,
                logs,
            })
        }
        Err(_) => {
            warn_aside(&e, dir, set_aside(dir, "damaged"));
            None
        }
    }
}

/// What the write thread is asked to do.
enum Job {
    /// Keep these SPL logs on disk (a restore's, with where their files stand).
    Logs(Vec<LogSpec>),
    /// Write `data`, its SPL logs being these.
    Save {
        data: SessionData,
        logs: Vec<LogSpec>,
    },
}

/// The write thread: one write at a time, each result reported to the control thread, the
/// SPL logs appended in between.
pub(crate) struct Writer {
    tx: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

impl Writer {
    pub(crate) fn spawn(dir: PathBuf, done: Sender<ControlMsg>) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Job>();
        let thread = std::thread::Builder::new()
            .name("ac2d-autosave".into())
            .spawn(move || {
                let mut files = SplFiles::new(dir.clone(), Instant::now());
                loop {
                    let job = match files.due() {
                        Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
                        None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                    };
                    match job {
                        Ok(Job::Logs(logs)) => files.apply(logs),
                        Ok(Job::Save { data, logs }) => {
                            files.apply(logs);
                            files.append_all();
                            let at = data.saved_at;
                            let linked = files.linked(&data.measurements);
                            let result = write(&dir, &data, &linked)
                                .map(|()| at)
                                .map_err(|e| e.to_string());
                            if let Err(e) = &result {
                                tracing::warn!("autosave failed: {e}");
                            } else {
                                tracing::debug!("autosaved to {}", dir.display());
                            }
                            let _ = done.send(ControlMsg::Autosaved {
                                result: Box::new(result),
                            });
                        }
                        Err(RecvTimeoutError::Timeout) => files.tick(Instant::now()),
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                files.close_all();
            })?;
        Ok(Self {
            tx: Some(tx),
            thread: Some(thread),
        })
    }

    fn send(&self, job: Job) -> bool {
        self.tx.as_ref().is_some_and(|t| t.send(job).is_ok())
    }

    /// Finishes the queued writes, appends and syncs the SPL logs, and ends the thread.
    fn finish(&mut self) {
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.finish();
    }
}

/// The control thread's side: what is on disk, what is being written, when to write next.
pub(crate) struct Autosaver {
    pub(crate) dir: PathBuf,
    writer: Writer,
    /// What the last successful write held.
    on_disk: Option<Fingerprint>,
    /// What the write in progress holds.
    in_flight: Option<Fingerprint>,
    /// First and last change not yet written.
    dirty: Option<(Instant, Instant)>,
    /// No write before this, after a failure.
    retry_at: Option<Instant>,
}

impl Autosaver {
    pub(crate) fn new(dir: PathBuf, writer: Writer) -> Self {
        Self {
            dir,
            writer,
            on_disk: None,
            in_flight: None,
            dirty: None,
            retry_at: None,
        }
    }

    /// `fp` is now on disk (a restore), its SPL logs `logs`.
    pub(crate) fn restored(&mut self, fp: Fingerprint, logs: Vec<LogSpec>) {
        self.on_disk = Some(fp);
        self.dirty = None;
        self.writer.send(Job::Logs(logs));
    }

    /// A change to the state, whose fingerprint is now `fp`. Returns whether anything is
    /// left to write.
    pub(crate) fn changed(&mut self, fp: &Fingerprint, now: Instant) -> bool {
        if self.in_flight.is_none() && self.on_disk.as_ref() == Some(fp) {
            self.dirty = None;
            return false;
        }
        self.dirty = Some((self.dirty.map_or(now, |(first, _)| first), now));
        true
    }

    /// When the next write is due.
    pub(crate) fn due(&self) -> Option<Instant> {
        if self.in_flight.is_some() {
            return None;
        }
        let (first, last) = self.dirty?;
        let at = (last + DEBOUNCE).min(first + MAX_WAIT);
        Some(self.retry_at.map_or(at, |r| at.max(r)))
    }

    /// Starts writing `data` with its SPL logs `logs` unless its fingerprint `fp` is
    /// already on disk. Returns whether a write started.
    pub(crate) fn write(&mut self, data: SessionData, fp: Fingerprint, logs: Vec<LogSpec>) -> bool {
        self.dirty = None;
        if self.on_disk.as_ref() == Some(&fp) {
            return false;
        }
        if self.writer.send(Job::Save { data, logs }) {
            self.in_flight = Some(fp);
            true
        } else {
            false
        }
    }

    /// The write in progress finished. Returns whether changes are still waiting.
    pub(crate) fn finished(&mut self, ok: bool, now: Instant) -> bool {
        let fp = self.in_flight.take();
        if ok {
            self.on_disk = fp;
            self.retry_at = None;
        } else {
            self.retry_at = Some(now + RETRY);
            if self.dirty.is_none() {
                self.dirty = Some((now, now));
            }
        }
        self.dirty.is_some()
    }

    /// Whether anything is unwritten or being written.
    pub(crate) fn busy(&self) -> bool {
        self.dirty.is_some() || self.in_flight.is_some()
    }

    /// Shutdown: writes `data` if it is not on disk yet, and waits for the writes and the
    /// SPL logs' last rows.
    pub(crate) fn flush(&mut self, data: Option<(SessionData, Fingerprint, Vec<LogSpec>)>) {
        if let Some((data, fp, logs)) = data
            && self.on_disk.as_ref() != Some(&fp)
            && self.in_flight.as_ref() != Some(&fp)
        {
            let _ = self.writer.send(Job::Save { data, logs });
        }
        self.writer.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::leq_log::{self, LeqLog, SharedLog};
    use ac2_proto::model::{
        MeasConfig, MeasKind, PeakWeighting, SplConfig, SplLogRow, TimeWeighting, Weighting,
    };
    use ac2_proto::units::{Db, Dbfs, MeasId, Seconds, WallNs};
    use ac2_traces::spl_log::SplLogInfo;
    use std::sync::{Arc, Mutex};

    const NS: u64 = 1_000_000_000;
    const T0: u64 = 1_790_000_000 * NS;

    fn data(at: u64) -> SessionData {
        SessionData {
            saved_at: WallNs(at),
            measurements: Vec::new(),
            spl_logs: Vec::new(),
            traces: Vec::new(),
        }
    }

    fn meter() -> SavedMeasurement {
        SavedMeasurement {
            id: MeasId(1),
            config: MeasConfig {
                name: "x".into(),
                kind: MeasKind::Spl {
                    config: SplConfig {
                        input: 0,
                        weighting: Weighting::A,
                        time_weighting: TimeWeighting::Fast,
                        peak_weighting: PeakWeighting::C,
                        leq: ac2_proto::model::LeqConfig::default_windows(),
                        position: None,
                    },
                },
            },
            running: true,
            frozen: false,
            delay: None,
        }
    }

    fn with_meter(at: u64) -> SessionData {
        SessionData {
            measurements: vec![meter()],
            ..data(at)
        }
    }

    fn row(k: u64) -> SplLogRow {
        SplLogRow {
            start: WallNs(T0 + k * NS),
            measured: Seconds(1.0),
            laeq: Dbfs(-30.0 - (k % 17) as f64 * 0.37),
            lceq: Dbfs(-28.0),
            lzeq: Dbfs(-27.0),
            lcpeak: Dbfs(-12.0),
            lafmax: Dbfs(-25.0),
            sensitivity: Some(Db(120.0)),
            position: None,
        }
    }

    fn push(log: &SharedLog, from: u64, n: u64) {
        let mut l = leq_log::lock(log);
        for k in from..from + n {
            l.push(row(k));
        }
    }

    fn spec(log: &SharedLog, resume: Option<Resume>) -> LogSpec {
        LogSpec {
            meas: MeasId(1),
            log: log.clone(),
            info: SplLogInfo {
                meas: MeasId(1),
                name: "x".into(),
                input: 0,
                mic: None,
            },
            resume,
        }
    }

    /// Bytes of every file under `dir`.
    fn du(dir: &Path) -> u64 {
        let mut n = 0;
        for e in fs::read_dir(dir).expect("dir").flatten() {
            let m = e.metadata().expect("meta");
            n += if m.is_dir() { du(&e.path()) } else { m.len() };
        }
        n
    }

    #[test]
    fn write_keeps_the_previous_as_backup() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("autosave");
        write(&dir, &data(1), &[]).expect("first");
        assert!(!dir.join(PREV_MANIFEST).exists());
        write(&dir, &data(2), &[]).expect("second");
        write(&dir, &data(3), &[]).expect("third");
        assert_eq!(
            session::read_manifest(&dir).expect("cur").saved_at,
            WallNs(3)
        );
        assert_eq!(
            session::read_manifest_named(&dir, PREV_MANIFEST)
                .expect("prev")
                .saved_at,
            WallNs(2)
        );
        let r = restore(&dir).expect("restore");
        assert_eq!((r.data.saved_at, r.from), (WallNs(3), dir.join(MANIFEST)));
    }

    #[test]
    fn restore_falls_back_to_the_backup_and_sets_damage_aside() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("autosave");
        write(&dir, &data(1), &[]).expect("first");
        write(&dir, &data(2), &[]).expect("second");
        fs::write(dir.join(MANIFEST), b"{ not json").expect("damage");
        let r = restore(&dir).expect("restore");
        assert_eq!(
            (r.data.saved_at, r.from),
            (WallNs(1), dir.join(PREV_MANIFEST))
        );
        assert!(dir.join("session.damaged.json").exists());
        assert!(!dir.join(MANIFEST).exists());
        // Restored again (nothing written in between): the backup still.
        assert_eq!(restore(&dir).expect("again").data.saved_at, WallNs(1));
        // The next write goes on from there.
        write(&dir, &data(3), &[]).expect("third");
        assert_eq!(restore(&dir).expect("third").data.saved_at, WallNs(3));
        // Both unreadable: the directory is set aside.
        fs::write(dir.join(MANIFEST), b"{ not json").expect("damage");
        fs::write(dir.join(PREV_MANIFEST), b"{ not json").expect("damage");
        assert!(restore(&dir).is_none());
        assert!(!dir.exists());
        assert!(root.path().join("autosave.damaged").exists());
    }

    /// A write interrupted between the manifest's two renames leaves only the previous
    /// manifest, which a restore takes; a directory with only the SPL logs of a first write
    /// that never completed restores nothing and is left in place.
    #[test]
    fn an_interrupted_write_restores_the_previous() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("autosave");
        write(&dir, &data(1), &[]).expect("first");
        fs::rename(dir.join(MANIFEST), dir.join(PREV_MANIFEST)).expect("cut");
        assert_eq!(restore(&dir).expect("restore").data.saved_at, WallNs(1));
        let fresh = root.path().join("fresh");
        fs::create_dir_all(fresh.join("spl")).expect("dir");
        fs::write(fresh.join("spl/1-5.csv"), b"# ac2 spl log v2\n").expect("log");
        assert!(restore(&fresh).is_none());
        assert!(fresh.exists());
    }

    #[test]
    fn another_version_is_set_aside_not_deleted() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("autosave");
        write(&dir, &data(1), &[]).expect("first");
        let m = dir.join(session::MANIFEST);
        let text = fs::read_to_string(&m).expect("manifest");
        let other = text.replace(
            &format!("\"version\": {}", session::VERSION),
            "\"version\": 999",
        );
        assert_ne!(text, other);
        fs::write(&m, other).expect("version");
        assert!(restore(&dir).is_none());
        assert!(!dir.exists());
        let aside = root.path().join("autosave.v999");
        assert!(aside.join(session::MANIFEST).exists());
    }

    #[test]
    fn no_restore_moves_the_autosave_aside() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("autosave");
        write(&dir, &data(1), &[]).expect("first");
        skip_restore(&dir);
        assert!(!dir.exists());
        assert!(
            root.path()
                .join("autosave.unrestored")
                .join(session::MANIFEST)
                .exists()
        );
        assert!(restore(&dir).is_none());
    }

    /// A meter with a day of log runs for an hour of simulated minutes, each minute's rows
    /// appended and an autosave written every minute regardless: what reaches the disk is
    /// the log once, then the new rows and the manifests, never the log again.
    #[test]
    fn a_running_meter_costs_its_new_rows() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("autosave");
        let log: SharedLog = Arc::new(Mutex::new(LeqLog::default()));
        let day = 24 * 3600;
        push(&log, 0, day);
        let mut files = SplFiles::new(dir.clone(), Instant::now());
        files.apply(vec![spec(&log, None)]);
        write(&dir, &with_meter(1), &files.linked(&[meter()])).expect("first");
        let whole = files.bytes;
        let path = files.path_of(MeasId(1)).expect("file");
        assert_eq!(fs::metadata(&path).expect("log").len(), whole);
        let before = du(&dir);

        let minutes = 60;
        let mut manifests = 0;
        for m in 0..minutes {
            push(&log, day + m * 60, 60);
            files.append_all();
            write(&dir, &with_meter(2 + m), &files.linked(&[meter()])).expect("write");
            manifests += fs::metadata(dir.join(MANIFEST)).expect("manifest").len();
        }
        let appended = files.bytes - whole;
        let line = whole / day;
        let rows = minutes * 60;
        assert!(
            appended <= rows * (line + 2),
            "appended {appended} B for {rows} rows of ~{line} B"
        );
        // The directory grew by the rows alone (the manifests replace each other).
        let grown = du(&dir) - before;
        assert!(
            grown <= appended + 4096,
            "grew {grown} B, appended {appended} B"
        );
        // Everything written: the log once, the rows, the manifests. The old scheme wrote
        // the whole log (the day and then some) every minute.
        let total = whole + appended + manifests;
        let old = minutes * whole;
        assert!(total * 20 < old, "{total} B vs {old} B");
        assert_eq!(files.path_of(MeasId(1)), Some(path.clone()));

        // Read back: every row, in order.
        let (back, on_disk) = session::load_named(&dir, MANIFEST).expect("load");
        let r = &back.spl_logs[0].rows;
        assert_eq!(r.len() as u64, day + rows);
        assert!(
            r.iter()
                .enumerate()
                .all(|(k, x)| x.start == row(k as u64).start)
        );
        assert_eq!(on_disk[0].rows, day + rows);
    }

    /// Restored after a power cut that kept half of the last append: every whole row is
    /// back, the Leq over them is what was logged, and appending carries on after the last
    /// whole row without a broken line.
    #[test]
    fn restore_reads_past_a_cut_append_and_carries_on() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("autosave");
        let log: SharedLog = Arc::new(Mutex::new(LeqLog::default()));
        push(&log, 0, 100);
        let mut files = SplFiles::new(dir.clone(), Instant::now());
        files.apply(vec![spec(&log, None)]);
        write(&dir, &with_meter(1), &files.linked(&[meter()])).expect("first");
        push(&log, 100, 50);
        files.append_all();
        files.close_all();
        let path = files.path_of(MeasId(1));
        assert!(path.is_none(), "closed");
        let (_, on_disk) = session::load_named(&dir, MANIFEST).expect("load");
        let path = dir.join(&on_disk[0].file);
        let mut text = fs::read(&path).expect("log");
        // Half a line more, as a cut append leaves it.
        text.extend_from_slice(b"2026-09-21T14:20:00.000Z,17900001");
        fs::write(&path, &text).expect("cut");

        let r = restore(&dir).expect("restore");
        let rows = &r.data.spl_logs[0].rows;
        assert_eq!(rows.len(), 150);
        let restored = LeqLog::from_rows(rows.clone());
        let logged = leq_log::lock(&log).run().expect("run");
        let back = restored.run().expect("run");
        assert_eq!(back.started_at, logged.started_at);
        assert!((back.levels_dbfs[0] - logged.levels_dbfs[0]).abs() < 1e-3);

        // The daemon carries on: the restored log, the file resumed, more rows.
        let log: SharedLog = Arc::new(Mutex::new(restored));
        let disk = &r.logs[0];
        let mut files = SplFiles::new(dir.clone(), Instant::now());
        files.apply(vec![spec(
            &log,
            Some(Resume {
                file: disk.file.clone(),
                rows: disk.rows,
                complete_len: disk.complete_len,
            }),
        )]);
        assert_eq!(files.path_of(MeasId(1)), Some(path.clone()));
        let rewritten = files.bytes;
        assert_eq!(rewritten, 0, "resumed, not rewritten");
        push(&log, 150, 30);
        files.close_all();
        let (back, _) = session::load_named(&dir, MANIFEST).expect("load");
        let r = &back.spl_logs[0].rows;
        assert_eq!(r.len(), 180);
        assert!(
            r.iter()
                .enumerate()
                .all(|(k, x)| x.start == row(k as u64).start)
        );
    }

    /// `spl.log_new` swaps the log inside the shared handle: the next append closes the
    /// ended log's file and starts one for the new log; a log past its retention by
    /// [`spl_files::COMPACT_SLACK`] rows is rewritten with the retained rows only.
    #[test]
    fn a_new_log_gets_a_new_file_and_long_logs_are_compacted() {
        use ac2_proto::model::SplLogPage;
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("autosave");
        let log: SharedLog = Arc::new(Mutex::new(LeqLog::default()));
        push(&log, 0, 10);
        let mut files = SplFiles::new(dir.clone(), Instant::now());
        files.apply(vec![spec(&log, None)]);
        let first = files.path_of(MeasId(1)).expect("file");
        {
            let mut l = leq_log::lock(&log);
            let next = l.next();
            *l = next;
        }
        push(&log, 20, 5);
        files.append_all();
        let second = files.path_of(MeasId(1)).expect("file");
        assert_ne!(first, second);
        write(&dir, &with_meter(1), &files.linked(&[meter()])).expect("write");
        let (back, _) = session::load_named(&dir, MANIFEST).expect("load");
        assert_eq!(back.spl_logs[0].rows.len(), 5);

        let retained = SplLogPage::RETAINED_ROWS as u64;
        // Up to the limit (5 rows are in already): appended.
        let n = retained + spl_files::COMPACT_SLACK - 5;
        for k in (0..n).step_by(3600) {
            push(&log, 100 + k, 3600.min(n - k));
            files.append_all();
        }
        let lines = || fs::read_to_string(&second).expect("log").lines().count() as u64;
        assert_eq!(lines(), 5 + retained + spl_files::COMPACT_SLACK);
        // One more: rewritten.
        push(&log, 200 + retained + spl_files::COMPACT_SLACK, 1);
        files.append_all();
        // Header lines, then the retained rows.
        assert_eq!(lines(), retained + 5);
        assert_eq!(files.path_of(MeasId(1)), Some(second));
    }

    #[test]
    fn an_unwritable_directory_fails_and_keeps_nothing_half_written() {
        let root = tempfile::tempdir().expect("tempdir");
        // A file where the directory's parent should be: no user, root included, can
        // create the autosave under it.
        let blocker = root.path().join("blocker");
        fs::write(&blocker, b"").expect("file");
        let dir = blocker.join("autosave");
        assert!(write(&dir, &data(1), &[]).is_err());
    }

    #[test]
    fn debounce_waits_for_quiet_and_caps_the_wait() {
        let (tx, _rx) = mpsc::channel();
        let root = tempfile::tempdir().expect("tempdir");
        let w = Writer::spawn(root.path().join("a"), tx).expect("writer");
        let mut a = Autosaver::new(root.path().join("a"), w);
        let t0 = Instant::now();
        let fp: Fingerprint = (Vec::new(), Vec::new(), Vec::new());
        assert!(a.due().is_none());
        assert!(a.changed(&fp, t0));
        assert_eq!(a.due(), Some(t0 + DEBOUNCE));
        let t1 = t0 + Duration::from_secs(1);
        a.changed(&fp, t1);
        assert_eq!(a.due(), Some(t1 + DEBOUNCE));
        let t9 = t0 + Duration::from_secs(9);
        a.changed(&fp, t9);
        assert_eq!(a.due(), Some(t0 + MAX_WAIT));
        // Written: nothing due while in flight, and the same state is not written twice.
        assert!(a.write(data(1), fp.clone(), Vec::new()));
        assert!(a.due().is_none());
        assert!(!a.finished(true, t9));
        assert!(!a.changed(&fp, t9));
        assert!(!a.write(data(2), fp.clone(), Vec::new()));
        // A failure retries, not before RETRY.
        let other: Fingerprint = (
            vec![SavedMeasurement {
                id: MeasId(1),
                config: MeasConfig {
                    name: "x".into(),
                    kind: MeasKind::Spl {
                        config: SplConfig {
                            input: 0,
                            weighting: Weighting::A,
                            time_weighting: TimeWeighting::Fast,
                            peak_weighting: PeakWeighting::C,
                            leq: ac2_proto::model::LeqConfig::default_windows(),
                            position: None,
                        },
                    },
                },
                running: false,
                frozen: false,
                delay: None,
            }],
            Vec::new(),
            Vec::new(),
        );
        assert!(a.changed(&other, t9));
        assert!(a.write(data(3), other, Vec::new()));
        assert!(a.finished(false, t9));
        assert_eq!(a.due(), Some(t9 + RETRY));
    }
}
