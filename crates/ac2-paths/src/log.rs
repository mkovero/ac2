//! The tracing subscriber every ac2 program with a long life installs: one filter, written
//! to stderr (journald and terminals read it) and to `<program>.log` in [`log_dir`], so an
//! app launched from a GUI with no terminal still leaves a log a tester can send.
//!
//! Only the daemon's control and I/O threads and the UI log; the audio callbacks never do,
//! because formatting and file writes allocate and make syscalls.

use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    file: Option<File>,
) -> impl tracing_core::Subscriber + Send + Sync {
    // Each event is formatted into one buffer and written with one call, and the file is
    // opened for append, so threads' lines never interleave without a lock.
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
    let (file, path) = opened.map_or((None, None), |(f, p)| (Some(f), Some(p)));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_lands_in_the_file_and_the_previous_run_is_kept() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (f, path) = open_log(dir.path(), "prog").expect("open log");
        assert_eq!(path, dir.path().join("prog.log"));
        tracing::subscriber::with_default(subscriber(EnvFilter::new("info"), Some(f)), || {
            tracing::info!("first run");
            tracing::debug!("filtered out");
        });
        let first = fs::read_to_string(&path).expect("log");
        assert!(first.contains("first run"), "{first}");
        assert!(!first.contains("filtered out"), "{first}");
        assert!(
            !first.contains('\x1b'),
            "no colour codes in the file: {first:?}"
        );

        let (f, _) = open_log(dir.path(), "prog").expect("open log");
        tracing::subscriber::with_default(subscriber(EnvFilter::new("info"), Some(f)), || {
            tracing::warn!("second run");
        });
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
            subscriber(EnvFilter::new(DEFAULT_FILTER), Some(f)),
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
