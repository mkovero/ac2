//! The session directory of a daemon given none: a directory of its own under the system
//! temp dir, made at start and removed when the daemon stops. A simulated rig or a test
//! daemon then never writes the user's sessions nor clears their staging, which a real
//! daemon may be using at the same time.

use std::io;
use std::path::{Path, PathBuf};

const PREFIX: &str = "ac2d-sessions-";

/// A session directory this daemon owns; removed on drop.
#[derive(Debug)]
pub(crate) struct OwnSessions(PathBuf);

impl OwnSessions {
    /// Makes a fresh directory, closed to other users where the platform allows, and
    /// removes those of this user's daemons that are no longer running: a daemon killed
    /// never ran its drop, and nothing else removes its directory.
    pub(crate) fn make() -> io::Result<Self> {
        let tmp = std::env::temp_dir();
        let me = Self::make_in(&tmp)?;
        sweep(&tmp, &me.0);
        Ok(me)
    }

    fn make_in(parent: &Path) -> io::Result<Self> {
        let dir = parent.join(format!(
            "{PREFIX}{}-{:016x}",
            std::process::id(),
            crate::util::random_u64()
        ));
        builder().create(&dir)?;
        let me = Self(dir);
        // Closed to other users as the local sockets' directory is, per OS: Unix by owner
        // and mode 0700, Windows by lying under the user's `%LOCALAPPDATA%` (the usual
        // `%TEMP%`), whose ACL grants only the user, SYSTEM and administrators. A temp dir
        // elsewhere is refused, saying so, rather than holding sessions others can read.
        ac2_paths::socket_dir::check(&me.0)
            .map_err(|e| io::Error::other(format!("the session directory: {e}")))?;
        Ok(me)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for OwnSessions {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The process that made a directory named `ac2d-sessions-<pid>-<16 hex digits>`.
fn maker(name: &str) -> Option<u32> {
    let (pid, tag) = name.strip_prefix(PREFIX)?.split_once('-')?;
    let hex = tag.len() == 16 && tag.bytes().all(|b| b.is_ascii_hexdigit());
    hex.then(|| pid.parse().ok()).flatten()
}

/// Removes the session directories in `parent` whose daemon no longer runs. Only plain
/// directories (no link) of the user who owns `own` (this daemon's, just made) go: the
/// system temp dir is shared, and another user's files are theirs.
fn sweep(parent: &Path, own: &Path) {
    let Ok(mine) = std::fs::symlink_metadata(own) else {
        return;
    };
    let Ok(rd) = std::fs::read_dir(parent) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(pid) = name.to_str().and_then(maker) else {
            continue;
        };
        if pid == std::process::id() || ac2_paths::process::alive(pid) {
            continue;
        }
        let Ok(m) = std::fs::symlink_metadata(e.path()) else {
            continue;
        };
        if !m.is_dir() || !same_owner(&m, &mine) {
            continue;
        }
        match std::fs::remove_dir_all(e.path()) {
            Ok(()) => tracing::info!(
                "{}: removed, its daemon (pid {pid}) no longer runs",
                e.path().display()
            ),
            Err(err) => tracing::debug!("{}: not removed: {err}", e.path().display()),
        }
    }
}

/// Made with mode 0700 on Unix: no other user enters it.
#[cfg(unix)]
fn builder() -> std::fs::DirBuilder {
    use std::os::unix::fs::DirBuilderExt;
    let mut b = std::fs::DirBuilder::new();
    b.mode(0o700);
    b
}

/// Windows has no mode: the ACL inherited from `%LOCALAPPDATA%` closes it (checked when made).
#[cfg(not(unix))]
fn builder() -> std::fs::DirBuilder {
    std::fs::DirBuilder::new()
}

#[cfg(unix)]
fn same_owner(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.uid() == b.uid()
}

/// Every directory here was made under the user's own `%LOCALAPPDATA%` (checked when made),
/// which no other user can write to.
#[cfg(not(unix))]
fn same_owner(_: &std::fs::Metadata, _: &std::fs::Metadata) -> bool {
    true
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A pid no process holds now: a child's, once it was reaped.
    fn dead_pid() -> u32 {
        let mut c = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = c.id();
        c.wait().unwrap();
        assert!(!ac2_paths::process::alive(pid));
        pid
    }

    /// A killed daemon's directory goes at the next one's start; a running daemon's, a
    /// name of another shape and a file stay.
    #[test]
    fn a_dead_daemons_session_directory_is_removed_at_start() {
        let tmp = tempfile::tempdir().unwrap();
        let dead = tmp
            .path()
            .join(format!("{PREFIX}{}-00000000deadbeef", dead_pid()));
        std::fs::create_dir_all(dead.join(".incoming")).unwrap();
        std::fs::write(dead.join(".incoming/up-1.part"), b"x").unwrap();
        let running = OwnSessions::make_in(tmp.path()).unwrap();
        let other = tmp.path().join(format!("{PREFIX}1-not-a-tag"));
        std::fs::create_dir(&other).unwrap();
        let file = tmp
            .path()
            .join(format!("{PREFIX}{}-00000000deadbee0", dead_pid()));
        std::fs::write(&file, b"not a directory").unwrap();

        let me = OwnSessions::make_in(tmp.path()).unwrap();
        sweep(tmp.path(), me.path());
        assert!(!dead.exists(), "a dead daemon's directory removed");
        assert!(running.path().is_dir());
        assert!(me.path().is_dir());
        assert!(other.is_dir());
        assert!(file.is_file());
    }

    #[test]
    fn only_a_session_directory_name_names_its_maker() {
        assert_eq!(maker("ac2d-sessions-42-0123456789abcdef"), Some(42));
        assert_eq!(maker("ac2d-sessions-42-0123"), None);
        assert_eq!(maker("ac2d-sessions-x-0123456789abcdef"), None);
        assert_eq!(maker("ac2d-sessions-42"), None);
        assert_eq!(maker("ac2-sessions-42-0123456789abcdef"), None);
    }
}
