//! The spectrographs (flat and 3D) scroll on the display's clock
//! ([`ac2_scene::scroll_clock`]): each frame's audio time sets where the time axis runs,
//! and every pass draws it where it stands then. Placed frame by frame, the rows would
//! jump by the audio each frame carries (whole analysis blocks, unevenly), which reads as
//! a stutter; on the display's clock they move the same distance every display frame and
//! the newest frame still shows the pass it arrives.

use super::*;

use std::time::Instant;

impl RigState {
    /// Measurement `meas`'s history took a frame ending at audio time `audio_s`, applied
    /// at `now`: its time axis follows.
    pub(super) fn keep_scroll(&mut self, meas: MeasId, now: Instant, audio_s: f64) {
        self.folded
            .scroll
            .entry(meas)
            .or_default()
            .observe(now, audio_s);
    }
}

impl AppState {
    /// How many history slots past the end of pane `pane`'s newest slot its spectrograph's
    /// time axis stands at `now`
    /// ([`ac2_scene::spectrograph::SpectrographHistory::lead_slots`]); 0 (drawn at their
    /// places) with the scroll off (Settings › Display, "jumps to each new frame"), or
    /// without a history.
    pub fn scroll_lead(&self, pane: PaneId, now: Instant) -> f64 {
        if !self.prefs.spec_ease {
            return 0.0;
        }
        // A hidden measurement's view draws no rows.
        let Some(m) = self.pane_meas(pane).filter(|m| !self.meas_hidden(m)) else {
            return 0.0;
        };
        match (
            self.rig.folded.scroll.get(&m.id),
            self.rig.folded.spectrographs.get(&m.id),
        ) {
            (Some(c), Some(h)) => h.lead_slots(c.position_s(now)),
            _ => 0.0,
        }
    }

    /// Some spectrograph on screen still scrolls at `now`: its time axis has not reached
    /// where it waits for a stalled stream. Only then does the pacer run for it.
    pub fn spectrographs_scrolling(&self, now: Instant) -> bool {
        self.prefs.spec_ease
            && !self.rig.folded.scroll.is_empty()
            && self.visible_panes().into_iter().any(|p| {
                self.layout.view(p).is_some_and(|v| {
                    v.kind == PaneKind::Spectrum && v.modes.spectrum.spectrograph()
                }) && self
                    .pane_meas(p)
                    .filter(|m| !self.meas_hidden(m))
                    .and_then(|m| self.rig.folded.scroll.get(&m.id))
                    .is_some_and(|c| c.moving(now))
            })
    }
}
