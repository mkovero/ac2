//! The Rig monitor page: where the rig's OSC cues go (typed `ip:port`, empty: off), whether
//! the daemon starts metering on its own at start, a one-off test message, how sending
//! fares, and the meters the rig keeps. Every edit names the rig settings' rev the page
//! showed, so a change another client made first is refused, not overwritten.

use ac2_proto::Command;
use ac2_proto::model::Rig;
use ac2_proto::units::Rev;
use ac2_scene::osc::{DEFAULT_TEST, parse_message, parse_target, target_text};

/// A row of the page, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RigRow {
    Target,
    Monitor,
    Test,
}

impl RigRow {
    pub const ALL: [RigRow; 3] = [RigRow::Target, RigRow::Monitor, RigRow::Test];

    pub fn title(self) -> &'static str {
        match self {
            RigRow::Target => ac2_scene::osc::TARGET_TITLE,
            RigRow::Monitor => ac2_scene::osc::MONITOR_TITLE,
            RigRow::Test => ac2_scene::osc::TEST_TITLE,
        }
    }

    pub fn is_text(self) -> bool {
        matches!(self, RigRow::Target | RigRow::Test)
    }
}

/// The page being edited.
#[derive(Clone, Debug, PartialEq)]
pub struct RigPage {
    pub focus: RigRow,
    /// The target as typed (the rig's while not edited).
    pub target: String,
    /// The test message as typed.
    pub test: String,
    /// The rig settings' rev the page shows: edits name it as their base.
    pub rev: Rev,
    /// The target was typed since the rig's was shown: a mirror change leaves it.
    pub edited: bool,
    /// The focused text is selected: typing replaces it.
    pub selected: bool,
    /// Why the last Enter was refused.
    pub error: Option<String>,
    /// The target sent by the last Enter, with what its call is named: a conflict types it
    /// again.
    sent: Option<(String, String)>,
    /// The last target was refused as made on settings changed since: the typed one stays.
    pub notice: Option<String>,
}

impl Default for RigPage {
    fn default() -> Self {
        Self {
            focus: RigRow::Target,
            target: String::new(),
            test: DEFAULT_TEST.to_owned(),
            rev: Rev(0),
            edited: false,
            selected: true,
            error: None,
            sent: None,
            notice: None,
        }
    }
}

impl RigPage {
    pub fn new(rig: Option<&Rig>) -> Self {
        let mut p = Self::default();
        if let Some(r) = rig {
            p.reload(r);
        }
        p
    }

    /// The rig's settings shown again, an edit dropped (the daemon refused it as made on
    /// settings changed since).
    pub fn reload(&mut self, rig: &Rig) {
        self.target = target_text(rig.osc_target);
        self.rev = rig.rev;
        self.edited = false;
        self.selected = self.focus.is_text();
        self.notice = None;
    }

    /// The mirror changed: the page follows the rig unless the operator is typing a target;
    /// a target kept after a conflict is made on the rig's newest settings. The focus, the
    /// selection and the test message stay: the mirror changes every second while the rig
    /// meters, and selecting the focused text again would make the next key replace what
    /// was typed. Only a new target from the rig selects it again, so typing replaces it.
    pub fn follow(&mut self, rig: &Rig) {
        if !self.edited {
            let target = target_text(rig.osc_target);
            if target != self.target {
                self.target = target;
                self.selected |= self.focus == RigRow::Target;
            }
            self.rev = rig.rev;
            self.notice = None;
        } else if self.notice.is_some() {
            self.rev = rig.rev;
        }
    }

    /// The daemon refused `what` as made on settings changed since: the target it sent is
    /// typed again on the rig's current rev, and a line says Enter again overwrites. False
    /// when `what` is not the page's last target.
    pub fn rebase(&mut self, what: &str, rig: &Rig) -> bool {
        let Some((_, typed)) = self.sent.take_if(|(w, _)| w == what) else {
            return false;
        };
        self.target = typed;
        self.rev = rig.rev;
        self.edited = true;
        self.selected = false;
        self.notice = Some(ac2_scene::osc::CHANGED_ELSEWHERE_ENTER.to_owned());
        true
    }

    pub fn typing(&self) -> bool {
        self.focus.is_text()
    }

    /// ↑/↓: the row above or below; wraps.
    pub fn move_row(&mut self, d: i32) {
        let n = RigRow::ALL.len() as i32;
        let i = RigRow::ALL
            .iter()
            .position(|r| *r == self.focus)
            .unwrap_or(0) as i32;
        self.focus_row(RigRow::ALL[(i + d).rem_euclid(n) as usize]);
    }

    /// Focuses `row` (the mouse, the arrows).
    pub fn focus_row(&mut self, row: RigRow) {
        self.focus = row;
        self.selected = row.is_text();
    }

    fn text_mut(&mut self) -> Option<&mut String> {
        match self.focus {
            RigRow::Target => {
                self.edited = true;
                Some(&mut self.target)
            }
            RigRow::Test => Some(&mut self.test),
            RigRow::Monitor => None,
        }
    }

    pub fn type_text(&mut self, s: &str) {
        let selected = std::mem::take(&mut self.selected);
        if let Some(t) = self.text_mut() {
            if selected {
                t.clear();
            }
            t.push_str(s);
        }
        self.error = None;
    }

    pub fn backspace(&mut self) {
        let selected = std::mem::take(&mut self.selected);
        if let Some(t) = self.text_mut() {
            if selected {
                t.clear();
            } else {
                t.pop();
            }
        }
        self.error = None;
    }

    /// Monitor at start set to `on`: the command, or nothing when it already is.
    pub fn monitor(&mut self, rig: &Rig, on: bool) -> Option<(Command, String)> {
        self.focus_row(RigRow::Monitor);
        self.sent = None;
        (rig.monitor_at_start != on).then(|| {
            let word = if on { "on" } else { "off" };
            (
                Command::RigMonitorStart { base: self.rev, on },
                format!("monitor at start {word}"),
            )
        })
    }

    /// Enter on the focused row: the command to send and what the toast says, or nothing
    /// (a refusal says why in [`Self::error`]).
    pub fn enter(&mut self, rig: &Rig) -> Option<(Command, String)> {
        self.error = None;
        self.notice = None;
        self.sent = None;
        match self.focus {
            RigRow::Target => match parse_target(&self.target) {
                Ok(addr) => {
                    let what = match addr {
                        Some(a) => format!("OSC target {a}"),
                        None => "OSC target off".to_owned(),
                    };
                    let cmd = Command::OscTarget {
                        base: self.rev,
                        addr,
                    };
                    // Sent: the page shows the rig's target again as the mirror brings it.
                    self.edited = false;
                    self.sent = Some((what.clone(), self.target.clone()));
                    Some((cmd, what))
                }
                Err(e) => {
                    self.error = Some(e);
                    None
                }
            },
            RigRow::Monitor => self.monitor(rig, !rig.monitor_at_start),
            RigRow::Test => match parse_message(&self.test, false) {
                Ok(Some(msg)) => {
                    let what = format!("OSC test {} sent", ac2_scene::osc::message_text(&msg));
                    Some((Command::OscTest { msg }, what))
                }
                Ok(None) => {
                    self.error = Some("type a message to send, like /ac2/test 1".into());
                    None
                }
                Err(e) => {
                    self.error = Some(e);
                    None
                }
            },
        }
    }
}
