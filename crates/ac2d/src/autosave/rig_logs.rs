//! The rig SPL meters' logs on disk: a session directory of their own, kept by a second
//! write thread exactly as the autosave keeps the session's meters' logs ([`SplFiles`]
//! appends, the manifest names the files).
//!
//! It lives apart from the autosave because the autosave belongs to the working session:
//! a load replaces it, `--no-restore` and a damaged manifest move the whole directory
//! aside. A meter that watches a noise limit for the rig must keep its log through all of
//! that, and a restarted daemon must carry on from it, so its Leq windows (computed from
//! the log) come back where they were.
//!
//! Its manifest holds the rig meters only to name their log files; their configuration is
//! the rig settings' (`rig.json`). Writes are not reported back: a failed one is logged,
//! and the next change writes again.

use std::path::{Path, PathBuf};

use ac2_traces::session::Session as SessionData;

use super::{Fingerprint, Job, LogSpec, Restored, Writer};

/// The rig meters' log directory and its write thread.
pub(crate) struct RigLogs {
    dir: PathBuf,
    writer: Writer,
    /// What the last write asked held.
    asked: Option<Fingerprint>,
}

impl RigLogs {
    /// Starts the write thread for `dir`.
    pub(crate) fn spawn(dir: PathBuf) -> std::io::Result<Self> {
        let writer = Writer::spawn_named(dir.clone(), None, "ac2d-rig-logs")?;
        Ok(Self {
            dir,
            writer,
            asked: None,
        })
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// What the directory holds, read at start (a damaged one is set aside, never
    /// deleted).
    pub(crate) fn read(&self) -> Option<Restored> {
        super::restore(&self.dir)
    }

    /// `fp` is on disk already (just read back), its logs `logs` carrying on in their files.
    pub(crate) fn restored(&mut self, fp: Fingerprint, logs: Vec<LogSpec>) {
        self.asked = Some(fp);
        self.writer.send(Job::Logs(logs));
    }

    /// Writes the manifest when what it names changed since the last write asked.
    pub(crate) fn changed(&mut self, data: SessionData, fp: Fingerprint, logs: Vec<LogSpec>) {
        // A rig that never had a meter gets no directory.
        if self.asked.as_ref() == Some(&fp) || (self.asked.is_none() && fp.0.is_empty()) {
            return;
        }
        if self.writer.send(Job::Save { data, logs }) {
            self.asked = Some(fp);
        }
    }

    /// Shutdown: the queued writes and the logs' last rows reach the disk.
    pub(crate) fn finish(&mut self) {
        self.writer.finish();
    }
}
