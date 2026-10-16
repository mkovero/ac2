//! The OSC cues of the Leq dialog: under the limits, folded until shown, one row per cued
//! subject — the meter as a whole first, then each Leq window, each peak limit, each band
//! window and the predicted window — with a typed message per state (`/light/1 1.0`,
//! [`ac2_scene::osc::parse_message`]) and whether the level is appended.

use ac2_proto::model::{CueState, OscCue, OscMessage, PeakQuantity};
use ac2_scene::osc::{STATES, message_text, parse_message};

/// A column of a cue row, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CueCol {
    /// The message for a state.
    State(CueState),
    /// Whether the level is appended to each message.
    Append,
}

impl CueCol {
    /// Left to right: the states in [`STATES`] order, then the toggle.
    pub const ALL: [CueCol; 5] = [
        CueCol::State(STATES[0]),
        CueCol::State(STATES[1]),
        CueCol::State(STATES[2]),
        CueCol::State(STATES[3]),
        CueCol::Append,
    ];

    /// Column heading.
    pub fn title(self) -> &'static str {
        match self {
            CueCol::State(s) => ac2_scene::osc::field_title(s),
            CueCol::Append => "Level",
        }
    }

    pub(super) fn is_text(self) -> bool {
        matches!(self, CueCol::State(_))
    }

    fn index(s: CueState) -> usize {
        STATES.iter().position(|x| *x == s).unwrap_or(0)
    }
}

/// What a cue row stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CueRow {
    /// The whole meter: the worst state of its limits.
    Meter,
    /// Leq window `i`.
    Window(usize),
    /// A peak limit (listed while it has a limit).
    Peak(PeakQuantity),
    /// Band window `i`.
    Band(usize),
    /// The band meter's predicted window.
    Predicted,
}

/// One subject's cue being edited: a typed line per state, and the append toggle.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CueEdit {
    /// In [`STATES`] order.
    pub text: [String; 4],
    pub append: bool,
    /// The cue as it came: given back untouched while nothing was edited (a cue set from
    /// the CLI may append the level to some messages only).
    before: Option<Box<OscCue>>,
}

impl CueEdit {
    pub fn of(c: Option<&OscCue>) -> Self {
        let Some(c) = c else {
            return Self::default();
        };
        let msg = |s| c.get(s).map_or_else(String::new, message_text);
        Self {
            text: STATES.map(msg),
            append: STATES
                .iter()
                .any(|s| c.get(*s).is_some_and(|m| m.append_level)),
            before: Some(Box::new(c.clone())),
        }
    }

    /// The typed line for `s`.
    pub fn text_of(&self, s: CueState) -> &str {
        &self.text[CueCol::index(s)]
    }

    pub(super) fn text_mut(&mut self, s: CueState) -> &mut String {
        &mut self.text[CueCol::index(s)]
    }

    /// How many states have a message.
    pub fn set(&self) -> usize {
        self.text.iter().filter(|t| !t.trim().is_empty()).count()
    }

    /// The first field that does not read as a message, and why (shown beside the row).
    pub fn error(&self) -> Option<(CueState, String)> {
        STATES.iter().find_map(|s| {
            parse_message(self.text_of(*s), self.append)
                .err()
                .map(|e| (*s, e))
        })
    }

    /// The cue as typed: `None` when no state has a message.
    pub fn cue(&self) -> Result<Option<Box<OscCue>>, String> {
        let mut c = OscCue::default();
        for s in STATES {
            let m: Option<OscMessage> = parse_message(self.text_of(s), self.append)?;
            *match s {
                CueState::Ok => &mut c.ok,
                CueState::Near => &mut c.near,
                CueState::Over => &mut c.over,
                CueState::Fault => &mut c.fault,
            } = m;
        }
        if let Some(b) = &self.before {
            let shown = Self::of(Some(b));
            if shown.text == self.text && shown.append == self.append {
                return Ok(Some(b.clone()));
            }
        }
        Ok((c != OscCue::default()).then(|| Box::new(c)))
    }

    /// [`Self::cue`] with the subject named in the refusal.
    pub(super) fn checked(&self, name: &str) -> Result<Option<Box<OscCue>>, String> {
        self.cue().map_err(|e| format!("OSC cue of {name}: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::{OscArg, OscPath};

    fn m(path: &str, append: bool) -> OscMessage {
        OscMessage {
            path: OscPath::new(path).expect("test value"),
            args: vec![OscArg::Float(1.0)],
            append_level: append,
        }
    }

    #[test]
    fn a_cue_reads_back_and_an_untouched_one_stays_as_it_came() {
        // Appended on one message only: no single toggle says that, so it stays untouched.
        let c = OscCue {
            over: Some(m("/light/red", true)),
            ok: Some(m("/light/green", false)),
            ..Default::default()
        };
        let e = CueEdit::of(Some(&c));
        assert_eq!(e.text_of(CueState::Over), "/light/red 1.0");
        assert_eq!(e.text_of(CueState::Near), "");
        assert!(e.append);
        assert_eq!(e.set(), 2);
        assert_eq!(e.cue(), Ok(Some(Box::new(c.clone()))));
        // Edited: the toggle applies to every message.
        let mut e2 = e.clone();
        e2.text_mut(CueState::Near).push_str("/light/amber 1.0");
        let got = e2.cue().expect("test value").expect("test value");
        assert!(got.ok.as_ref().expect("test value").append_level);
        assert_eq!(got.near, Some(m("/light/amber", true)));
        // Emptied: no cue.
        let mut e3 = e;
        e3.text = Default::default();
        assert_eq!(e3.cue(), Ok(None));
    }

    #[test]
    fn a_bad_line_names_its_field() {
        let mut e = CueEdit::default();
        e.text_mut(CueState::Near).push_str("light 1");
        assert_eq!(
            e.error(),
            Some((CueState::Near, "an OSC address starts with '/'".into()))
        );
        assert_eq!(
            e.checked("LAeq 15 min"),
            Err("OSC cue of LAeq 15 min: an OSC address starts with '/'".into())
        );
    }
}
