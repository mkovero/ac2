//! The tracing subscriber every ac2 program with a long life installs: one filter, written
//! to stderr (journald and terminals read it) and to `<program>.log` in [`log_dir`], so an
//! app launched from a GUI with no terminal still leaves a log a tester can send.
//!
//! Only the daemon's control and I/O threads and the UI log; the audio callbacks never do,
//! because formatting and file writes allocate and make syscalls.

use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry, fmt};

use crate::log_dir;

/// Filter when `RUST_LOG` is unset: ac2's own crates (all named `ac2…`; directive targets
/// match by prefix) at info, dependencies only when something is wrong.
pub const DEFAULT_FILTER: &str = "warn,ac2=info";

/// `RUST_LOG` if set and valid, else [`DEFAULT_FILTER`].
pub fn filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER))
}

/// Largest a log file grows in one run before it is moved to `<program>.log.1` and a fresh
/// one started: a kiosk runs for weeks, and a warning repeated every few seconds must not
/// fill the disk the autosave and the SPL logs need. At most twice this is kept.
pub const MAX_LOG_BYTES: u64 = 16 << 20;

/// The log file, moved to `.1` (replacing the one before) and started anew whenever the
/// next line would take it past its cap.
#[derive(Debug)]
pub struct CappedFile {
    path: PathBuf,
    cap: u64,
    inner: Mutex<Capped>,
}

#[derive(Debug)]
struct Capped {
    /// `None` after a rotation failed to open the fresh file: lines go to stderr only.
    file: Option<File>,
    len: u64,
}

impl CappedFile {
    /// `file`, open for append at `path`, rotated past `cap` bytes.
    pub fn new(file: File, path: PathBuf, cap: u64) -> Self {
        let len = file.metadata().map_or(0, |m| m.len());
        Self {
            path,
            cap,
            inner: Mutex::new(Capped {
                file: Some(file),
                len,
            }),
        }
    }

    fn rotate(&self, c: &mut Capped) {
        c.file = None;
        let _ = fs::rename(&self.path, previous(&self.path));
        c.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .ok();
        c.len = 0;
    }
}

fn previous(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".1");
    PathBuf::from(p)
}

// Each event arrives as one buffer in one call (the fmt layer formats it first), so a line
// is never split across a rotation; the lock is only held for that one write.
impl io::Write for &CappedFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut c = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if c.len > 0 && c.len + buf.len() as u64 > self.cap {
            self.rotate(&mut c);
        }
        let Some(f) = c.file.as_mut() else {
            return Ok(buf.len());
        };
        let n = f.write(buf)?;
        c.len += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut c = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        c.file.as_mut().map_or(Ok(()), io::Write::flush)
    }
}

/// Opens a fresh `<dir>/<program>.log`, first moving the previous run's file to
/// `<program>.log.1` (replacing the one before), so a crash's log survives one restart
/// without the directory growing.
pub fn open_log(dir: &Path, program: &str) -> io::Result<(File, PathBuf)> {
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("{program}.log"));
    if path.exists() {
        fs::rename(&path, dir.join(format!("{program}.log.1")))?;
    }
    let f = OpenOptions::new().create(true).append(true).open(&path)?;
    Ok((f, path))
}

/// The subscriber: `filter` over stderr and, when given, `file` (without colour codes).
pub fn subscriber(
    filter: EnvFilter,
    file: Option<CappedFile>,
) -> impl tracing_core::Subscriber + Send + Sync {
    // Each event is formatted into one buffer and written with one call, so threads' lines
    // never interleave.
    let file = file.map(|f| fmt::layer().with_ansi(false).with_writer(Arc::new(f)));
    Registry::default()
        .with(filter)
        // Colour codes only for a terminal: journald and redirected files store them as
        // literal escape bytes.
        .with(
            fmt::layer()
                .with_ansi(io::stderr().is_terminal())
                .with_writer(io::stderr),
        )
        .with(file)
}

/// Installs the global subscriber with `RUST_LOG` (default [`DEFAULT_FILTER`]), logging to
/// stderr and, when `dir` is given, to `<dir>/<program>.log`. Returns the log file's path;
/// `None` when no file was asked for or it could not be opened (said on stderr: logging to a
/// terminal still works, and a read-only home must not stop the program).
pub fn init_in(dir: Option<&Path>, program: &str) -> Option<PathBuf> {
    let opened = dir.and_then(|d| match open_log(d, program) {
        Ok(o) => Some(o),
        Err(e) => {
            eprintln!("{program}: no log file in {}: {e}", d.display());
            None
        }
    });
    let (file, path) = opened.map_or((None, None), |(f, p)| {
        (Some(CappedFile::new(f, p.clone(), MAX_LOG_BYTES)), Some(p))
    });
    // Also routes the `log` crate's records (the audio backends use it) into this filter.
    if let Err(e) = subscriber(filter(), file).try_init() {
        eprintln!("{program}: logging already set up: {e}");
        return None;
    }
    path
}

/// [`init_in`] with the platform [`log_dir`].
pub fn init(program: &str) -> Option<PathBuf> {
    init_in(Some(&log_dir()), program)
}

/// Logs a panic through the subscriber, with its thread, place and a backtrace, so that the
/// log file (not only stderr) says why the process went. Release builds abort on a panic
/// (`panic = "abort"`) and systemd restarts the program: a thread that died alone would
/// leave the rest running half-dead, a stuck value on screen. Before the subscriber is
/// installed the standard hook prints instead.
pub fn install_panic_hook(program: &'static str) {
    let standard = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if !tracing::dispatcher::has_been_set() {
            standard(info);
            return;
        }
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>");
        let payload = info.payload();
        let msg = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("Box<dyn Any>");
        let place = info
            .location()
            .map_or_else(String::new, |l| format!(" at {}:{}", l.file(), l.line()));
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!("{program}: thread '{name}' panicked{place}: {msg}\n{backtrace}");
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capped(f: File, path: &Path) -> CappedFile {
        CappedFile::new(f, path.to_owned(), MAX_LOG_BYTES)
    }

    /// Past its cap the file moves to `.1` and a fresh one starts, whole lines each: at most
    /// the current file and one before it.
    #[test]
    fn a_long_run_rotates_and_keeps_two_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (f, path) = open_log(dir.path(), "prog").expect("open log");
        let cap = 4096;
        let sub = subscriber(
            EnvFilter::new("info"),
            Some(CappedFile::new(f, path.clone(), cap)),
        );
        tracing::subscriber::with_default(sub, || {
            for k in 0..500 {
                tracing::warn!("repeating warning number {k:04}");
            }
        });
        let cur = fs::read_to_string(&path).expect("log");
        let prev = fs::read_to_string(dir.path().join("prog.log.1")).expect("previous");
        assert!(cur.len() as u64 <= cap && prev.len() as u64 <= cap);
        assert!(cur.contains("number 0499"), "{cur}");
        assert!(!prev.contains("number 0000"), "the oldest lines are gone");
        for text in [&cur, &prev] {
            assert!(text.ends_with('\n'));
            assert!(
                text.lines().all(|l| l.contains("repeating warning number")),
                "whole lines only"
            );
        }
        let files = fs::read_dir(dir.path()).expect("dir").count();
        assert_eq!(files, 2);
    }

    #[test]
    fn a_line_lands_in_the_file_and_the_previous_run_is_kept() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (f, path) = open_log(dir.path(), "prog").expect("open log");
        assert_eq!(path, dir.path().join("prog.log"));
        tracing::subscriber::with_default(
            subscriber(EnvFilter::new("info"), Some(capped(f, &path))),
            || {
                tracing::info!("first run");
                tracing::debug!("filtered out");
            },
        );
        let first = fs::read_to_string(&path).expect("log");
        assert!(first.contains("first run"), "{first}");
        assert!(!first.contains("filtered out"), "{first}");
        assert!(
            !first.contains('\x1b'),
            "no colour codes in the file: {first:?}"
        );

        let (f, _) = open_log(dir.path(), "prog").expect("open log");
        tracing::subscriber::with_default(
            subscriber(EnvFilter::new("info"), Some(capped(f, &path))),
            || {
                tracing::warn!("second run");
            },
        );
        let now = fs::read_to_string(&path).expect("log");
        assert!(
            now.contains("second run") && !now.contains("first run"),
            "{now}"
        );
        let prev = fs::read_to_string(dir.path().join("prog.log.1")).expect("previous log");
        assert_eq!(prev, first);
    }

    #[test]
    fn the_default_keeps_ac2_at_info_and_dependencies_at_warn() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (f, path) = open_log(dir.path(), "prog").expect("open log");
        tracing::subscriber::with_default(
            subscriber(EnvFilter::new(DEFAULT_FILTER), Some(capped(f, &path))),
            || {
                tracing::info!(target: "ac2d::control", "ours");
                tracing::info!(target: "ac2_ui", "ui");
                tracing::info!(target: "mdns_sd", "theirs");
                tracing::warn!(target: "mdns_sd", "their warning");
            },
        );
        let s = fs::read_to_string(path).expect("log");
        for want in ["ours", "ui", "their warning"] {
            assert!(s.contains(want), "{want}: {s}");
        }
        assert!(!s.contains("theirs"), "{s}");
    }
}
