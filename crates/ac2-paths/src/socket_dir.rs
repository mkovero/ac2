//! The directory that holds the local ipc sockets. Whoever can enter it can connect to the
//! daemon, which answers every local connection as the operator, or put a socket of its own
//! in place of the daemon's: so it must be closed to every other user. The daemon does not
//! start otherwise, and a client does not connect otherwise (a socket there could be
//! anyone's, answering in the daemon's place).
//!
//! What closes it differs per OS. Unix: the directory is this user's, mode 0700 (a connect
//! needs search permission on every directory of the path). Windows: the directory lies
//! under this user's local application data (`%LOCALAPPDATA%`); the profile's ACL grants
//! only the user, SYSTEM and administrators, and what is made below it inherits that. An
//! AF_UNIX bind there needs write access to the directory and a connect write access to the
//! socket file, which inherits the same ACL. On both, the directory itself is never a link
//! (nor a junction on Windows): whoever owns the target would choose where the sockets go.

use std::fmt;
use std::path::{Path, PathBuf};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use unix::{check_as, close_as, effective_uid};
#[cfg(windows)]
pub use windows::check_in;

/// Why a socket directory or path cannot carry the local sockets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketDirError {
    /// The directory.
    pub dir: PathBuf,
    /// What is wrong with it, and what it must be.
    pub problem: String,
}

impl fmt::Display for SocketDirError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "socket directory {}: {}",
            self.dir.display(),
            self.problem
        )
    }
}

impl std::error::Error for SocketDirError {}

/// Longest path, in bytes, an `ipc://` endpoint may name. The socket address holds the path
/// in a fixed `sun_path` with its terminating NUL: 108 bytes on Linux and on Windows
/// (`afunix.h`), 104 on macOS and the BSDs. libzmq refuses a longer one only at bind or
/// connect, with nothing but `ENAMETOOLONG`.
pub const MAX_SOCKET_PATH: usize = if cfg!(any(target_os = "linux", target_os = "android", windows))
{
    107
} else {
    103
};

/// The socket path of an `ipc://` endpoint; `None` for other transports.
fn socket_path(endpoint: &str) -> Option<&str> {
    endpoint.strip_prefix("ipc://")
}

/// The socket file of an `ipc://` endpoint; `None` for other transports.
pub fn socket_file(endpoint: &str) -> Option<&Path> {
    socket_path(endpoint).map(Path::new)
}

/// The directory of an `ipc://` endpoint's socket; `None` for other transports or a socket
/// named without one.
pub fn of_endpoint(endpoint: &str) -> Option<&Path> {
    Path::new(socket_path(endpoint)?)
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
}

/// Checks that an `ipc://` endpoint's socket path fits the socket address (others pass).
pub fn check_path(endpoint: &str) -> Result<(), SocketDirError> {
    let Some(path) = socket_path(endpoint) else {
        return Ok(());
    };
    if path.len() <= MAX_SOCKET_PATH {
        return Ok(());
    }
    Err(SocketDirError {
        dir: of_endpoint(endpoint).unwrap_or(Path::new(path)).to_owned(),
        problem: format!(
            "the socket path {path} is {} bytes, and an ipc socket path holds at most \
             {MAX_SOCKET_PATH}; set AC2_RUNTIME_DIR to a shorter directory",
            path.len()
        ),
    })
}

/// Checks the socket path of `endpoint` and its directory (an `ipc://` one; others pass).
pub fn check_endpoint(endpoint: &str) -> Result<(), SocketDirError> {
    check_path(endpoint)?;
    of_endpoint(endpoint).map_or(Ok(()), check)
}

/// [`check_endpoint`], except that a directory not there yet passes as absent (`Ok(false)`):
/// nothing listens in it, so a client finds no daemon there, as with a missing socket. The
/// caller checks such a directory once something answered in it.
pub fn check_endpoint_present(endpoint: &str) -> Result<bool, SocketDirError> {
    check_path(endpoint)?;
    let Some(dir) = of_endpoint(endpoint) else {
        return Ok(true);
    };
    match std::fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        _ => check(dir).map(|()| true),
    }
}

/// Checks that `dir` is closed to other users, as this OS closes it (see the module docs).
pub fn check(dir: &Path) -> Result<(), SocketDirError> {
    #[cfg(unix)]
    {
        check_as(dir, effective_uid())
    }
    #[cfg(windows)]
    {
        check_in(dir, &windows::user_base())
    }
}

/// Makes `dir` ready for the sockets: creates it when missing, closes it to other users
/// where the OS lets this process do so (Unix: mode 0700 on a directory of its own), and
/// checks it ([`check`]).
pub fn prepare(dir: &Path) -> Result<(), SocketDirError> {
    std::fs::create_dir_all(dir).map_err(|e| SocketDirError {
        dir: dir.to_owned(),
        problem: format!("cannot make it: {e}"),
    })?;
    // Not checked here: a directory this user cannot change, or a link, is refused by the
    // check below, with what is wrong with it.
    #[cfg(unix)]
    let _ = close_as(dir, effective_uid());
    check(dir)
}

/// Whether a socket file is at `path`. Not followed: on Windows an AF_UNIX socket file is a
/// reparse point that a following look cannot open, so it would read as absent.
pub fn socket_present(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn other_transports_need_no_directory() {
        check_endpoint("tcp://127.0.0.1:1").unwrap();
        check_endpoint("inproc://x").unwrap();
        check_endpoint("ipc://ctrl.sock").unwrap();
    }

    #[test]
    fn a_socket_path_too_long_for_the_address_is_refused_by_length() {
        let t = tempfile::tempdir().unwrap();
        let fits = "d".repeat(MAX_SOCKET_PATH - "/ctrl.sock".len());
        let at = |name: &str| format!("ipc://{name}/ctrl.sock");
        check_path(&at(&fits)).unwrap();
        let long = format!("{fits}d");
        let e = check_path(&at(&long)).unwrap_err();
        let msg = e.to_string();
        assert!(
            msg.contains(&format!("is {} bytes", MAX_SOCKET_PATH + 1)),
            "{msg}"
        );
        assert!(msg.contains("AC2_RUNTIME_DIR"), "{msg}");
        // Refused before the directory is looked at, present or not.
        let deep = t.path().join("x".repeat(MAX_SOCKET_PATH));
        let ep = format!("ipc://{}", deep.join("ctrl.sock").display());
        assert!(check_endpoint(&ep).unwrap_err().problem.contains("bytes"));
        assert!(check_endpoint_present(&ep).is_err());
    }

    #[test]
    fn a_directory_made_here_is_taken() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("a").join("run");
        prepare(&dir).unwrap();
        check(&dir).unwrap();
        assert!(!socket_present(&dir.join("ctrl.sock")));
        std::fs::write(dir.join("ctrl.sock"), b"").unwrap();
        assert!(socket_present(&dir.join("ctrl.sock")));
    }
}
