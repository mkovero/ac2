//! The directory that holds the local ipc sockets. Whoever can enter it can connect to the
//! daemon, which answers every local connection as the operator, or put a socket of its own
//! in place of the daemon's before the bind: so it must be the user's own and closed to
//! everyone else, and the daemon does not start otherwise.

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use crate::StartError;

/// Creates the directory of `endpoint`'s socket (an `ipc://` one; others need none), closes
/// it to other users, and checks that it is a directory of this user's, mode 0700.
pub(crate) fn prepare(endpoint: &str) -> Result<(), StartError> {
    prepare_as(endpoint, effective_uid())
}

fn prepare_as(endpoint: &str, uid: u32) -> Result<(), StartError> {
    let Some(path) = endpoint.strip_prefix("ipc://") else {
        return Ok(());
    };
    let Some(dir) = Path::new(path)
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
    else {
        return Ok(());
    };
    std::fs::create_dir_all(dir).map_err(StartError::Io)?;
    // Not checked here: a directory this user cannot change is refused by the check below,
    // with what is wrong with it.
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    check(dir, uid)
}

fn check(dir: &Path, uid: u32) -> Result<(), StartError> {
    let refuse = |problem: String| StartError::RuntimeDir {
        dir: dir.to_owned(),
        problem,
    };
    // Not followed: a link would let whoever owns its target choose where the sockets go.
    let m = std::fs::symlink_metadata(dir).map_err(StartError::Io)?;
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
        return Err(refuse(format!(
            "its mode is {mode:o} and could not be set to 700"
        )));
    }
    Ok(())
}

#[allow(unsafe_code)]
fn effective_uid() -> u32 {
    // SAFETY: geteuid takes nothing, touches no memory and cannot fail.
    unsafe { libc::geteuid() }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

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
        let e = prepare_as(&endpoint(t.path()), effective_uid().wrapping_add(1)).unwrap_err();
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
        let e = prepare(&endpoint(&link)).unwrap_err();
        assert!(matches!(e, StartError::RuntimeDir { .. }), "{e}");
    }

    #[test]
    fn other_transports_need_no_directory() {
        prepare("tcp://127.0.0.1:1").unwrap();
        prepare("inproc://x").unwrap();
    }
}
