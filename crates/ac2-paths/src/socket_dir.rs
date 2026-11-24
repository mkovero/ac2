//! The directory that holds the local ipc sockets. Whoever can enter it can connect to the
//! daemon, which answers every local connection as the operator, or put a socket of its own
//! in place of the daemon's: so it must be the user's own and closed to everyone else. The
//! daemon does not start otherwise, and a client does not connect otherwise (a socket there
//! could be anyone's, answering in the daemon's place).

use std::fmt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Why a socket directory is not this user's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketDirError {
    /// The directory.
    pub dir: PathBuf,
    /// What is wrong with it.
    pub problem: String,
}

impl fmt::Display for SocketDirError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "socket directory {}: {}; it must be a directory of this user's, mode 700",
            self.dir.display(),
            self.problem
        )
    }
}

impl std::error::Error for SocketDirError {}

/// The directory of an `ipc://` endpoint's socket; `None` for other transports or a socket
/// named without one.
pub fn of_endpoint(endpoint: &str) -> Option<&Path> {
    Path::new(endpoint.strip_prefix("ipc://")?)
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
}

/// Checks the directory of `endpoint`'s socket (an `ipc://` one; others pass).
pub fn check_endpoint(endpoint: &str) -> Result<(), SocketDirError> {
    of_endpoint(endpoint).map_or(Ok(()), check)
}

/// Checks that `dir` is a directory (not a link to one) of this process's user, mode 0700.
pub fn check(dir: &Path) -> Result<(), SocketDirError> {
    check_as(dir, effective_uid())
}

/// [`check`] for the user `uid`.
pub fn check_as(dir: &Path, uid: u32) -> Result<(), SocketDirError> {
    let refuse = |problem: String| SocketDirError {
        dir: dir.to_owned(),
        problem,
    };
    // Not followed: a link would let whoever owns its target choose where the sockets go.
    let m = std::fs::symlink_metadata(dir).map_err(|e| refuse(format!("cannot read it: {e}")))?;
    if !m.file_type().is_dir() {
        return Err(refuse(
            "it is not a directory (a symbolic link is not followed)".into(),
        ));
    }
    if m.uid() != uid {
        return Err(refuse(format!(
            "it is owned by user {}, not by this user ({uid})",
            m.uid()
        )));
    }
    let mode = m.mode() & 0o7777;
    if mode != 0o700 {
        return Err(refuse(format!("its mode is {mode:o}, not 700")));
    }
    Ok(())
}

/// This process's effective user id.
#[allow(unsafe_code)]
pub fn effective_uid() -> u32 {
    // SAFETY: geteuid takes nothing, touches no memory and cannot fail.
    unsafe { libc::geteuid() }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn endpoint(dir: &Path) -> String {
        format!("ipc://{}", dir.join("ctrl.sock").display())
    }

    #[test]
    fn only_a_closed_directory_of_ours_passes() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("run");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        check_endpoint(&endpoint(&dir)).unwrap();

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let e = check_endpoint(&endpoint(&dir)).unwrap_err();
        assert!(e.problem.contains("mode is 755"), "{e}");

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let e = check_as(&dir, effective_uid().wrapping_add(1)).unwrap_err();
        assert!(e.to_string().contains("owned by user"), "{e}");

        let link = t.path().join("link");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert!(check(&link).is_err());
        assert!(check(&t.path().join("missing")).is_err());
    }

    #[test]
    fn other_transports_need_no_directory() {
        check_endpoint("tcp://127.0.0.1:1").unwrap();
        check_endpoint("inproc://x").unwrap();
        check_endpoint("ipc://ctrl.sock").unwrap();
    }
}
