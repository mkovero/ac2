//! Unix: the socket directory is this user's own, mode 0700.

use std::os::unix::fs::MetadataExt;
use std::path::Path;

use super::SocketDirError;

/// What every refusal says the directory must be.
const RULE: &str = "it must be a directory of this user's, mode 700";

/// [`super::check`] for the user `uid`.
pub fn check_as(dir: &Path, uid: u32) -> Result<(), SocketDirError> {
    let refuse = |problem: String| SocketDirError {
        dir: dir.to_owned(),
        problem: format!("{problem}; {RULE}"),
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

/// Sets `dir` to mode 0700 when it is a directory of `uid`'s; returns whether it was one.
/// Nothing is changed through a link: a chmod that followed one would close, or change,
/// whatever directory its maker pointed it at. The mode is set on the directory opened
/// without following a link, so one swapped in after the look is refused, not followed.
pub fn close_as(dir: &Path, uid: u32) -> std::io::Result<bool> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let m = std::fs::symlink_metadata(dir)?;
    if !m.file_type().is_dir() || m.uid() != uid {
        return Ok(false);
    }
    let d = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(dir)?;
    d.set_permissions(std::fs::Permissions::from_mode(0o700))?;
    Ok(true)
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
    use crate::socket_dir::{check, check_endpoint, check_endpoint_present};
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

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

        // Not there yet: absent, not refused; a link or an open one still is.
        let missing = t.path().join("missing");
        assert_eq!(check_endpoint_present(&endpoint(&missing)), Ok(false));
        assert_eq!(check_endpoint_present(&endpoint(&dir)), Ok(true));
        assert!(check_endpoint_present(&endpoint(&link)).is_err());
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(check_endpoint_present(&endpoint(&dir)).is_err());
    }

    #[test]
    fn closing_never_goes_through_a_link() {
        let t = tempfile::tempdir().unwrap();
        let target = t.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = t.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().mode() & 0o7777;
        assert!(!close_as(&link, effective_uid()).unwrap());
        assert_eq!(mode(&target), 0o755, "the link's target is left as it was");
        assert!(!close_as(&target, effective_uid().wrapping_add(1)).unwrap());
        assert_eq!(
            mode(&target),
            0o755,
            "another user's directory is left as it was"
        );
        assert!(close_as(&target, effective_uid()).unwrap());
        assert_eq!(mode(&target), 0o700);
    }
}
