//! One `ac2d` per user on the default local endpoints, and the process's exit codes.
//!
//! On Unix libzmq's ipc listener unlinks an existing socket path before it binds, so a
//! second daemon on the same `ctrl.sock` would silently take the clients over while the
//! first still holds the audio interface; on Windows the second fails to bind, but only
//! after it has rotated the log and opened the backend. Neither can be told apart by the
//! socket alone (the file survives a crash), so the daemon on the default endpoints holds an
//! exclusive lock on [`lock_file`] for its whole life, taken before anything else: the OS
//! drops it with the process however that ends, so a crashed daemon never blocks the next.
//! What the lock guards is the user's audio interface and stores (`rig.json`, the
//! calibrations, the autosave), which a daemon uses whatever endpoints it listens on, so
//! every daemon takes it but an `--ephemeral` one, which keeps none of them.
//!
//! The holder writes its pid ([`crate::pid_file`]) and where it listens
//! ([`endpoints_file`]) beside the lock: a starter whose child exited
//! [`exit::ANOTHER_RUNNING`] learns there where the winner is, which need not be the
//! default local endpoints (a network-mode service).

use std::fmt;
use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

/// Process exit codes of `ac2d`, for whoever starts it (`ac2 daemon start`, the app, a
/// service manager).
pub mod exit {
    /// The daemon could not start or ended on a fatal error.
    pub const FATAL: u8 = 1;
    /// The arguments, a store path or the instance lock could not be used.
    pub const SETUP: u8 = 2;
    /// Another `ac2d` already runs for this user (it holds the instance lock); nothing
    /// was touched. `packaging/linux/ac2d.service` counts it as success, so a unit that loses
    /// the lock ends inactive instead of restarting into systemd's start limit.
    pub const ANOTHER_RUNNING: u8 = 3;
}

/// The lock file of the daemon on the default local endpoints, in [`crate::runtime_dir`].
pub fn lock_file() -> PathBuf {
    crate::runtime_dir().join(LOCK_FILE)
}

const LOCK_FILE: &str = "ac2d.lock";
const ENDPOINTS_FILE: &str = "ac2d.endpoints";

/// The endpoints file of the daemon holding the lock, in [`crate::runtime_dir`]:
/// [`listening_line`], then `mode=local` or `mode=network` (CURVE: a client needs the
/// server key and to be paired). A wildcard interface (`tcp://0.0.0.0:…`) is reached on a
/// loopback address.
pub fn endpoints_file() -> PathBuf {
    crate::runtime_dir().join(ENDPOINTS_FILE)
}

/// The line `--print-endpoints` prints and [`endpoints_file`] starts with.
pub fn listening_line(ctrl: &str, data: &str) -> String {
    format!("ac2d: listening ctrl={ctrl} data={data}")
}

/// How the lock holder's endpoints are reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// No authentication: ipc or loopback TCP.
    Local,
    /// CURVE on TCP.
    Network,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Network => "network",
        }
    }
}

/// Why the instance lock was not taken.
#[derive(Debug)]
pub enum LockError {
    /// Another process holds it.
    Held(PathBuf),
    /// The directory or the file could not be made, opened or locked.
    Io(PathBuf, io::Error),
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Held(p) => write!(
                f,
                "another ac2d is running for this user (it holds {})",
                p.display()
            ),
            Self::Io(p, e) => write!(f, "cannot take the instance lock {}: {e}", p.display()),
        }
    }
}

impl std::error::Error for LockError {}

/// The held instance lock: the daemon on the default local endpoints keeps it until it
/// exits, and with it the pid file `ac2 daemon stop` reads.
#[derive(Debug)]
pub struct Instance {
    _lock: File,
    pid_file: PathBuf,
    pid_written: bool,
    endpoints_file: PathBuf,
    endpoints_written: bool,
}

impl Instance {
    /// Takes the lock in `dir` (made if missing, owner-only on Unix, since the sockets
    /// beside it carry no authentication but the directory's permissions).
    pub fn lock(dir: &Path) -> Result<Self, LockError> {
        let path = dir.join(LOCK_FILE);
        let io_err = |e| LockError::Io(path.clone(), e);
        std::fs::create_dir_all(dir).map_err(io_err)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
                .map_err(io_err)?;
        }
        // Never truncated: the file's content is nothing, only its lock counts.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(io_err)?;
        match file.try_lock() {
            Ok(()) => Ok(Self {
                _lock: file,
                pid_file: dir.join("ac2d.pid"),
                pid_written: false,
                endpoints_file: dir.join(ENDPOINTS_FILE),
                endpoints_written: false,
            }),
            Err(TryLockError::WouldBlock) => Err(LockError::Held(path)),
            Err(TryLockError::Error(e)) => Err(LockError::Io(path, e)),
        }
    }

    /// Writes this process's pid to the pid file beside the lock.
    pub fn write_pid(&mut self) -> io::Result<()> {
        ac2_paths::write_private_atomic(
            &self.pid_file,
            format!("{}\n", std::process::id()).as_bytes(),
        )?;
        self.pid_written = true;
        Ok(())
    }

    /// The pid file beside the lock.
    pub fn pid_file(&self) -> &Path {
        &self.pid_file
    }

    /// Writes the endpoints file beside the lock (see [`endpoints_file`]).
    pub fn write_endpoints(&mut self, ctrl: &str, data: &str, mode: Mode) -> io::Result<()> {
        let text = format!("{}\nmode={}\n", listening_line(ctrl, data), mode.name());
        ac2_paths::write_private_atomic(&self.endpoints_file, text.as_bytes())?;
        self.endpoints_written = true;
        Ok(())
    }

    /// The endpoints file beside the lock.
    pub fn endpoints_file(&self) -> &Path {
        &self.endpoints_file
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // Removed while the lock is still held, so it never names a successor's pid.
        if self.pid_written {
            let _ = std::fs::remove_file(&self.pid_file);
        }
        if self.endpoints_written {
            let _ = std::fs::remove_file(&self.endpoints_file);
        }
    }
}
