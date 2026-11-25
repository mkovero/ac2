//! Windows: the socket directory lies under this user's local application data, whose ACL
//! the profile gives only the user, SYSTEM and administrators; directories made below it
//! inherit that ACL, and so does the socket file an AF_UNIX bind makes there.

use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::SocketDirError;

/// `FILE_ATTRIBUTE_REPARSE_POINT`: a symbolic link, a junction, or another redirection.
const REPARSE_POINT: u32 = 0x400;

/// This user's local application data (`%LOCALAPPDATA%`, `FOLDERID_LocalAppData`); with no
/// profile, a name nothing is under, so every directory is refused.
pub(super) fn user_base() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| b.data_local_dir().to_path_buf())
        .unwrap_or_default()
}

/// Checks that `dir` is a directory, itself no reparse point, whose real path lies under
/// `base`'s.
pub fn check_in(dir: &Path, base: &Path) -> Result<(), SocketDirError> {
    let refuse = |problem: String| SocketDirError {
        dir: dir.to_owned(),
        problem: format!(
            "{problem}; it must be a directory under {} (this user's local application \
             data), not a link or junction",
            base.display()
        ),
    };
    // Not followed: a link or junction would let whoever made it choose where the sockets go.
    let m = std::fs::symlink_metadata(dir).map_err(|e| refuse(format!("cannot read it: {e}")))?;
    if !m.file_type().is_dir() || m.file_attributes() & REPARSE_POINT != 0 {
        return Err(refuse(
            "it is not a directory (a link or junction is not followed)".into(),
        ));
    }
    // Real paths: a link or junction further up leads elsewhere, and so is refused here.
    let real = std::fs::canonicalize(dir).map_err(|e| refuse(format!("cannot resolve it: {e}")))?;
    let under = std::fs::canonicalize(base).is_ok_and(|b| real.starts_with(b));
    if !under {
        return Err(refuse(format!("it is at {}", real.display())));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn only_a_directory_under_the_base_passes() {
        let t = tempfile::tempdir().unwrap();
        let base = t.path().join("local");
        let run = base.join("ac2").join("run");
        std::fs::create_dir_all(&run).unwrap();
        check_in(&run, &base).unwrap();

        let outside = t.path().join("shared");
        std::fs::create_dir(&outside).unwrap();
        let e = check_in(&outside, &base).unwrap_err();
        assert!(e.problem.contains("must be a directory under"), "{e}");

        std::fs::write(base.join("file"), b"").unwrap();
        assert!(check_in(&base.join("file"), &base).is_err());
        assert!(check_in(&base.join("missing"), &base).is_err());
        assert!(check_in(&run, &t.path().join("no-profile")).is_err());
    }

    #[test]
    fn a_junction_under_the_base_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let base = t.path().join("local");
        let outside = t.path().join("shared");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let junction = base.join("run");
        // A junction needs no privilege, unlike a directory symbolic link.
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .status()
            .unwrap();
        assert!(made.success());
        let e = check_in(&junction, &base).unwrap_err();
        assert!(e.problem.contains("not a directory"), "{e}");
        // Below it: the real path is outside the base.
        std::fs::create_dir(outside.join("inner")).unwrap();
        assert!(check_in(&junction.join("inner"), &base).is_err());
    }

    #[test]
    fn the_users_runtime_directory_passes() {
        let dir = crate::runtime_dir();
        crate::socket_dir::prepare(&dir).unwrap();
        crate::socket_dir::check(&dir).unwrap();
    }
}
