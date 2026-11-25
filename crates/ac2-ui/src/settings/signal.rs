//! The stimulus rows of the Audio I/O page (the signal's, then this app's stop and level):
//! which row has the focus and the sine frequency or the level being typed.

use ac2_scene::stim_rows::StimRow;
use ac2_scene::stimulus::SignalRow;

use super::{CeilingEdit, Page, Settings};

/// The stimulus signal rows of the Audio I/O page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SignalRows {
    /// The focused signal row; `None`: the focus is elsewhere on the page.
    pub focus: Option<SignalRow>,
    /// The focused row of this app's stimulus settings after the signal rows.
    pub app: Option<StimRow>,
    /// The frequency or the level being typed; empty shows the one set.
    pub text: String,
    pub error: Option<String>,
}

/// A stimulus row of the page, in page order: [`StimFocus::rows`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StimFocus {
    Signal(SignalRow),
    App(StimRow),
}

impl StimFocus {
    /// The rows shown (`sine`: the sine's own too), top to bottom.
    pub fn rows(sine: bool) -> Vec<StimFocus> {
        SignalRow::shown(sine)
            .iter()
            .map(|r| StimFocus::Signal(*r))
            .chain(StimRow::ALL.map(StimFocus::App))
            .collect()
    }
}

impl SignalRows {
    pub(super) fn leave(&mut self) {
        *self = Self::default();
    }

    /// The focused stimulus row, if one is.
    pub fn at(&self) -> Option<StimFocus> {
        self.focus
            .map(StimFocus::Signal)
            .or(self.app.map(StimFocus::App))
    }

    /// Focuses `row`, dropping what was typed on another.
    pub fn set(&mut self, row: StimFocus) {
        self.leave();
        match row {
            StimFocus::Signal(r) => self.focus = Some(r),
            StimFocus::App(r) => self.app = Some(r),
        }
    }

    /// The focused row takes typed text: the sine's frequency or the level.
    pub fn typed(&self) -> bool {
        self.focus == Some(SignalRow::Frequency) || self.app == Some(StimRow::Level)
    }

    /// Enter on the frequency row: the frequency typed, or the reason it is refused.
    /// `rate_hz`: the open stream's sample rate.
    pub fn enter(&mut self, rate_hz: Option<u32>) -> Option<ac2_proto::units::Hz> {
        match ac2_scene::stimulus::typed_sine_freq(&self.text, rate_hz) {
            Ok(hz) => {
                *self = Self {
                    focus: self.focus,
                    app: self.app,
                    ..Self::default()
                };
                Some(hz)
            }
            Err(e) => {
                self.error = Some(e);
                None
            }
        }
    }
}

impl Settings {
    /// Shows the Audio I/O page focused on a stimulus signal row.
    pub fn focus_signal(&mut self, row: SignalRow) {
        self.show(Page::AudioIo);
        self.focus_want = None;
        self.on_ceiling = false;
        self.ceiling = CeilingEdit::default();
        self.stream.finish_edit();
        self.stream.detect_cancel();
        self.signal.set(StimFocus::Signal(row));
    }

    /// Shows the Audio I/O page focused on a row of this app's stimulus settings.
    pub fn focus_stim(&mut self, row: StimRow) {
        self.focus_signal(SignalRow::Signal);
        self.signal.set(StimFocus::App(row));
    }
}
