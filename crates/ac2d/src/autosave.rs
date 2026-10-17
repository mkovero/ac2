//! Autosave of the measurements and traces, in the session file format
//! (`ac2_traces::session`), so that a daemon restart (a new `--max-level`, a crash, a reboot)
//! does not lose them.
//!
//! ```text
//! <dir>/session.json        the current autosave's manifest
//! <dir>/session.prev.json   the one before it, kept as a backup
//! <dir>/traces/, <dir>/spl/ the files either names
//! <dir>.v<N>                an autosave of session format N that this build cannot read, set aside
//! <dir>.damaged-<time>      an unreadable autosave, set aside
//! <dir>.damaged-files/      trace and log files an autosave named but could not read
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
//! The working state's small files (`working.json`, the recovery copy put in place) are
//! written on the same thread ([`Autosaver::run`]), in the order the control thread asks.

mod rig_logs;
mod spl_files;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ac2_proto::model::{StateFile, TraceMeta};
use ac2_proto::units::MeasId;
use ac2_traces::session::{
    self, MANIFEST, PREV_MANIFEST, SavedMeasurement, SavedSplLogFile, Session as SessionData,
    SessionError, SplLogOnDisk,
};
use ac2_traces::session::{Dropped, DroppedKind};

pub(crate) use self::rig_logs::RigLogs;
use self::spl_files::SplFiles;
pub(crate) use self::spl_files::{LogSpec, Resume};
use crate::control::ControlMsg;
use crate::damage::Damage;

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

/// Damage found by [`restore`] for an unreadable whole (`path`, `to`).
fn whole(e: &SessionError, path: &Path, to: Option<PathBuf>, what: &str) -> Damage {
    let d = Damage {
        kind: StateFile::Autosave,
        path: path.to_owned(),
        set_aside_as: to,
        reason: format!("{e}; {what}"),
    };
    d.log();
    d
}

/// Moves the files a lenient read left out (`dropped`, relative to `dir`) out of the
/// autosave, where the next write would delete them as unnamed: into `<dir>.damaged-files/`,
/// each named `<file>.damaged-<time>`.
pub(crate) fn set_dropped_aside(dir: &Path, dropped: Vec<Dropped>) -> Vec<Damage> {
    let into = sibling(dir, "", ".damaged-files");
    dropped
        .into_iter()
        .map(|d| {
            let from = dir.join(&d.file);
            let moved = fs::create_dir_all(&into)
                .and_then(|()| {
                    let flat = into.join(d.file.replace(['/', '\\'], "_"));
                    fs::rename(&from, &flat)?;
                    ac2_paths::set_aside_damaged(&flat)
                })
                .and_then(|to| ac2_paths::sync_dir(dir).map(|()| to));
            let (set_aside_as, what) = match moved {
                Ok(to) => (Some(to), "left out"),
                Err(_) if !from.exists() => (None, "left out"),
                Err(e) => {
                    tracing::warn!("autosave: cannot set {} aside: {e}", from.display());
                    (None, "left out (it could not be set aside)")
                }
            };
            let what = match d.kind {
                DroppedKind::Trace => format!("{what}; the trace is not restored"),
                DroppedKind::SplLog => format!("{what}; the meter starts a new log"),
                DroppedKind::BandLog => format!("{what}; the meter starts without band history"),
            };
            let damage = Damage {
                kind: match d.kind {
                    DroppedKind::Trace => StateFile::Trace,
                    DroppedKind::SplLog => StateFile::SplLog,
                    DroppedKind::BandLog => StateFile::BandLog,
                },
                path: from,
                set_aside_as,
                reason: format!("{}; {what}", d.reason),
            };
            damage.log();
            damage
        })
        .collect()
}

/// Logs the logs a power cut left a partial tail on (a torn row, zeros). Not damage: every
/// whole row was read, and the tail is cut off before the next append.
fn note_torn_tails(dir: &Path, logs: &[SplLogOnDisk]) {
    let files = logs.iter().flat_map(|l| {
        std::iter::once((l.file.as_str(), l.complete_len))
            .chain(l.bands.iter().map(|b| (b.file.as_str(), b.complete_len)))
    });
    for (file, complete) in files {
        let p = dir.join(file);
        if let Ok(m) = fs::metadata(&p)
            && m.len() > complete
        {
            tracing::warn!(
                "{}: a partial tail of {} bytes after the last whole row (a cut write) is left \
                 out and cut off before the next append",
                p.display(),
                m.len() - complete
            );
        }
    }
}

/// The autosave in `dir`: its current manifest, else the previous one. An autosave of
/// another session format version is set aside, as is an unreadable one; neither is
/// deleted. A trace or log file that cannot be read costs only itself (moved out by
/// [`set_dropped_aside`]). What was damaged comes back beside the autosave.
pub(crate) fn restore(dir: &Path) -> (Option<Restored>, Vec<Damage>) {
    let (cur, prev) = (dir.join(MANIFEST), dir.join(PREV_MANIFEST));
    if !cur.exists() && !prev.exists() {
        // Nothing, or the SPL logs of a first write that never completed.
        return (None, Vec::new());
    }
    let restored = |data, from, logs: Vec<SplLogOnDisk>, dropped| {
        note_torn_tails(dir, &logs);
        let damage = set_dropped_aside(dir, dropped);
        (Some(Restored { data, from, logs }), damage)
    };
    let e = match session::load_lenient(dir, MANIFEST) {
        Ok((data, logs, dropped)) => return restored(data, cur, logs, dropped),
        Err(e @ SessionError::Version { found, .. }) => {
            // Not damage: another build's autosave, kept for it.
            warn_aside(&e, dir, set_aside(dir, &format!("v{found}")));
            return (None, Vec::new());
        }
        Err(e) => e,
    };
    match session::load_lenient(dir, PREV_MANIFEST) {
        Ok((data, logs, dropped)) => {
            let mut damage = Vec::new();
            if cur.exists() {
                // Kept for inspection; a manifest of that name is not read.
                let to = ac2_paths::set_aside_damaged(&cur).ok();
                damage.push(whole(&e, &cur, to, "restored the previous manifest"));
            }
            let (r, more) = restored(data, prev, logs, dropped);
            damage.extend(more);
            (r, damage)
        }
        Err(_) => {
            let to = ac2_paths::set_aside_damaged(dir).ok();
            (
                None,
                vec![whole(&e, dir, to, "started with an empty session")],
            )
        }
    }
}

fn warn_aside(e: &SessionError, p: &Path, to: Option<PathBuf>) {
    tracing::warn!(
        "autosave not restored: {e}; set aside as {}",
        to.map_or_else(|| p.display().to_string(), |t| t.display().to_string())
    );
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
    /// Small disk work beside the autosave (the working state's `working.json`, the
    /// recovery copy put in place or removed): in the order asked, after the writes asked
    /// before it, and finished before shutdown like a write.
    Run(Box<dyn FnOnce() + Send>),
}

/// The write thread: one write at a time, each result reported to the control thread, the
/// SPL logs appended in between.
pub(crate) struct Writer {
    tx: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

impl Writer {
    /// The autosave's writer, reporting each write to `done`.
    pub(crate) fn spawn(dir: PathBuf, done: Sender<ControlMsg>) -> std::io::Result<Self> {
        Self::spawn_named(dir, Some(done), "ac2d-autosave")
    }

    /// A writer whose writes are reported to `done` when given (else only logged).
    fn spawn_named(
        dir: PathBuf,
        done: Option<Sender<ControlMsg>>,
        name: &str,
    ) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Job>();
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let mut files = SplFiles::new(dir.clone(), Instant::now()).reporting(done.clone());
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
                                tracing::warn!("{} failed: {e}", dir.display());
                            } else {
                                tracing::debug!("saved to {}", dir.display());
                            }
                            if let Some(done) = &done {
                                let _ = done.send(ControlMsg::Autosaved {
                                    result: Box::new(result),
                                });
                            }
                        }
                        Ok(Job::Run(work)) => work(),
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

    /// Runs `work` on the write thread, after everything asked of it before. Returns
    /// whether it was queued (`false`: the thread is gone).
    pub(crate) fn run(&self, work: Box<dyn FnOnce() + Send>) -> bool {
        self.writer.send(Job::Run(work))
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
    use ac2_proto::spl_log::SplLogInfo;
    use ac2_proto::units::{Db, Dbfs, MeasId, Seconds, WallNs};
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
                scope: ac2_proto::model::MeasScope::Session,
                name: "x".into(),
                kind: MeasKind::Spl {
                    config: SplConfig {
                        cue: None,
                        bands: None,
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
            delay: None,
            colour: None,
            layout: None,
            compared: Vec::new(),
            offset: ac2_proto::units::Db(0.0),
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
            resume_bands: None,
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
        let r = restore(&dir).0.expect("restore");
        assert_eq!((r.data.saved_at, r.from), (WallNs(3), dir.join(MANIFEST)));
    }

    #[test]
    fn restore_falls_back_to_the_backup_and_sets_damage_aside() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("autosave");
        write(&dir, &data(1), &[]).expect("first");
        write(&dir, &data(2), &[]).expect("second");
        fs::write(dir.join(MANIFEST), b"{ not json").expect("damage");
        let (r, damage) = restore(&dir);
        let r = r.expect("restore");
        assert_eq!(
            (r.data.saved_at, r.from),
            (WallNs(1), dir.join(PREV_MANIFEST))
        );
        assert_eq!(damage.len(), 1);
        assert_eq!(damage[0].kind, StateFile::Autosave);
        assert_eq!(set_aside_count(&dir, "session.json.damaged-"), 1);
        assert!(!dir.join(MANIFEST).exists());
        // Restored again (nothing written in between): the backup still.
        assert_eq!(restore(&dir).0.expect("again").data.saved_at, WallNs(1));
        // The next write goes on from there.
        write(&dir, &data(3), &[]).expect("third");
        assert_eq!(restore(&dir).0.expect("third").data.saved_at, WallNs(3));
        // Both unreadable: the directory is set aside.
        fs::write(dir.join(MANIFEST), b"{ not json").expect("damage");
        fs::write(dir.join(PREV_MANIFEST), b"{ not json").expect("damage");
        let (r, damage) = restore(&dir);
        assert!(r.is_none());
        assert_eq!(damage.len(), 1);
        assert!(!dir.exists());
        assert_eq!(set_aside_count(root.path(), "autosave.damaged-"), 1);
    }

    fn set_aside_count(dir: &Path, prefix: &str) -> usize {
        fs::read_dir(dir)
            .expect("read_dir")
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
            .count()
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
        assert_eq!(restore(&dir).0.expect("restore").data.saved_at, WallNs(1));
        let fresh = root.path().join("fresh");
        fs::create_dir_all(fresh.join("spl")).expect("dir");
        fs::write(fresh.join("spl/1-5.csv"), b"# ac2 spl log v2\n").expect("log");
        assert!(restore(&fresh).0.is_none());
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
        assert!(restore(&dir).0.is_none());
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
        assert!(restore(&dir).0.is_none());
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

        let r = restore(&dir).0.expect("restore");
        let rows = &r.data.spl_logs[0].rows;
        assert_eq!(rows.len(), 150);
        let restored = LeqLog::from_rows(rows.clone(), Vec::new());
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

    fn band_row(k: u64) -> ac2_traces::band_log::BandLogRow {
        let mut levels = [-70.0f32; ac2_proto::model::BAND_COUNT];
        levels[5] = -60.0 - (k % 7) as f32 * 0.25;
        ac2_traces::band_log::BandLogRow {
            start: WallNs(T0 + k * NS),
            measured: Seconds(1.0),
            levels,
            correction: Db(if k.is_multiple_of(2) { 0.0 } else { 5.0 }),
            period: ac2_proto::model::BandPeriod::Night,
            sensitivity: Some(Db(120.0)),
        }
    }

    fn push_bands(log: &SharedLog, from: u64, n: u64) {
        let mut l = leq_log::lock(log);
        for k in from..from + n {
            l.push(row(k));
            l.push_band(band_row(k));
        }
    }

    /// A band meter's rows go to their own file beside the SPL log: written whole, then
    /// appended, read back row for row, and resumed after a restore.
    #[test]
    fn band_rows_are_appended_beside_and_resumed() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("autosave");
        let log: SharedLog = Arc::new(Mutex::new(LeqLog::default()));
        push_bands(&log, 0, 40);
        let mut files = SplFiles::new(dir.clone(), Instant::now());
        files.apply(vec![spec(&log, None)]);
        write(&dir, &with_meter(1), &files.linked(&[meter()])).expect("first");
        push_bands(&log, 40, 20);
        files.append_all();
        write(&dir, &with_meter(2), &files.linked(&[meter()])).expect("second");
        files.close_all();

        let r = restore(&dir).0.expect("restore");
        let bands = &r.data.spl_logs[0].bands;
        assert_eq!(bands.len(), 60);
        for (k, b) in bands.iter().enumerate() {
            let want = band_row(k as u64);
            assert_eq!(b.start, want.start);
            assert_eq!(b.correction, want.correction);
            assert_eq!(b.period, want.period);
            assert!((b.levels[5] - want.levels[5]).abs() < 0.006, "{b:?}");
        }
        let disk = r.logs[0].bands.as_ref().expect("band file");
        assert!(disk.file.ends_with(".bands.csv"), "{}", disk.file);
        assert_eq!(disk.rows, 60);

        // Resumed: appended after the last whole line, not rewritten.
        let restored = LeqLog::from_rows(r.data.spl_logs[0].rows.clone(), bands.clone());
        let log: SharedLog = Arc::new(Mutex::new(restored));
        let main = &r.logs[0];
        let mut files = SplFiles::new(dir.clone(), Instant::now());
        files.apply(vec![LogSpec {
            resume_bands: Some(Resume {
                file: disk.file.clone(),
                rows: disk.rows,
                complete_len: disk.complete_len,
            }),
            ..spec(
                &log,
                Some(Resume {
                    file: main.file.clone(),
                    rows: main.rows,
                    complete_len: main.complete_len,
                }),
            )
        }]);
        assert_eq!(files.bytes, 0, "resumed, not rewritten");
        push_bands(&log, 60, 10);
        files.close_all();
        let (back, _) = session::load_named(&dir, MANIFEST).expect("load");
        let b = &back.spl_logs[0].bands;
        assert_eq!(b.len(), 70);
        assert!(
            b.iter()
                .enumerate()
                .all(|(k, x)| x.start == band_row(k as u64).start)
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

    /// A log that cannot be written is reported once, with a short reason and since when;
    /// written again, it is reported cleared; a meter that goes clears its failure too.
    #[test]
    fn a_failing_log_is_reported_and_cleared() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("autosave");
        fs::create_dir_all(&dir).expect("dir");
        // A file where the log directory should be.
        let blocker = dir.join(ac2_traces::session::SPL_DIR);
        fs::write(&blocker, b"").expect("file");
        let log: SharedLog = Arc::new(Mutex::new(LeqLog::default()));
        push(&log, 0, 10);
        let (tx, rx) = mpsc::channel();
        let mut files = SplFiles::new(dir.clone(), Instant::now()).reporting(Some(tx));
        files.apply(vec![spec(&log, None)]);
        let failure = match rx.try_recv() {
            Ok(ControlMsg::LogWrite {
                meas: MeasId(1),
                log: ac2_proto::model::MeterLog::Spl,
                failure: Some(f),
            }) => f,
            other => panic!("{other:?}"),
        };
        assert!(failure.since.0 > T0, "{failure:?}");
        assert!(!failure.reason.is_empty());
        // Retries that fail again say nothing new.
        files.append_all();
        assert!(rx.try_recv().is_err());

        fs::remove_file(&blocker).expect("unblock");
        push(&log, 10, 5);
        files.append_all();
        assert!(matches!(
            rx.try_recv(),
            Ok(ControlMsg::LogWrite {
                meas: MeasId(1),
                failure: None,
                ..
            })
        ));
        assert!(files.path_of(MeasId(1)).is_some_and(|p| p.exists()));

        // Failing again, then the meter goes: cleared.
        fs::remove_dir_all(dir.join(ac2_traces::session::SPL_DIR)).expect("rm");
        fs::write(&blocker, b"").expect("file");
        files.apply(vec![]);
        files.apply(vec![spec(&log, None)]);
        assert!(matches!(
            rx.try_recv(),
            Ok(ControlMsg::LogWrite {
                failure: Some(_),
                ..
            })
        ));
        files.apply(vec![]);
        assert!(matches!(
            rx.try_recv(),
            Ok(ControlMsg::LogWrite { failure: None, .. })
        ));
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
                    scope: ac2_proto::model::MeasScope::Session,
                    name: "x".into(),
                    kind: MeasKind::Spl {
                        config: SplConfig {
                            cue: None,
                            bands: None,
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
                delay: None,
                colour: None,
                layout: None,
                compared: Vec::new(),
                offset: ac2_proto::units::Db(0.0),
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
