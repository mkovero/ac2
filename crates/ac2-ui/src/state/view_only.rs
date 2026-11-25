//! An app the rig authorized as a viewer: it shows everything and refuses every change
//! before sending it, saying why, instead of a daemon refusal per action.

use super::*;
use crate::sessions::{SessionJob, SessionStep};
use ac2_proto::model::ClientRole;

impl AppState {
    /// The rig shown authorized this app's key as a viewer.
    pub fn view_only(&self) -> bool {
        self.rig
            .mirror
            .as_ref()
            .is_some_and(|m| m.role == Some(ClientRole::Viewer))
    }

    /// Takes out of `out` what a viewer may not send, with one plain message for them.
    /// Quitting goes on without the session steps before it: the working state is not the
    /// viewer's to save.
    pub(super) fn refuse_view_only(&mut self, out: &mut Vec<Request>) {
        if !self.view_only() {
            return;
        }
        let mut refused = 0;
        for r in std::mem::take(out) {
            if r.viewer_may_send() {
                out.push(r);
                continue;
            }
            refused += 1;
            if let Request::Sessions(job) = &r
                && job.steps.contains(&SessionStep::Quit)
            {
                out.push(Request::Sessions(SessionJob {
                    steps: vec![SessionStep::Quit],
                }));
            }
        }
        if refused > 0 {
            self.warn(format!(
                "not sent: {}",
                ac2_scene::top_bar::VIEW_ONLY_DETAIL
            ));
        }
    }
}
