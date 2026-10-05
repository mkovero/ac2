//! The acoustic calibration dialog of the calibrations view (`docs/design/q7-calibration.md`
//! §2, §12): a calibrator on the mic at a known level (94 or 114 dB SPL, usually 1 kHz)
//! while the daemon reads the input's level; `cal.spl` stores the input's sensitivity and
//! names the mic on the input.
//!
//! Keyboard-first like the electrical dialog it sits beside: ↑/↓ (Tab) move between the
//! fields, ←/→ step the level (94 ↔ 114 dB) and the tone (1 kHz ↔ 250 Hz), typing edits the
//! focused field, Enter reads and stores, Esc closes. A refusal (no signal, not steady yet,
//! clipping) stays in the dialog to retry.
//!
//! Pure data: the reducer turns Enter into a `cal.spl` request and hands the reply back
//! through [`AcousticDialog::reply`].

use ac2_proto::Command;
use ac2_proto::cal;
use ac2_proto::model::{CalMethod, State};
use ac2_proto::units::{DbSpl, Hz};

use crate::electrical_dialog::parse_freq;

/// The fields, top to bottom.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Field {
    /// The mic's name (what the calibration is stored for).
    Mic,
    /// The calibrator's level.
    #[default]
    Level,
    /// The calibrator's tone.
    Freq,
}

const FIELDS: [Field; 3] = [Field::Mic, Field::Level, Field::Freq];

/// The levels calibrators produce, ←/→ steps between them.
pub const LEVELS: [&str; 2] = ["94 dB", "114 dB"];
/// The tones calibrators produce: 1 kHz (where A weighting is 0 dB) and the 250 Hz of
/// pistonphones.
pub const TONES: [&str; 2] = ["1 kHz", "250 Hz"];

/// The `what` of the dialog's request: its reply comes back to the dialog by it.
pub const WHAT_PREFIX: &str = "acoustic calibration of input ";

#[derive(Clone, Debug, PartialEq)]
pub struct AcousticDialog {
    pub input: u16,
    pub mic: String,
    pub level: String,
    pub freq: String,
    pub focus: Field,
    /// The input's current calibration of this mic when it is electrical: the calibrator
    /// replaces it (it measures the capsule too), said in the dialog.
    pub replaces: Option<String>,
    /// A request is out.
    pub pending: Option<String>,
    pub error: Option<String>,
}

fn input_no(c: u16) -> u32 {
    u32::from(c) + 1
}

/// `94 dB`, `114`, `94 dB SPL` → dB SPL, 70 … 150 (calibrators sit at 94, 104, 114, 124).
pub fn parse_level(t: &str) -> Result<DbSpl, String> {
    let v = crate::state::parse_number(t, &["db spl", "dbspl", "db"])
        .map_err(|e| format!("calibrator level: {e}"))?;
    if (70.0..=150.0).contains(&v) {
        Ok(DbSpl(v))
    } else {
        Err("the calibrator level is 70 … 150 dB SPL (94 or 114 on most calibrators)".into())
    }
}

impl AcousticDialog {
    /// The dialog for `input`, the mic named on it (when it has one) prefilled.
    pub fn new(s: &State, input: u16) -> Self {
        let mic = cal::input_setup(&s.inputs, input).mic.unwrap_or_default();
        let replaces = s
            .session
            .open
            .as_ref()
            .and_then(|o| {
                s.calibrations.iter().find(|e| {
                    e.key.device == o.input_device && e.key.channel == input && e.key.mic == mic
                })
            })
            .filter(|e| matches!(e.spl.method, CalMethod::Electrical { .. }))
            .map(ac2_scene::cal::calibrator);
        Self {
            input,
            focus: if mic.is_empty() {
                Field::Mic
            } else {
                Field::Level
            },
            mic,
            level: LEVELS[0].into(),
            freq: TONES[0].into(),
            replaces,
            pending: None,
            error: None,
        }
    }

    /// `Acoustic calibration · in 2 · MM1 34804`.
    pub fn title(&self) -> String {
        let mic = self.mic.trim();
        if mic.is_empty() {
            format!("Acoustic calibration · in {}", input_no(self.input))
        } else {
            format!("Acoustic calibration · in {} · {mic}", input_no(self.input))
        }
    }

    /// What to do before Enter.
    pub fn instructions(&self) -> &'static str {
        ac2_scene::cal::acoustic_steps()
    }

    /// ↑/↓, wrapping.
    pub fn move_focus(&mut self, d: i32) {
        let i = FIELDS.iter().position(|f| *f == self.focus).unwrap_or(0) as i32;
        self.focus = FIELDS[(i + d).rem_euclid(FIELDS.len() as i32) as usize];
    }

    /// ←/→: the next calibrator level or tone.
    pub fn step(&mut self) {
        let next = |cur: &str, all: &[&str]| {
            let i = all
                .iter()
                .position(|x| *x == cur.trim())
                .map_or(0, |i| i + 1);
            all[i % all.len()].to_owned()
        };
        match self.focus {
            Field::Mic => return,
            Field::Level => self.level = next(&self.level, &LEVELS),
            Field::Freq => self.freq = next(&self.freq, &TONES),
        }
        self.error = None;
    }

    fn field(&mut self) -> &mut String {
        match self.focus {
            Field::Mic => &mut self.mic,
            Field::Level => &mut self.level,
            Field::Freq => &mut self.freq,
        }
    }

    pub fn type_text(&mut self, t: &str) {
        self.error = None;
        let f = self.field();
        f.extend(t.chars().filter(|c| !c.is_control()));
    }

    pub fn backspace(&mut self) {
        self.error = None;
        self.field().pop();
    }

    /// Enter: the request, or why not (shown in the dialog, the field focused).
    pub fn submit(&mut self) -> Option<Command> {
        if self.pending.is_some() {
            return None;
        }
        self.error = None;
        let parsed = (|| {
            let mic = self.mic.trim().to_owned();
            if mic.is_empty() {
                return Err((
                    Field::Mic,
                    "name the mic: the calibration is stored for this mic on this input".into(),
                ));
            }
            cal::check_mic_name(&mic).map_err(|e| (Field::Mic, e))?;
            let level = parse_level(&self.level).map_err(|e| (Field::Level, e))?;
            let freq = parse_freq(&self.freq).map_err(|e| (Field::Freq, e))?;
            Ok((mic, level, freq))
        })();
        let (mic, calibrator_level, calibrator_freq): (String, DbSpl, Hz) = match parsed {
            Ok(p) => p,
            Err((f, e)) => {
                self.focus = f;
                self.error = Some(e);
                return None;
            }
        };
        self.pending = Some(self.what());
        Some(Command::CalSpl {
            input: self.input,
            mic,
            calibrator_level,
            calibrator_freq,
        })
    }

    /// The request's `what`.
    pub fn what(&self) -> String {
        format!(
            "{WHAT_PREFIX}{} ({})",
            input_no(self.input),
            self.mic.trim()
        )
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
/// separately ([`AcousticDialog::type_text`]).
pub fn key(
    d: &mut AcousticDialog,
    chord: &crate::keys::Chord,
) -> Option<crate::cal_view::CalAction> {
    use eframe::egui::Key;
    match chord.key {
        Key::ArrowUp => d.move_focus(-1),
        Key::Tab if chord.shift => d.move_focus(-1),
        Key::ArrowDown | Key::Tab => d.move_focus(1),
        Key::ArrowLeft | Key::ArrowRight => d.step(),
        Key::Enter => {
            return d
                .submit()
                .map(|c| crate::cal_view::CalAction::Calibrate(c, d.what()));
        }
        _ => {}
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::InputSetup;

    fn state(mic: Option<&str>) -> State {
        let mut s = ac2_proto::samples::state();
        s.inputs = vec![InputSetup {
            channel: 1,
            mic: mic.map(str::to_owned),
            curve: ac2_proto::model::CurveChoice::NotChosen,
        }];
        s
    }

    /// The mic named on the input is prefilled and the focus starts on the level; Enter
    /// sends `cal.spl` at 94 dB, 1 kHz; ←/→ step to 114 dB and 250 Hz.
    #[test]
    fn prefilled_and_stepped() {
        let mut d = AcousticDialog::new(&state(Some("MM1 34804")), 1);
        assert_eq!(d.title(), "Acoustic calibration · in 2 · MM1 34804");
        assert_eq!(d.focus, Field::Level);
        assert_eq!(
            d.submit(),
            Some(Command::CalSpl {
                input: 1,
                mic: "MM1 34804".into(),
                calibrator_level: DbSpl(94.0),
                calibrator_freq: Hz(1000.0),
            })
        );
        assert_eq!(d.submit(), None, "one request at a time");
        let what = d.what();
        assert!(!d.reply(&what, &Err("not steady yet: retry".into())));
        assert_eq!(d.error.as_deref(), Some("not steady yet: retry"));
        d.step();
        assert_eq!(d.level, "114 dB");
        d.move_focus(1);
        d.step();
        assert_eq!(d.freq, "250 Hz");
        let Some(Command::CalSpl {
            calibrator_level,
            calibrator_freq,
            ..
        }) = d.submit()
        else {
            panic!("a request")
        };
        assert_eq!(
            (calibrator_level, calibrator_freq),
            (DbSpl(114.0), Hz(250.0))
        );
        assert!(d.reply(&what, &Ok(())), "stored: the dialog closes");
    }

    /// An input without a mic name starts on the mic; Enter without one, or with a bad
    /// level, says why and focuses the field.
    #[test]
    fn refusals_focus_the_field() {
        let mut d = AcousticDialog::new(&state(None), 1);
        assert_eq!(d.focus, Field::Mic);
        assert_eq!(d.title(), "Acoustic calibration · in 2");
        assert_eq!(d.submit(), None);
        assert!(
            d.error
                .as_deref()
                .is_some_and(|e| e.contains("name the mic"))
        );
        d.type_text("M30");
        d.focus = Field::Level;
        d.backspace();
        d.backspace();
        d.backspace();
        d.backspace();
        d.backspace();
        d.type_text("9");
        assert_eq!(d.submit(), None);
        assert_eq!(d.focus, Field::Level);
        assert!(d.error.as_deref().is_some_and(|e| e.contains("70 … 150")));
        d.type_text("4");
        assert!(matches!(
            d.submit(),
            Some(Command::CalSpl { calibrator_level: DbSpl(v), .. }) if v == 94.0
        ));
        assert_eq!(d.what(), "acoustic calibration of input 2 (M30)");
    }
}
