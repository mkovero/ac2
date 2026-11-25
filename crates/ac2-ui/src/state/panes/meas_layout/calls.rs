//! The answers to this app's `meas.layout` calls: a reply, or the call never sent (a viewer
//! refuses it, or the measurement's edit chain dropped it).

use crate::state::*;

impl AppState {
    /// A reply to a `meas.layout` call of this app's: `true` when it was one. Said only
    /// when the measurement got its first layout or the call failed: every pane edit
    /// answered aloud would bury what the edit itself said.
    /// The edit held behind the call goes now.
    pub(in crate::state) fn layout_reply(
        &mut self,
        what: &str,
        result: &Result<(), String>,
        out: &mut Vec<Request>,
    ) -> bool {
        self.layout_answered(what, result.as_ref().map(|_| ()).map_err(Some), out)
    }

    /// A `meas.layout` call of this app's that was not sent: answered as a refusal, with
    /// nothing said, as what dropped it says so once for every edit it dropped. `true`
    /// when it was one.
    pub(in crate::state) fn layout_unsent(&mut self, what: &str, out: &mut Vec<Request>) -> bool {
        self.layout_answered(what, Err(None), out)
    }

    fn layout_answered(
        &mut self,
        what: &str,
        result: Result<(), Option<&String>>,
        out: &mut Vec<Request>,
    ) -> bool {
        let Some(i) = self
            .rig
            .layout_queue
            .calls
            .iter()
            .position(|c| c.what == what && !c.answered)
        else {
            return false;
        };
        let meas = self.rig.layout_queue.calls[i].meas;
        match result {
            Ok(()) => {
                let c = &mut self.rig.layout_queue.calls[i];
                c.answered = true;
                let first = c.first;
                if c.applied {
                    self.rig.layout_queue.calls.remove(i);
                }
                if first {
                    self.toast(what);
                }
            }
            Err(e) => {
                self.rig.layout_queue.calls.remove(i);
                if let Some(e) = e {
                    self.fault(format!("{what}: {e}"));
                }
            }
        }
        if let Some(l) = self.rig.layout_queue.queued.remove(&meas) {
            self.send_layout(meas, l, out);
        }
        true
    }
}
