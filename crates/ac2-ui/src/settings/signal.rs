//! The stimulus signal rows of the Audio I/O page: which row has the focus and the sine
//! frequency being typed.

use ac2_scene::stimulus::SignalRow;

use super::{CeilingEdit, Page, Settings};

/// The stimulus signal rows of the Audio I/O page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SignalRows {
    /// The focused row; `None`: the focus is elsewhere on the page.
    pub focus: Option<SignalRow>,
    /// The frequency being typed; empty shows the one set.
    pub text: String,
    pub error: Option<String>,
}

impl SignalRows {
    pub(super) fn leave(&mut self) {
        *self = Self::default();
    }

    /// Enter on the frequency row: the frequency typed, or the reason it is refused.
    /// `rate_hz`: the open stream's sample rate.
    pub fn enter(&mut self, rate_hz: Option<u32>) -> Option<ac2_proto::units::Hz> {
        match ac2_scene::stimulus::typed_sine_freq(&self.text, rate_hz) {
            Ok(hz) => {
                *self = Self {
                    focus: self.focus,
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
        self.signal.leave();
        self.signal.focus = Some(row);
    }
}
