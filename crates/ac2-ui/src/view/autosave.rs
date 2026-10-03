//! The autosave status the top bar shows: `autosaved 5 min ago`, `saving…`, or the failure
//! in warning colour (text from `ac2_scene::autosave`).

use ac2_proto::units::WallNs;
use ac2_scene::autosave::{AutosaveLabel, autosave_label};
use ac2_scene::time::ClockOffset;

use crate::state::AppState;

impl AppState {
    /// The daemon's autosave status as shown, or `None` when it does not autosave (or
    /// nothing is connected).
    pub fn autosave_label(&self, client_now: WallNs) -> Option<AutosaveLabel> {
        let daemon = self.daemon()?;
        let offset = ClockOffset(
            self.mirror
                .as_ref()
                .and_then(|v| v.clock_offset_ns)
                .map_or(0, |o| {
                    o.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
                }),
        );
        autosave_label(&daemon.autosave, client_now, offset)
    }
}
