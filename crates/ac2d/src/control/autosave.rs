//! The control thread's side of the autosave (`crate::autosave`): restore at start, a
//! debounced write after changes to the measurements or traces, the status on the wire.

use std::time::Instant;

use ac2_proto::event::Change;
use ac2_proto::model::{Autosave, AutosaveState};
use ac2_proto::units::WallNs;

use super::Control;
use crate::autosave::{self, Fingerprint};

impl Control {
    fn fingerprint(&self) -> Fingerprint {
        (
            self.saved_measurements(),
            self.store.state().traces.clone(),
            self.spl_log_totals(),
        )
    }

    /// Commits the autosave status if it changed.
    fn set_autosave(&mut self, state: AutosaveState, saved_at: Option<WallNs>) {
        let a = Autosave { state, saved_at };
        if self.store.state().autosave != a {
            self.commit(Change::Autosave(a));
        }
    }

    /// Loads the autosave (disarmed, as `file.load`; the audio device is not opened), or
    /// with `--no-restore` moves it aside.
    pub(super) fn start_autosave(&mut self) {
        let Some(dir) = self.autosave.as_ref().map(|a| a.dir.clone()) else {
            return;
        };
        if !self.restore {
            autosave::skip_restore(&dir);
            return;
        }
        let Some((data, from)) = autosave::restore(&dir) else {
            tracing::info!("autosave: nothing to restore in {}", dir.display());
            return;
        };
        let (n_meas, n_traces, at) = (data.measurements.len(), data.traces.len(), data.saved_at);
        // The restore's own commits are not changes to write back.
        let saver = self.autosave.take();
        let replaced = self.replace_session(None, data);
        self.autosave = saver;
        match replaced {
            Ok(epoch) => {
                tracing::info!(
                    "autosave restored from {}: {n_meas} measurements, {n_traces} traces \
                     (epoch {}, disarmed, no audio session opened)",
                    from.display(),
                    epoch.0
                );
                let fp = self.fingerprint();
                if let Some(a) = self.autosave.as_mut() {
                    a.restored(fp);
                }
                self.set_autosave(AutosaveState::Saved, Some(at));
            }
            Err(e) => {
                let to = autosave::set_aside(&from, "damaged");
                tracing::warn!(
                    "autosave in {} not restored: {}; set aside as {}",
                    from.display(),
                    e.msg,
                    to.map_or_else(|| "-".into(), |t| t.display().to_string())
                );
            }
        }
    }

    /// The measurements or traces changed.
    pub(super) fn autosave_changed(&mut self) {
        if self.autosave.is_none() {
            return;
        }
        let fp = self.fingerprint();
        let Some(a) = self.autosave.as_mut() else {
            return;
        };
        let pending = a.changed(&fp, Instant::now());
        let cur = self.store.state().autosave.clone();
        match (&cur.state, pending) {
            (AutosaveState::Saved, true) => self.set_autosave(AutosaveState::Pending, cur.saved_at),
            (AutosaveState::Pending, false) => {
                self.set_autosave(AutosaveState::Saved, cur.saved_at);
            }
            // A failure stays shown until a write succeeds.
            _ => {}
        }
    }

    /// The debounce ran out: hands the current state to the write thread.
    pub(super) fn autosave_write(&mut self) {
        let data = match self.session_data() {
            Ok(d) => d,
            Err(e) => {
                let saved_at = self.store.state().autosave.saved_at;
                if let Some(a) = self.autosave.as_mut() {
                    a.finished(false, Instant::now());
                }
                self.set_autosave(AutosaveState::Failed { reason: e.msg }, saved_at);
                return;
            }
        };
        let fp = self.fingerprint();
        let Some(a) = self.autosave.as_mut() else {
            return;
        };
        if !a.write(data, fp) {
            let cur = self.store.state().autosave.clone();
            if cur.state == AutosaveState::Pending {
                self.set_autosave(AutosaveState::Saved, cur.saved_at);
            }
        }
    }

    /// The write thread reported back.
    pub(super) fn autosaved(&mut self, result: Result<WallNs, String>) {
        let Some(a) = self.autosave.as_mut() else {
            return;
        };
        let more = a.finished(result.is_ok(), Instant::now());
        let prev = self.store.state().autosave.saved_at;
        match result {
            Ok(at) => self.set_autosave(
                if more {
                    AutosaveState::Pending
                } else {
                    AutosaveState::Saved
                },
                Some(at),
            ),
            Err(reason) => self.set_autosave(AutosaveState::Failed { reason }, prev),
        }
    }

    /// Shutdown: whatever is not on disk yet is written before the daemon exits.
    pub(super) fn flush_autosave(&mut self) {
        let Some(mut a) = self.autosave.take() else {
            return;
        };
        let data = if a.busy() || self.store.state().autosave.state != AutosaveState::Saved {
            self.session_data().ok().map(|d| (d, self.fingerprint()))
        } else {
            None
        };
        a.flush(data);
    }
}
