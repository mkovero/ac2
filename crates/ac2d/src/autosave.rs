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
mod tests;
