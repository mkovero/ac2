//! The Rig monitor page: where the rig's OSC cues go (typed `ip:port`, empty: off), whether
//! the daemon starts metering on its own at start, a one-off test message, where the rig's
//! Art-Net universe goes (an address and a universe, applied together), how long its
//! channels fade to a new cue state and a test frame,
//! how sending fares, the meters the rig keeps, and how its programme meters read (the
//! ballistic, the loudness target, the correlator's window, the over threshold, the
//! correlation gate: stepped with
//! ←/→, each step sent at once). Every edit names the rig settings' rev
//! the page showed, so a change another client made first is refused, not overwritten.

use crate::caret::Caret;
use ac2_proto::Command;
use ac2_proto::model::{LevelsSettings, Rig};
use ac2_proto::units::Rev;
use ac2_scene::artnet;
use ac2_scene::osc::{DEFAULT_TEST, parse_message, parse_target, target_text};
use ac2_scene::programme::rig as levels;

/// A row of the page, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RigRow {
    Target,
    Monitor,
    Test,
    ArtnetTarget,
    Universe,
    Fade,
    DmxTest,
    Ballistic,
    LoudnessTarget,
    CorrelationWindow,
    OverThreshold,
    CorrelationGate,
}

impl RigRow {
    pub const ALL: [RigRow; 12] = [
        RigRow::Target,
        RigRow::Monitor,
        RigRow::Test,
        RigRow::ArtnetTarget,
        RigRow::Universe,
        RigRow::Fade,
        RigRow::DmxTest,
        RigRow::Ballistic,
        RigRow::LoudnessTarget,
        RigRow::CorrelationWindow,
        RigRow::OverThreshold,
        RigRow::CorrelationGate,
    ];

    pub fn title(self) -> &'static str {
        match self {
            RigRow::Target => ac2_scene::osc::TARGET_TITLE,
            RigRow::Monitor => ac2_scene::osc::MONITOR_TITLE,
            RigRow::Test => ac2_scene::osc::TEST_TITLE,
            RigRow::ArtnetTarget => artnet::TARGET_TITLE,
            RigRow::Universe => artnet::UNIVERSE_TITLE,
            RigRow::Fade => artnet::FADE_TITLE,
            RigRow::DmxTest => artnet::TEST_TITLE,
            RigRow::Ballistic => levels::BALLISTIC_TITLE,
            RigRow::LoudnessTarget => levels::TARGET_TITLE,
            RigRow::CorrelationWindow => levels::WINDOW_TITLE,
            RigRow::OverThreshold => levels::OVER_TITLE,
            RigRow::CorrelationGate => levels::GATE_TITLE,
        }
    }

    pub fn is_text(self) -> bool {
        !matches!(self, RigRow::Monitor) && !self.is_levels()
    }

    /// A programme-meter row: a value ←/→ steps, each step sent.
    pub fn is_levels(self) -> bool {
        matches!(
            self,
            RigRow::Ballistic
                | RigRow::LoudnessTarget
                | RigRow::CorrelationWindow
                | RigRow::OverThreshold
                | RigRow::CorrelationGate
        )
    }

    /// The note under a programme-meter row.
    pub fn levels_note(self) -> Option<&'static str> {
        match self {
            RigRow::Ballistic => Some(levels::BALLISTIC_NOTE),
            RigRow::LoudnessTarget => Some(levels::TARGET_NOTE),
            RigRow::CorrelationWindow => Some(levels::WINDOW_NOTE),
            RigRow::OverThreshold => Some(levels::OVER_NOTE),
            RigRow::CorrelationGate => Some(levels::GATE_NOTE),
            _ => None,
        }
    }

    /// The Art-Net target's rows: Enter on either applies both.
    fn is_artnet(self) -> bool {
        matches!(self, RigRow::ArtnetTarget | RigRow::Universe)
    }
}

/// A target sent by the last Enter, as typed: a conflict types it again.
#[derive(Clone, Debug, PartialEq)]
enum Typed {
    Osc(String),
    Artnet(String, String),
    Fade(String),
}

/// The page being edited.
#[derive(Clone, Debug, PartialEq)]
pub struct RigPage {
    pub focus: RigRow,
    /// The OSC target as typed (the rig's while not edited).
    pub target: String,
    /// The test message as typed.
    pub test: String,
    /// The Art-Net target's address as typed (the rig's while not edited).
    pub artnet: String,
    /// The Art-Net target's universe as typed (the rig's while not edited).
    pub universe: String,
    /// The Art-Net fade as typed, seconds (the rig's while not edited).
    pub fade: String,
    /// The test frame's levels as typed.
    pub dmx_test: String,
    /// The rig settings' rev the page shows: edits name it as their base.
    pub rev: Rev,
    /// The OSC target was typed since the rig's was shown: a mirror change leaves it.
    pub edited: bool,
    /// The Art-Net target was typed since the rig's was shown: a mirror change leaves it.
    pub artnet_edited: bool,
    /// The fade was typed since the rig's was shown: a mirror change leaves it.
    pub fade_edited: bool,
    /// Where typing goes in the focused text: at its end when the row is entered.
    pub caret: Caret,
    /// Why the last Enter on the focused row was refused (shown under it).
    pub error: Option<String>,
    /// The target sent by the last Enter, with what its call is named.
    sent: Option<(String, Typed)>,
    /// The last target was refused as made on settings changed since: the typed one stays.
    pub notice: Option<String>,
}

impl Default for RigPage {
    fn default() -> Self {
        Self {
            focus: RigRow::Target,
            target: String::new(),
            test: DEFAULT_TEST.to_owned(),
            artnet: String::new(),
            universe: String::new(),
            fade: artnet::fade_text(ac2_proto::model::ArtnetFade::DEFAULT),
            dmx_test: artnet::DEFAULT_TEST.to_owned(),
            rev: Rev(0),
            edited: false,
            artnet_edited: false,
            fade_edited: false,
            caret: Caret::default(),
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
        (self.artnet, self.universe) = artnet::target_texts(rig.artnet_target);
        self.fade = artnet::fade_text(rig.artnet_fade);
        self.rev = rig.rev;
        self.edited = false;
        self.artnet_edited = false;
        self.fade_edited = false;
        self.caret_to_end();
        self.notice = None;
    }

    /// The mirror changed: the page follows the rig except for a target the operator is
    /// typing; a target kept after a conflict is made on the rig's newest settings. The
    /// focus, the caret and the test texts stay: the mirror changes every second while the
    /// rig meters. Only a new target from the rig moves the caret, to its end.
    pub fn follow(&mut self, rig: &Rig) {
        if !self.edited {
            let target = target_text(rig.osc_target);
            if target != self.target {
                self.target = target;
                if self.focus == RigRow::Target {
                    self.caret_to_end();
                }
            }
        }
        if !self.artnet_edited {
            let (a, u) = artnet::target_texts(rig.artnet_target);
            if (&a, &u) != (&self.artnet, &self.universe) {
                (self.artnet, self.universe) = (a, u);
                if self.focus.is_artnet() {
                    self.caret_to_end();
                }
            }
        }
        if !self.fade_edited {
            let f = artnet::fade_text(rig.artnet_fade);
            if f != self.fade {
                self.fade = f;
                if self.focus == RigRow::Fade {
                    self.caret_to_end();
                }
            }
        }
        if !self.edited && !self.artnet_edited && !self.fade_edited {
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
        match typed {
            Typed::Osc(t) => {
                self.target = t;
                self.edited = true;
            }
            Typed::Artnet(a, u) => {
                self.artnet = a;
                self.universe = u;
                self.artnet_edited = true;
            }
            Typed::Fade(f) => {
                self.fade = f;
                self.fade_edited = true;
            }
        }
        self.rev = rig.rev;
        self.caret_to_end();
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

    /// Focuses `row` (the mouse, the arrows); a refusal shown under another row goes.
    pub fn focus_row(&mut self, row: RigRow) {
        if row != self.focus {
            self.error = None;
        }
        self.focus = row;
        self.caret_to_end();
    }

    fn text_mut(&mut self) -> Option<&mut String> {
        match self.focus {
            RigRow::Target => {
                self.edited = true;
                Some(&mut self.target)
            }
            RigRow::Test => Some(&mut self.test),
            RigRow::ArtnetTarget => {
                self.artnet_edited = true;
                Some(&mut self.artnet)
            }
            RigRow::Universe => {
                self.artnet_edited = true;
                Some(&mut self.universe)
            }
            RigRow::Fade => {
                self.fade_edited = true;
                Some(&mut self.fade)
            }
            RigRow::DmxTest => Some(&mut self.dmx_test),
            _ => None,
        }
    }

    /// The focused row's text.
    fn text(&self) -> Option<&str> {
        match self.focus {
            RigRow::Target => Some(&self.target),
            RigRow::Test => Some(&self.test),
            RigRow::ArtnetTarget => Some(&self.artnet),
            RigRow::Universe => Some(&self.universe),
            RigRow::Fade => Some(&self.fade),
            RigRow::DmxTest => Some(&self.dmx_test),
            _ => None,
        }
    }

    fn caret_to_end(&mut self) {
        self.caret = Caret::end_of(self.text().unwrap_or(""));
    }

    /// Applies `f` to the focused text and the caret, kept apart from `self` meanwhile.
    fn edit_text(&mut self, f: impl FnOnce(&mut String, &mut Caret)) {
        let mut c = self.caret;
        if let Some(t) = self.text_mut() {
            f(t, &mut c);
        }
        self.caret = c;
        self.error = None;
    }

    /// Typed text, at the caret.
    pub fn type_text(&mut self, s: &str) {
        self.edit_text(|t, c| c.insert(t, s));
    }

    /// Backspace: the character before the caret.
    pub fn backspace(&mut self) {
        self.edit_text(|t, c| c.backspace(t));
    }

    /// Delete: the character after the caret.
    pub fn delete_char(&mut self) {
        self.edit_text(|t, c| c.delete(t));
    }

    /// ←/→: the caret a character left or right.
    pub fn caret_step(&mut self, d: i32) {
        let mut c = self.caret;
        if let Some(t) = self.text() {
            c.step(t, d);
        }
        self.caret = c;
    }

    /// Home / End.
    pub fn caret_edge(&mut self, end: bool) {
        let mut c = self.caret;
        if let Some(t) = self.text() {
            if end {
                c.end(t);
            } else {
                c.home();
            }
        }
        self.caret = c;
    }

    /// A click on `row`'s text, before its character `n`.
    pub fn caret_at(&mut self, row: RigRow, n: usize) {
        self.focus_row(row);
        if let Some(t) = self.text() {
            self.caret = Caret::at(t, n);
        }
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

    /// ←/→ (`d`) on programme-meter row `row`: the rig's value one step on, as a
    /// `levels.set` of that value alone; nothing at either end of a range (the loudness
    /// target says why in [`Self::error`]).
    pub fn levels_step(&mut self, rig: &Rig, row: RigRow, d: i32) -> Option<(Command, String)> {
        use ac2_proto::model::{
            Ballistic, CORRELATION_GATE_RANGE, CorrelationWindow, OVER_THRESHOLD_RANGE,
        };
        use ac2_proto::units::{Dbfs, Dbtp, Lufs};
        self.focus_row(row);
        self.sent = None;
        self.error = None;
        let now = rig.levels;
        let step_in = |all: usize, at: usize| {
            let i = at as i32 + d.signum();
            (0..all as i32).contains(&i).then_some(i as usize)
        };
        let stepped = |v: f64, by: f64, (lo, hi): (f64, f64)| {
            let n = ((v / by).round() + f64::from(d.signum())) * by;
            (n >= lo && n <= hi && n != v).then_some(n)
        };
        let mut want = now;
        match row {
            RigRow::Ballistic => {
                let at = Ballistic::ALL.iter().position(|b| *b == now.ballistic)?;
                want.ballistic = Ballistic::ALL[step_in(Ballistic::ALL.len(), at)?];
            }
            RigRow::LoudnessTarget => {
                let by = levels::TARGET_STEP_LU;
                let n = ((now.loudness_target.0 / by).round() + f64::from(d.signum())) * by;
                want.loudness_target = Lufs(n);
                // Past either end the row says what the rig takes, in the daemon's words.
                if let Err(e) = want.check() {
                    self.error = Some(e);
                    return None;
                }
            }
            RigRow::CorrelationWindow => {
                let all = CorrelationWindow::ALL;
                let at = all.iter().position(|w| *w == now.correlation_window)?;
                want.correlation_window = all[step_in(all.len(), at)?];
            }
            RigRow::OverThreshold => {
                want.over_threshold = Dbtp(stepped(
                    now.over_threshold.0,
                    levels::OVER_STEP_DB,
                    OVER_THRESHOLD_RANGE,
                )?);
            }
            RigRow::CorrelationGate => {
                want.correlation_gate = Dbfs(stepped(
                    now.correlation_gate.0,
                    levels::GATE_STEP_DB,
                    CORRELATION_GATE_RANGE,
                )?);
            }
            _ => return None,
        }
        levels_set(self.rev, now, want)
    }

    /// The value programme-meter row `row` shows: the rig's.
    pub fn levels_text(rig: &Rig, row: RigRow) -> String {
        let l = rig.levels;
        match row {
            RigRow::Ballistic => l.ballistic.word().to_owned(),
            RigRow::LoudnessTarget => levels::target_text(l.loudness_target.0),
            RigRow::CorrelationWindow => l.correlation_window.text().to_owned(),
            RigRow::OverThreshold => levels::over_text(l.over_threshold.0),
            RigRow::CorrelationGate => levels::gate_text(l.correlation_gate.0),
            _ => String::new(),
        }
    }

    /// Enter on the focused row: the command to send and what the toast says, or nothing
    /// (a refusal says why in [`Self::error`]).
    pub fn enter(&mut self, rig: &Rig) -> Option<(Command, String)> {
        self.error = None;
        self.notice = None;
        self.sent = None;
        let r = match self.focus {
            RigRow::Target => parse_target(&self.target).map(|addr| {
                let what = match addr {
                    Some(a) => format!("OSC target {a}"),
                    None => "OSC target off".to_owned(),
                };
                // Sent: the page shows the rig's target again as the mirror brings it.
                self.edited = false;
                self.sent = Some((what.clone(), Typed::Osc(self.target.clone())));
                (
                    Command::OscTarget {
                        base: self.rev,
                        addr,
                    },
                    what,
                )
            }),
            RigRow::ArtnetTarget | RigRow::Universe => {
                artnet::parse_target(&self.artnet, &self.universe).map(|target| {
                    let what = match target {
                        Some(t) => format!(
                            "Art-Net target {} universe {}",
                            t.addr,
                            artnet::universe_text(t.universe)
                        ),
                        None => "Art-Net target off".to_owned(),
                    };
                    self.artnet_edited = false;
                    self.sent = Some((
                        what.clone(),
                        Typed::Artnet(self.artnet.clone(), self.universe.clone()),
                    ));
                    (
                        Command::ArtnetTarget {
                            base: self.rev,
                            target,
                        },
                        what,
                    )
                })
            }
            RigRow::Fade => artnet::parse_fade(&self.fade).map(|fade| {
                let what = artnet::fade_toast(fade);
                self.fade_edited = false;
                self.sent = Some((what.clone(), Typed::Fade(self.fade.clone())));
                (
                    Command::ArtnetFade {
                        base: self.rev,
                        seconds: ac2_proto::units::Seconds(fade.seconds()),
                    },
                    what,
                )
            }),
            RigRow::Monitor => return self.monitor(rig, !rig.monitor_at_start),
            row @ (RigRow::Ballistic
            | RigRow::LoudnessTarget
            | RigRow::CorrelationWindow
            | RigRow::OverThreshold
            | RigRow::CorrelationGate) => return self.levels_step(rig, row, 1),
            RigRow::Test => match parse_message(&self.test, false) {
                Ok(Some(msg)) => {
                    let what = format!("OSC test {} sent", ac2_scene::osc::message_text(&msg));
                    Ok((Command::OscTest { msg }, what))
                }
                Ok(None) => Err("type a message to send, like /ac2/test 1".into()),
                Err(e) => Err(e),
            },
            RigRow::DmxTest => match artnet::parse_test(&self.dmx_test) {
                Ok(_) if rig.artnet_target.is_none() => {
                    Err("no Art-Net target: set one above first".into())
                }
                Ok(channels) => {
                    let what = format!("DMX test {} sent", artnet::cue_text(&channels));
                    Ok((Command::ArtnetTest { channels }, what))
                }
                Err(e) => Err(e),
            },
        };
        r.map_err(|e| self.error = Some(e)).ok()
    }
}

/// `levels.set` of the programme-meter settings in `want` that differ from the rig's `now`,
/// on the rev `base` the change was made against, with what the toast says; `None` when
/// nothing differs. A value left alone is not sent, so it never overwrites another client's.
pub fn levels_set(
    base: Rev,
    now: LevelsSettings,
    want: LevelsSettings,
) -> Option<(Command, String)> {
    let ballistic = (want.ballistic != now.ballistic).then_some(want.ballistic);
    let correlation_window =
        (want.correlation_window != now.correlation_window).then_some(want.correlation_window);
    let over_threshold = (want.over_threshold != now.over_threshold).then_some(want.over_threshold);
    let correlation_gate =
        (want.correlation_gate != now.correlation_gate).then_some(want.correlation_gate);
    let loudness_target =
        (want.loudness_target != now.loudness_target).then_some(want.loudness_target);
    let what: Vec<String> = [
        ballistic.map(|b| format!("programme meters read {}", b.word())),
        loudness_target.map(|t| format!("loudness target {}", levels::target_text(t.0))),
        correlation_window.map(|w| format!("correlation window {}", w.text())),
        over_threshold.map(|t| format!("an over is above {}", levels::over_text(t.0))),
        correlation_gate.map(|g| format!("correlation gate {}", levels::gate_text(g.0))),
    ]
    .into_iter()
    .flatten()
    .collect();
    if what.is_empty() {
        return None;
    }
    let cmd = Command::LevelsSet {
        base,
        ballistic,
        correlation_window,
        over_threshold,
        correlation_gate,
        loudness_target,
    };
    Some((cmd, what.join(" · ")))
}
