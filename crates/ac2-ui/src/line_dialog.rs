//! The line calibration dialog of the calibrations view (`docs/design/programme-meter.md`
//! §1.1): a steady tone's RMS voltage, measured at a line input with a meter while the
//! daemon reads the input's level, gives the volts at 0 dBFS and so the dBu scale of the
//! input. No capsule, no sensitivity: the electrical calibration stops at `V_FS`.
//!
//! Keyboard-first like the other dialogs: typing edits the voltage, Enter stores, Esc
//! closes. Pure data: the reducer turns Enter into a `cal.line` request carrying the volts
//! alone (the daemon measures the tone's level itself and refuses a missing, clipping or
//! unsteady one) and hands the reply back through [`LineDialog::reply`].

use ac2_proto::Command;
use ac2_scene::unit_text::VoltsArg;

/// The `what` of the dialog's request: its reply comes back to the dialog by it.
pub const WHAT_PREFIX: &str = "line calibration of input ";

#[derive(Clone, Debug, PartialEq)]
pub struct LineDialog {
    pub input: u16,
    /// The voltage as typed.
    pub volts: String,
    /// A request is out.
    pub pending: Option<String>,
    pub error: Option<String>,
}

fn input_no(c: u16) -> u32 {
    u32::from(c) + 1
}

impl LineDialog {
    pub fn new(input: u16) -> Self {
        Self {
            input,
            volts: String::new(),
            pending: None,
            error: None,
        }
    }

    /// `Line calibration · in 3`.
    pub fn title(&self) -> String {
        format!("Line calibration · in {}", input_no(self.input))
    }

    /// What to do before Enter.
    pub fn method_text(&self) -> &'static str {
        "Feed the input a steady tone (a console's oscillator, 1 kHz, around nominal level) \
         and measure its RMS voltage at the input with a true-RMS meter while the level below \
         holds still."
    }

    pub fn type_text(&mut self, t: &str) {
        self.volts.extend(t.chars().filter(|c| !c.is_control()));
        self.error = None;
    }

    pub fn backspace(&mut self) {
        self.volts.pop();
    }

    /// Enter: the request with the volts, or why not (shown in the dialog). The level is
    /// the daemon's to measure; its refusals come back through [`Self::reply`].
    pub fn submit(&mut self) -> Option<Command> {
        if self.pending.is_some() {
            return None;
        }
        self.error = None;
        if self.volts.trim().is_empty() {
            self.error = Some("type the voltage the meter shows, e.g. 1.228 V".into());
            return None;
        }
        let volts = match self.volts.parse::<VoltsArg>() {
            Ok(v) => v.0,
            Err(e) => {
                self.error = Some(e.0);
                return None;
            }
        };
        self.pending = Some(self.what());
        Some(Command::CalLine {
            channel: self.input,
            volts,
        })
    }

    /// The request's `what`.
    pub fn what(&self) -> String {
        format!("{WHAT_PREFIX}{}", input_no(self.input))
    }

    /// A reply to a request with `what`. `true` when it was this dialog's and it is done
    /// (stored: the dialog closes); a refusal stays in the dialog to retry.
    pub fn reply(&mut self, what: &str, result: &Result<(), String>) -> bool {
        if self.pending.as_deref() != Some(what) {
            return false;
        }
        self.pending = None;
        match result {
            Ok(()) => true,
            Err(e) => {
                self.error = Some(e.clone());
                false
            }
        }
    }
}

/// A key in the dialog: the action for the calibrations view, if any. Text arrives
/// separately ([`LineDialog::type_text`]).
pub fn key(d: &mut LineDialog, chord: &crate::keys::Chord) -> Option<crate::cal_view::CalAction> {
    if chord.key == eframe::egui::Key::Enter {
        return d
            .submit()
            .map(|c| crate::cal_view::CalAction::Calibrate(c, d.what()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_sends_the_volts_alone() {
        let mut d = LineDialog::new(2);
        assert_eq!(d.title(), "Line calibration · in 3");
        assert!(d.submit().is_none());
        assert!(d.error.as_deref().is_some_and(|e| e.contains("voltage")));
        d.type_text("1.228 V");
        let cmd = d.submit().expect("sent");
        match cmd {
            Command::CalLine { channel, volts } => {
                assert_eq!(channel, 2);
                assert!((volts.0 - 1.228).abs() < 1e-9);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(d.what(), "line calibration of input 3");
        // One request at a time.
        assert!(d.submit().is_none());
        // A refusal (the daemon's: no tone, clipping, not steady) stays to retry; success
        // closes.
        assert!(!d.reply("line calibration of input 3", &Err("no".into())));
        assert_eq!(d.error.as_deref(), Some("no"));
        d.submit();
        assert!(d.reply("line calibration of input 3", &Ok(())));
    }
}
