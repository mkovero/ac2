//! The session directory of a daemon given none: a directory of its own under the system
//! temp dir, made at start and removed when the daemon stops. A simulated rig or a test
//! daemon then never writes the user's sessions nor clears their staging, which a real
//! daemon may be using at the same time.

use std::io;
use std::path::{Path, PathBuf};

/// A session directory this daemon owns; removed on drop.
#[derive(Debug)]
pub(crate) struct OwnSessions(PathBuf);

impl OwnSessions {
    /// Makes a fresh directory, closed to other users where the platform allows.
    pub(crate) fn make() -> io::Result<Self> {
        let dir = std::env::temp_dir().join(format!(
            "ac2d-sessions-{}-{:016x}",
            std::process::id(),
            crate::util::random_u64()
        ));
        let mut b = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
        b.create(&dir)?;
        Ok(Self(dir))
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
