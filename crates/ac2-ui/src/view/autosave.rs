//! The autosave indicator in the top bar: `autosaved 5 min ago`, `saving…`, or the failure
//! in warning colour (text from `ac2_scene::autosave`).

use ac2_proto::units::WallNs;
use ac2_scene::autosave::{AutosaveLabel, AutosaveTone, autosave_label};
use ac2_scene::time::ClockOffset;
use eframe::egui::{self, RichText};

use crate::app::App;
use crate::state::AppState;
use crate::theme::Chrome;

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

/// The indicator, if the daemon autosaves.
pub(super) fn autosave(app: &App, ui: &mut egui::Ui, ch: &Chrome) {
    let Some(l) = app.state.autosave_label(super::now().wall) else {
        return;
    };
    let color = match l.tone {
        AutosaveTone::Quiet | AutosaveTone::Busy => ch.dim,
        AutosaveTone::Warning => ch.warn,
    };
    ui.label(RichText::new(l.text).color(color))
        .on_hover_text(l.detail);
    ui.separator();
}
