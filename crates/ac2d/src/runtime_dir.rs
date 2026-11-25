//! The directory that holds the local ipc sockets. Whoever can enter it can connect to the
//! daemon, which answers every local connection as the operator, or put a socket of its own
//! in place of the daemon's before the bind: so it must be the user's own and closed to
//! everyone else, and the daemon does not start otherwise. The check itself is shared with
//! the clients ([`ac2_paths::socket_dir`]), which refuse to connect through such a
//! directory.

use ac2_paths::socket_dir::{self, SocketDirError};

use crate::StartError;

fn refused(e: SocketDirError) -> StartError {
    StartError::RuntimeDir {
        dir: e.dir,
        problem: e.problem,
    }
}

/// Checks that `endpoint`'s socket path fits the socket address, then creates the directory
/// of its socket (an `ipc://` one; others need none), closes it to other users and checks
/// it ([`socket_dir::prepare`]). A socket file left by a daemon that died is no concern:
/// libzmq removes whatever is at the path before it binds, and the instance lock keeps a
/// second daemon from getting that far.
pub(crate) fn prepare(endpoint: &str) -> Result<(), StartError> {
    socket_dir::check_path(endpoint).map_err(refused)?;
    #[cfg(unix)]
    {
        prepare_as(endpoint, socket_dir::effective_uid())
    }
    #[cfg(not(unix))]
    {
        socket_dir::of_endpoint(endpoint)
            .map_or(Ok(()), |d| socket_dir::prepare(d).map_err(refused))
    }
}

#[cfg(unix)]
fn prepare_as(endpoint: &str, uid: u32) -> Result<(), StartError> {
    let Some(dir) = socket_dir::of_endpoint(endpoint) else {
        return Ok(());
    };
    std::fs::create_dir_all(dir).map_err(StartError::Io)?;
    // Not checked here: a directory this user cannot change, or a link, is refused by the
    // check below, with what is wrong with it.
    let _ = socket_dir::close_as(dir, uid);
    socket_dir::check_as(dir, uid).map_err(refused)
}

#[cfg(all(test, unix))]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::Path;

    fn endpoint(dir: &Path) -> String {
        format!("ipc://{}", dir.join("ctrl.sock").display())
    }

    #[test]
    fn an_open_directory_of_ours_is_closed_and_taken() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("run");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        prepare(&endpoint(&dir)).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().mode() & 0o7777;
        assert_eq!(mode, 0o700);
        // Created when missing, closed from the start.
        let fresh = t.path().join("a/b");
        prepare(&endpoint(&fresh)).unwrap();
        assert_eq!(std::fs::metadata(&fresh).unwrap().mode() & 0o7777, 0o700);
    }

    #[test]
    fn another_users_directory_is_refused_by_name() {
        let t = tempfile::tempdir().unwrap();
        let e = prepare_as(
            &endpoint(t.path()),
            socket_dir::effective_uid().wrapping_add(1),
        )
        .unwrap_err();
        let msg = e.to_string();
        assert!(matches!(e, StartError::RuntimeDir { .. }), "{msg}");
        assert!(msg.contains(&t.path().display().to_string()), "{msg}");
        assert!(msg.contains("owned by user"), "{msg}");
    }

    #[test]
    fn a_link_to_a_directory_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let target = t.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let link = t.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        let e = prepare(&endpoint(&link)).unwrap_err();
        assert!(matches!(e, StartError::RuntimeDir { .. }), "{e}");
        let mode = std::fs::metadata(&target).unwrap().mode() & 0o7777;
        assert_eq!(mode, 0o755, "nothing is changed through the link");
    }

    #[test]
    fn other_transports_need_no_directory() {
        prepare("tcp://127.0.0.1:1").unwrap();
        prepare("inproc://x").unwrap();
    }
}
