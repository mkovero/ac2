//! The reducer's side of this app's stimulus rows of Settings › Audio I/O: whether the
//! transfers stop with the stimulus, and the stimulus level beside the max level it may
//! reach.

use ac2_scene::stim_rows::StimRow;
use eframe::egui::Key;

use super::{AppState, Overlay, PromptKind};
use crate::conn::Request;
use crate::keys::Chord;

impl AppState {
    /// A key on a row of this app's stimulus settings (↑/↓/Tab move, handled before).
    pub(super) fn stim_row_key(
        &mut self,
        row: StimRow,
        chord: Chord,
        swallow: Option<char>,
        out: &mut Vec<Request>,
    ) {
        match (row, chord.key) {
            (
                StimRow::StopTransfers,
                Key::ArrowLeft | Key::ArrowRight | Key::Space | Key::Enter,
            ) => self.toggle_stop_transfers(),
            (StimRow::Level, Key::Enter) => self.apply_level_row(out),
            _ => self.swallow_text = swallow,
        }
    }

    /// The mouse on a row: focuses it; ‹/› on the transfer row turn it over.
    pub(super) fn stim_row_click(&mut self, row: StimRow, d: i32) {
        if let Overlay::Settings(s) = &mut self.overlay
            && s.signal.app != Some(row)
        {
            s.focus_stim(row);
        }
        if row == StimRow::StopTransfers && d != 0 {
            self.toggle_stop_transfers();
        }
    }

    fn toggle_stop_transfers(&mut self) {
        self.prefs.stop_transfers_with_stimulus = !self.prefs.stop_transfers_with_stimulus;
        self.prefs_dirty = true;
    }

    /// Enter on the level row: the level typed, as the level prompt takes it (the same
    /// checks and words), for the rig shown.
    fn apply_level_row(&mut self, out: &mut Vec<Request>) {
        let Overlay::Settings(s) = &self.overlay else {
            return;
        };
        let text = s.signal.text.trim().to_owned();
        let r = if text.is_empty() {
            Err(ac2_scene::stim_rows::LEVEL_WANTED.to_owned())
        } else {
            self.set_level_text(&text, out)
        };
        if let Overlay::Settings(s) = &mut self.overlay {
            match r {
                Ok(()) => {
                    s.signal.text.clear();
                    s.signal.error = None;
                }
                Err(e) => s.signal.error = Some(e),
            }
        }
    }

    /// The title of a prompt: the level's states the max level it may reach.
    pub fn prompt_title(&self, kind: PromptKind) -> String {
        match kind {
            PromptKind::StimulusLevel => ac2_scene::stim_rows::level_prompt_title(self.ceiling()),
            k => k.label().to_owned(),
        }
    }
}
