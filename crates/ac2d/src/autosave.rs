//! Autosave of the measurements and traces, in the session file format
//! (`ac2_traces::session`), so that a daemon restart (a new `--max-level`, a crash, a reboot)
//! does not lose them.
//!
//! ```text
//! <dir>              the current autosave (a session directory)
//! <dir>.prev         the one before it, kept as a backup
//! .<dir>.new         a write in progress
//! <dir>.v<N>         an autosave of session format N that this build cannot read, set aside
//! <dir>.damaged      an unreadable autosave, set aside
//! <dir>.unrestored   the autosave a `--no-restore` start did not load
//! ```
//!
//! A write goes to a fresh `.new` directory first; only when it is complete does the
//! current autosave become `.prev` and the new one the current. A failed or interrupted
//! write therefore leaves the last good autosave in place, and a crash between the two
//! renames leaves `.prev`, which a start falls back to.
//!
//! Writes run on their own thread: a session with long sweeps holds megabytes of impulse
//! response, and the control thread must keep answering while that reaches the disk.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ac2_proto::model::TraceMeta;
use ac2_traces::session::{self, SavedMeasurement, Session as SessionData, SessionError};

use crate::control::ControlMsg;

/// Quiet time after the last change before a write: a burst of edits (a slot moved, a
/// rename, a delay typed) becomes one write.
pub(crate) const DEBOUNCE: Duration = Duration::from_millis(1500);
/// Longest a change waits while changes keep coming (delay tracking).
pub(crate) const MAX_WAIT: Duration = Duration::from_secs(10);
/// Wait before writing again after a failed write.
pub(crate) const RETRY: Duration = Duration::from_secs(10);

/// What a write would put on disk, minus the trace data: trace data never changes without
/// its metadata being committed again, so equal fingerprints mean equal autosaves.
pub(crate) type Fingerprint = (Vec<SavedMeasurement>, Vec<TraceMeta>);

/// `<dir><suffix>` beside `dir`.
fn sibling(dir: &Path, prefix: &str, suffix: &str) -> PathBuf {
    let name = dir
        .file_name()
        .map_or_else(|| "autosave".into(), |n| n.to_string_lossy().into_owned());
    dir.with_file_name(format!("{prefix}{name}{suffix}"))
}

/// The backup of `dir`.
pub(crate) fn prev(dir: &Path) -> PathBuf {
    sibling(dir, "", ".prev")
}

fn next(dir: &Path) -> PathBuf {
    sibling(dir, ".", ".new")
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

/// Writes `data` as the current autosave in `dir`; the current one becomes `.prev`.
pub(crate) fn write(dir: &Path, data: &SessionData) -> Result<(), SessionError> {
    let new = next(dir);
    remove_dir(&new)?;
    session::save(&new, data)?;
    if dir.exists() {
        let p = prev(dir);
        remove_dir(&p)?;
        rename(dir, &p)?;
    }
    rename(&new, dir)?;
    #[cfg(unix)]
    if let Some(parent) = dir.parent()
        && let Ok(d) = fs::File::open(parent)
    {
        // Persists the renames.
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

/// The newest readable autosave in `dir` (the current one, else `.prev`) and where it was.
/// An autosave of another session format version is set aside, as is an unreadable one,
/// with a warning; neither is deleted.
pub(crate) fn restore(dir: &Path) -> Option<(SessionData, PathBuf)> {
    for p in [dir.to_owned(), prev(dir)] {
        match session::load(&p) {
            Ok(data) => return Some((data, p)),
            Err(SessionError::NotFound(_)) => {}
            Err(e @ SessionError::Version { found, .. }) => {
                let to = set_aside(&p, &format!("v{found}"));
                tracing::warn!(
                    "autosave not restored: {e}; set aside as {}",
                    to.map_or_else(|| p.display().to_string(), |t| t.display().to_string())
                );
            }
            Err(e) => {
                let to = set_aside(&p, "damaged");
                tracing::warn!(
                    "autosave not restored: {e}; set aside as {}",
                    to.map_or_else(|| p.display().to_string(), |t| t.display().to_string())
                );
            }
        }
    }
    None
}

/// The write thread: one write at a time, each result reported to the control thread.
pub(crate) struct Writer {
    tx: Option<Sender<SessionData>>,
    thread: Option<JoinHandle<()>>,
}

impl Writer {
    pub(crate) fn spawn(dir: PathBuf, done: Sender<ControlMsg>) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<SessionData>();
        let thread = std::thread::Builder::new()
            .name("ac2d-autosave".into())
            .spawn(move || {
                while let Ok(data) = rx.recv() {
                    let at = data.saved_at;
                    let result = write(&dir, &data).map(|()| at).map_err(|e| e.to_string());
                    if let Err(e) = &result {
                        tracing::warn!("autosave failed: {e}");
                    } else {
                        tracing::debug!("autosaved to {}", dir.display());
                    }
                    let _ = done.send(ControlMsg::Autosaved {
                        result: Box::new(result),
                    });
                }
            })?;
        Ok(Self {
            tx: Some(tx),
            thread: Some(thread),
        })
    }

    fn send(&self, data: SessionData) -> bool {
        self.tx.as_ref().is_some_and(|t| t.send(data).is_ok())
    }

    /// Finishes the queued writes and ends the thread.
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

    /// `fp` is now on disk (a restore).
    pub(crate) fn restored(&mut self, fp: Fingerprint) {
        self.on_disk = Some(fp);
        self.dirty = None;
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

    /// Starts writing `data` unless its fingerprint `fp` is already on disk. Returns whether
    /// a write started.
    pub(crate) fn write(&mut self, data: SessionData, fp: Fingerprint) -> bool {
        self.dirty = None;
        if self.on_disk.as_ref() == Some(&fp) {
            return false;
        }
        if self.writer.send(data) {
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

    /// Shutdown: writes `data` if it is not on disk yet, and waits for the writes.
    pub(crate) fn flush(&mut self, data: Option<(SessionData, Fingerprint)>) {
        if let Some((data, fp)) = data
            && self.on_disk.as_ref() != Some(&fp)
            && self.in_flight.as_ref() != Some(&fp)
        {
            let _ = self.writer.send(data);
        }
        self.writer.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::{
        MeasConfig, MeasKind, PeakWeighting, SplConfig, TimeWeighting, Weighting,
    };
    use ac2_proto::units::MeasId;
    use ac2_proto::units::WallNs;

    fn data(at: u64) -> SessionData {
        SessionData {
            saved_at: WallNs(at),
            measurements: Vec::new(),
            traces: Vec::new(),
        }
    }

    #[test]
    fn write_keeps_the_previous_as_backup() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("autosave");
        write(&dir, &data(1)).expect("first");
        assert!(!prev(&dir).exists());
        write(&dir, &data(2)).expect("second");
        write(&dir, &data(3)).expect("third");
        assert_eq!(
            session::read_manifest(&dir).expect("cur").saved_at,
            WallNs(3)
        );
        assert_eq!(
            session::read_manifest(&prev(&dir)).expect("prev").saved_at,
            WallNs(2)
        );
        assert!(!next(&dir).exists());
        let (d, from) = restore(&dir).expect("restore");
        assert_eq!((d.saved_at, from), (WallNs(3), dir.clone()));
    }

    #[test]
    fn restore_falls_back_to_the_backup_and_sets_damage_aside() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("autosave");
        write(&dir, &data(1)).expect("first");
        write(&dir, &data(2)).expect("second");
        fs::write(dir.join(session::MANIFEST), b"{ not json").expect("damage");
        let (d, from) = restore(&dir).expect("restore");
        assert_eq!((d.saved_at, from), (WallNs(1), prev(&dir)));
        assert!(!dir.exists());
        assert!(root.path().join("autosave.damaged").exists());
    }

    #[test]
    fn another_version_is_set_aside_not_deleted() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("autosave");
        write(&dir, &data(1)).expect("first");
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
        write(&dir, &data(1)).expect("first");
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

    #[test]
    fn an_unwritable_directory_fails_and_keeps_nothing_half_written() {
        let root = tempfile::tempdir().expect("tempdir");
        // A file where the directory's parent should be: no user, root included, can
        // create the autosave under it.
        let blocker = root.path().join("blocker");
        fs::write(&blocker, b"").expect("file");
        let dir = blocker.join("autosave");
        assert!(write(&dir, &data(1)).is_err());
    }

    #[test]
    fn debounce_waits_for_quiet_and_caps_the_wait() {
        let (tx, _rx) = mpsc::channel();
        let root = tempfile::tempdir().expect("tempdir");
        let w = Writer::spawn(root.path().join("a"), tx).expect("writer");
        let mut a = Autosaver::new(root.path().join("a"), w);
        let t0 = Instant::now();
        let fp: Fingerprint = (Vec::new(), Vec::new());
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
        assert!(a.write(data(1), fp.clone()));
        assert!(a.due().is_none());
        assert!(!a.finished(true, t9));
        assert!(!a.changed(&fp, t9));
        assert!(!a.write(data(2), fp.clone()));
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
                        },
                    },
                },
                running: false,
                frozen: false,
                delay: None,
            }],
            Vec::new(),
        );
        assert!(a.changed(&other, t9));
        assert!(a.write(data(3), other));
        assert!(a.finished(false, t9));
        assert_eq!(a.due(), Some(t9 + RETRY));
    }
}
