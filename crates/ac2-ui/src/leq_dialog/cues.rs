//! The cues of the Leq dialog: under the limits, folded until shown, one row per cued
//! subject — the meter as a whole first, then each Leq window, each peak limit, each band
//! window and the predicted window — each with two lines: a typed OSC message per state
//! (`/light/1 1.0`, [`ac2_scene::osc::parse_message`]) and whether the level is appended,
//! then the DMX levels per state (`12=255 13=0`, [`ac2_scene::artnet::parse_cue`]).

use ac2_proto::model::{Cue, CueAction, CueState, OscMessage, PeakQuantity};
use ac2_scene::artnet;
use ac2_scene::osc::{STATES, message_text, parse_message};

/// A cell of a cue row, its OSC line left to right, then its DMX line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CueCol {
    /// The message for a state.
    State(CueState),
    /// Whether the level is appended to each message.
    Append,
    /// The DMX levels a state holds.
    Dmx(CueState),
}

impl CueCol {
    /// The OSC line, left to right: the states in [`STATES`] order, then the toggle.
    pub const ALL: [CueCol; 5] = [
        CueCol::State(STATES[0]),
        CueCol::State(STATES[1]),
        CueCol::State(STATES[2]),
        CueCol::State(STATES[3]),
        CueCol::Append,
    ];

    /// The DMX line, left to right, each state under its message.
    pub const DMX: [CueCol; 4] = [
        CueCol::Dmx(STATES[0]),
        CueCol::Dmx(STATES[1]),
        CueCol::Dmx(STATES[2]),
        CueCol::Dmx(STATES[3]),
    ];

    /// Column heading.
    pub fn title(self) -> &'static str {
        match self {
            CueCol::State(s) | CueCol::Dmx(s) => ac2_scene::osc::field_title(s),
            CueCol::Append => ac2_scene::osc::APPEND_TITLE,
        }
    }

    /// The field as a refusal beside its row names it: `Over`, `Over DMX`.
    pub fn field_name(self) -> String {
        match self {
            CueCol::Dmx(s) => format!("{} {}", ac2_scene::osc::field_title(s), artnet::CUE_LINE),
            c => c.title().to_owned(),
        }
    }

    pub(super) fn is_text(self) -> bool {
        !matches!(self, CueCol::Append)
    }

    /// Where the cell sits in its line: ↑ / ↓ keep it.
    pub(super) fn cell(self) -> usize {
        match self {
            CueCol::State(s) | CueCol::Dmx(s) => Self::index(s),
            CueCol::Append => STATES.len(),
        }
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

/// One subject's cue being edited: a typed message and a typed list of levels per state,
/// and the append toggle.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CueEdit {
    /// The messages, in [`STATES`] order.
    pub text: [String; 4],
    /// The DMX levels, in [`STATES`] order.
    pub dmx: [String; 4],
    pub append: bool,
    /// The cue as it came: given back untouched while nothing was edited (a cue set from
    /// the CLI may append the level to some messages only).
    before: Option<Box<Cue>>,
}

impl CueEdit {
    pub fn of(c: Option<&Cue>) -> Self {
        let Some(c) = c else {
            return Self::default();
        };
        let msg = |s| c.osc(s).map_or_else(String::new, message_text);
        Self {
            text: STATES.map(msg),
            dmx: STATES.map(|s| artnet::cue_text(c.dmx(s))),
            append: STATES
                .iter()
                .any(|s| c.osc(*s).is_some_and(|m| m.append_level)),
            before: Some(Box::new(c.clone())),
        }
    }

    /// The typed line for `s`.
    pub fn text_of(&self, s: CueState) -> &str {
        &self.text[CueCol::index(s)]
    }

    /// The typed levels for `s`.
    pub fn dmx_of(&self, s: CueState) -> &str {
        &self.dmx[CueCol::index(s)]
    }

    /// The typed text of a text cell.
    pub(super) fn cell_mut(&mut self, col: CueCol) -> Option<&mut String> {
        match col {
            CueCol::State(s) => Some(&mut self.text[CueCol::index(s)]),
            CueCol::Dmx(s) => Some(&mut self.dmx[CueCol::index(s)]),
            CueCol::Append => None,
        }
    }

    /// How many states have a message.
    pub fn set(&self) -> usize {
        self.text.iter().filter(|t| !t.trim().is_empty()).count()
    }

    /// How many states hold DMX levels.
    pub fn dmx_set(&self) -> usize {
        self.dmx.iter().filter(|t| !t.trim().is_empty()).count()
    }

    /// The first field that does not read, and why (shown beside the row): the messages
    /// first, then the levels.
    pub fn error(&self) -> Option<(CueCol, String)> {
        self.cue().err()
    }

    /// The cue as typed: `None` when no state does anything; else the first field that does
    /// not read, and why.
    pub fn cue(&self) -> Result<Option<Box<Cue>>, (CueCol, String)> {
        let mut c = Cue::default();
        for s in STATES {
            let osc: Option<OscMessage> =
                parse_message(self.text_of(s), self.append).map_err(|e| (CueCol::State(s), e))?;
            *c.get_mut(s) = Some(CueAction { osc, dmx: vec![] });
        }
        for s in STATES {
            let dmx = artnet::parse_cue(self.dmx_of(s)).map_err(|e| (CueCol::Dmx(s), e))?;
            let slot = c.get_mut(s);
            if let Some(act) = slot.as_mut() {
                act.dmx = dmx;
            }
            if slot.as_ref().is_some_and(CueAction::is_empty) {
                *slot = None;
            }
        }
        if let Some(b) = &self.before {
            let shown = Self::of(Some(b));
            if (&shown.text, &shown.dmx, shown.append) == (&self.text, &self.dmx, self.append) {
                return Ok(Some(b.clone()));
            }
        }
        Ok((c != Cue::default()).then(|| Box::new(c)))
    }

    /// [`Self::cue`] with the subject named in the refusal.
    pub(super) fn checked(&self, name: &str) -> Result<Option<Box<Cue>>, String> {
        self.cue().map_err(|(col, e)| match col {
            CueCol::Dmx(_) => format!("DMX cue of {name}: {e}"),
            _ => format!("OSC cue of {name}: {e}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::{OscArg, OscPath, parse_dmx_list};

    fn m(path: &str, append: bool) -> OscMessage {
        OscMessage {
            path: OscPath::new(path).expect("test value"),
            args: vec![OscArg::Float(1.0)],
            append_level: append,
        }
    }

    fn levels(s: &str) -> Vec<ac2_proto::model::DmxSet> {
        parse_dmx_list(s).expect("test value")
    }

    #[test]
    fn a_cue_reads_back_and_an_untouched_one_stays_as_it_came() {
        // Appended on one message only: no single toggle says that, so it stays untouched.
        let act = |m| {
            Some(CueAction {
                osc: Some(m),
                dmx: vec![],
            })
        };
        let c = Cue {
            over: Some(CueAction {
                osc: Some(m("/light/red", true)),
                dmx: levels("12=255 13=0"),
            }),
            ok: act(m("/light/green", false)),
            fault: Some(CueAction {
                osc: None,
                dmx: levels("12=255"),
            }),
            ..Default::default()
        };
        let e = CueEdit::of(Some(&c));
        assert_eq!(e.text_of(CueState::Over), "/light/red 1.0");
        assert_eq!(e.text_of(CueState::Near), "");
        assert_eq!(e.dmx_of(CueState::Over), "12=255 13=0");
        assert_eq!(e.dmx_of(CueState::Fault), "12=255");
        assert!(e.append);
        assert_eq!((e.set(), e.dmx_set()), (2, 2));
        assert_eq!(e.cue(), Ok(Some(Box::new(c.clone()))));
        // Edited: the toggle applies to every message.
        let mut e2 = e.clone();
        e2.cell_mut(CueCol::State(CueState::Near))
            .expect("text")
            .push_str("/light/amber 1.0");
        let got = e2.cue().expect("test value").expect("test value");
        assert!(got.osc(CueState::Ok).expect("test value").append_level);
        assert_eq!(got.osc(CueState::Near), Some(&m("/light/amber", true)));
        // The levels are kept as typed.
        assert_eq!(got.fault, c.fault);
        assert_eq!(got.dmx(CueState::Over), levels("12=255 13=0"));
        // Emptied messages: only the levels are left.
        let mut e3 = e.clone();
        e3.text = Default::default();
        let got = e3.cue().expect("test value").expect("test value");
        assert_eq!(got.fault, c.fault);
        assert_eq!(
            got.over,
            Some(CueAction {
                osc: None,
                dmx: levels("12=255 13=0"),
            })
        );
        assert_eq!(got.ok, None);
        // Levels typed on a state with no message, and levels emptied.
        let mut e4 = e;
        *e4.cell_mut(CueCol::Dmx(CueState::Near)).expect("text") = "14=40, 15=1".into();
        e4.cell_mut(CueCol::Dmx(CueState::Fault))
            .expect("text")
            .clear();
        let got = e4.cue().expect("test value").expect("test value");
        assert_eq!(got.dmx(CueState::Near), levels("14=40 15=1"));
        assert_eq!(got.fault, None);
        assert_eq!(CueEdit::default().cue(), Ok(None));
    }

    #[test]
    fn a_bad_line_names_its_field() {
        let mut e = CueEdit::default();
        e.cell_mut(CueCol::State(CueState::Near))
            .expect("text")
            .push_str("light 1");
        assert_eq!(
            e.error(),
            Some((
                CueCol::State(CueState::Near),
                "an OSC address starts with '/'".into()
            ))
        );
        assert_eq!(
            e.checked("LAeq 15 min"),
            Err("OSC cue of LAeq 15 min: an OSC address starts with '/'".into())
        );
        let mut e = CueEdit::default();
        e.cell_mut(CueCol::Dmx(CueState::Over))
            .expect("text")
            .push_str("12=300");
        let (col, why) = e.error().expect("a bad level");
        assert_eq!(col.field_name(), "Over DMX");
        assert_eq!(why, "\"12=300\": the value is a number 0–255");
        assert_eq!(
            e.checked("the meter"),
            Err("DMX cue of the meter: \"12=300\": the value is a number 0–255".into())
        );
    }

    /// The append toggle reads as what it adds, in the words of `--append-level`'s level
    /// (dB SPL), and the note under the cues says what gets it.
    #[test]
    fn the_append_toggle_names_db_spl() {
        assert_eq!(CueCol::Append.title(), "+ dB SPL");
        assert_eq!(CueCol::Append.field_name(), "+ dB SPL");
        assert_eq!(ac2_scene::osc::append_text(true), "on");
        assert_eq!(ac2_scene::osc::append_text(false), "off");
        assert_eq!(
            ac2_scene::osc::APPEND_NOTE,
            "+ dB SPL: each OSC message of this row gets the measured level (dB SPL) as a \
             last float argument; fault and DMX don't"
        );
    }
}
