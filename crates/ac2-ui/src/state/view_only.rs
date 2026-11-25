//! An app the rig authorized as a viewer: it shows everything and refuses every change
//! before sending it, saying why, instead of a daemon refusal per action.

use super::meas_edits::Unsent;
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

    /// `out[from..]`, what the rig in `rig` is asked, made ready to go: what a viewer may
    /// not send taken out, the measurement edits chained on that rig's.
    pub(super) fn finish_requests(&mut self, out: &mut Vec<Request>, from: usize) {
        let mut mine = out.split_off(from.min(out.len()));
        self.refuse_view_only(&mut mine);
        self.chain_meas_edits(&mut mine);
        out.append(&mut mine);
    }

    /// Takes out of `out` what a viewer may not send, with one plain message for them; what
    /// waits for the reply of a call taken out is answered as refused, and what that
    /// answer asks goes the same way. Quitting goes on without the session steps before
    /// it: the working state is not the viewer's to save.
    fn refuse_view_only(&mut self, out: &mut Vec<Request>) {
        if !self.view_only() {
            return;
        }
        let mut refused = 0;
        let mut todo = std::mem::take(out);
        while !todo.is_empty() {
            let mut asked = Vec::new();
            for r in todo {
                if r.viewer_may_send() {
                    out.push(r);
                    continue;
                }
                refused += 1;
                match r {
                    Request::Sessions(job) if job.steps.contains(&SessionStep::Quit) => {
                        out.push(Request::Sessions(SessionJob {
                            steps: vec![SessionStep::Quit],
                        }));
                    }
                    Request::Call { what, .. } => {
                        self.answer_unsent(&what, Unsent::ViewOnly, &mut asked);
                    }
                    _ => {}
                }
            }
            todo = asked;
        }
        if refused > 0 {
            self.warn(format!(
                "not sent: {}",
                ac2_scene::top_bar::VIEW_ONLY_DETAIL
            ));
        }
    }
}
