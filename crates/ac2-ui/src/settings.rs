//! The Settings view (Ctrl+P, the palette's "Settings…", the gear in the top bar): every
//! setting in one full-window view, a page per area, instead of one dialog per setting
//! found — or not — under Ctrl+K.
//!
//! Keyboard-first like every window (decision K9): the view owns the keyboard while open;
//! ↑/↓ move within the page, ←/→ change a choice, Enter applies, Esc closes the topmost
//! window (a calibration dialog or a confirmation over a page, else the view);
//! Ctrl+PageUp / Ctrl+PageDown (or Alt+← / Alt+→) step through the pages and Alt+1 … Alt+7
//! jump to one (no Ctrl+Tab: desktops take it). Shift+Esc still stops the stimulus from anywhere. The keys
//! that used to open a dialog open its page: Shift+O the Audio I/O page (focused on the device), Shift+L the SPL / Leq
//! page, the palette's input setup and calibrations their pages.
//!
//! Every setting says whose it is: [`THIS_APP`] (kept in `ui.toml` on this computer) or
//! [`THE_RIG`] (kept by the daemon; every client sees and changes it).
//!
//! Pure data like the dialogs it hosts ([`SessionDialog`], [`CalView`], [`LeqDialog`]): the
//! lines and texts come from the mirrored state on every call, and the reducer turns the
//! actions into requests.

use ac2_proto::model::{Generator, ServerInfo};
use ac2_proto::units::Dbfs;
use ac2_scene::rig::{RAISE_WHILE_LIVE, RAISE_WORD};
use ac2_scene::theme::ThemeName;
use ac2_scene::view::{
    CoherencePlacement, DistortionUnit, IrMode, LeqLayout, LeqStyle, SpectrumMode, SweepMode,
};

use crate::cal_view::CalView;
use crate::leq_dialog::LeqDialog;
use crate::session_dialog::SessionDialog;

mod connection;
mod rig;
pub use connection::{
    ClientKey, ConnAction, ConnEdit, ConnLine, ConnectionPage, SERVER_INFO_EVERY_S, conn_line_text,
    conn_lines, link_lines,
};
pub use rig::{RigPage, RigRow};

/// Whose a setting is: this computer's app alone.
pub const THIS_APP: &str = "this app";
/// Whose a setting is: the rig, for every client.
pub const THE_RIG: &str = "the rig — all clients";
/// The page with the audio device, the channels and the max level, as every text names it.
pub const AUDIO_IO: &str = "Audio I/O";
/// The Audio I/O page's heading when the Sessions dialog's New opened it.
pub const NEW_SESSION_AUDIO: &str = "New session — Audio I/O";

/// A page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// The audio device (backend, input and output device, rate, buffer), then the
    /// channels (inputs, outputs, stimulus outputs, the reference), then the system max level.
    AudioIo,
    /// Mics, curves, sensitivity calibrations.
    Calibration,
    /// The SPL meter's Leq windows and limits.
    Leq,
    /// The record toggle's limit, where recordings go.
    Recording,
    /// Theme, key hints, how each pane kind draws, the level axes.
    Display,
    /// The daemon link, this client's key, the server's mode and keys.
    Connection,
    /// Where the rig's OSC cues and Art-Net universe go, monitoring from daemon start, the
    /// rig's meters.
    Rig,
}

impl Page {
    pub const ALL: [Page; 7] = [
        Page::AudioIo,
        Page::Calibration,
        Page::Leq,
        Page::Recording,
        Page::Display,
        Page::Connection,
        Page::Rig,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Page::AudioIo => AUDIO_IO,
            Page::Calibration => "Calibration",
            Page::Leq => "SPL / Leq",
            Page::Recording => "Recording",
            Page::Display => "Display",
            Page::Connection => "Connection",
            Page::Rig => ac2_scene::osc::PAGE,
        }
    }

    /// Whose the page's settings are, as its header says.
    pub fn whose(self) -> &'static str {
        match self {
            Page::AudioIo => "the rig — all clients · stimulus outputs: this app",
            Page::Calibration | Page::Leq | Page::Rig => THE_RIG,
            Page::Recording => "the limit: this app · the folder: the rig — all clients",
            Page::Display => THIS_APP,
            Page::Connection => {
                "the link and this client's key: this app · server and client keys: the rig — \
                 all clients"
            }
        }
    }

    /// The page's keys, under it, chords in the current label style.
    pub fn keys(self) -> String {
        self.keys_in(crate::keys::label_style())
    }

    /// The page's keys with chords written in `style`.
    pub fn keys_in(self, style: crate::keys::LabelStyle) -> String {
        let text = match self {
            Page::AudioIo => {
                "↑↓ move · ←→ backend / input / output device, a mic's curve · type the rate or \
                 buffer · Space in session · R reference · M mic · S stimulus output · N names a \
                 mic or an output · D detects the loopback · P opens the driver's settings (ASIO \
                 panel, Audio MIDI Setup) · on the max level: type dBFS, Enter \
                 · Enter opens the session · Esc closes"
            }
            Page::Calibration => {
                "↑↓ move (PgUp/PgDn, Home/End) · ←→ mic curve of an input · N names the mic · I \
                 imports a curve file · R renames a curve · C calibrates with a calibrator · E \
                 calibrates electrically · Delete deletes (twice) · Esc closes"
            }
            Page::Leq => {
                let range = crate::keys::Chord::shift(eframe::egui::Key::Insert).label_in(style);
                return format!(
                    "Enter applies (the log and the windows carry on) · ↑↓ row · Tab cell · ←→ \
                     choose · in a cue line ←→ Home End move the caret, Delete deletes at it · \
                     Insert or + adds a window to the section · {range} or + range… a band \
                     window per band from … to · Delete or − removes the focused one · T on a \
                     band row: band transfer · Esc closes"
                );
            }
            Page::Recording => "↑↓ move · type minutes, Enter applies · Esc closes",
            Page::Display => "↑↓ move · ←→ change · Enter resets the level axes · Esc closes",
            Page::Connection => {
                "↑↓ move · Enter acts on the line · A authorizes a refused key (type its name) · \
                 Delete revokes a client (twice) · Esc closes"
            }
            Page::Rig => {
                "↑↓ Tab move · type a target or a test, ←→ Home End move the caret, Delete \
                 deletes at it · Enter applies the target, sends the test · ←→ or Enter on \
                 Monitor at start · Esc closes"
            }
        };
        text.to_owned()
    }

    pub fn index(self) -> usize {
        Page::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }

    /// The page `d` steps on, wrapping.
    pub fn step(self, d: i32) -> Page {
        let n = Page::ALL.len() as i32;
        Page::ALL[(self.index() as i32 + d).rem_euclid(n) as usize]
    }
}

/// A raise of the system max level waiting for its typed confirmation.
#[derive(Clone, Debug, PartialEq)]
pub struct RaiseConfirm {
    pub from: Dbfs,
    pub to: Dbfs,
    /// What the operator typed so far ([`RAISE_WORD`] confirms).
    pub typed: String,
}

/// The system max level row of the Audio I/O page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CeilingEdit {
    /// The level being typed; empty shows the level in force.
    pub text: String,
    pub confirm: Option<RaiseConfirm>,
    pub error: Option<String>,
}

impl CeilingEdit {
    /// Enter on the row: the change to send (the level, and whether it is a confirmed
    /// raise), or nothing yet (a raise asks for its confirmation first; a refusal says why).
    /// `live`: a stimulus is armed or playing, which no raise may happen under.
    pub fn enter(&mut self, g: Option<&Generator>, live: bool) -> Option<(Dbfs, bool)> {
        self.error = None;
        let Some(g) = g else {
            self.error = Some("not connected to a daemon".into());
            return None;
        };
        if let Some(c) = &self.confirm {
            if !c.typed.trim().eq_ignore_ascii_case(RAISE_WORD) {
                self.error = Some(format!(
                    "type {RAISE_WORD} to confirm, or Esc to keep {}",
                    ac2_scene::format::dbfs(c.from.0)
                ));
                return None;
            }
            if live {
                self.error = Some(RAISE_WHILE_LIVE.into());
                return None;
            }
            let to = c.to;
            self.confirm = None;
            self.text.clear();
            return Some((to, true));
        }
        let text = self.text.trim();
        if text.is_empty() {
            self.error = Some("type the new max level in dBFS (e.g. -40), then Enter".into());
            return None;
        }
        let v =
            match ac2_scene::unit_text::in_field::<ac2_scene::unit_text::LevelDbfs>(text, "dbfs") {
                Ok(v) => v.0.0,
                Err(e) => {
                    self.error = Some(e.0);
                    return None;
                }
            };
        if v > g.ceiling_bound.0 {
            self.error = Some(format!(
                "above this rig's bound {} (ac2d --max-level; only a restart with a higher bound \
                 raises it)",
                ac2_scene::format::dbfs(g.ceiling_bound.0)
            ));
            return None;
        }
        if v > g.ceiling.0 {
            if live {
                self.error = Some(RAISE_WHILE_LIVE.into());
                return None;
            }
            self.confirm = Some(RaiseConfirm {
                from: g.ceiling,
                to: Dbfs(v),
                typed: String::new(),
            });
            return None;
        }
        self.text.clear();
        Some((Dbfs(v), false))
    }

    pub fn type_text(&mut self, s: &str) {
        self.error = None;
        match &mut self.confirm {
            Some(c) => c.typed.push_str(s),
            None => self.text.push_str(s),
        }
    }

    pub fn backspace(&mut self) {
        match &mut self.confirm {
            Some(c) => {
                c.typed.pop();
            }
            None => {
                self.text.pop();
            }
        }
    }
}

/// A line of the Display page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayRow {
    Theme,
    KeyHints,
    WarningToasts,
    SplHold,
    LeqStyle,
    LeqHistory,
    LeqBars,
    SpectrumView,
    SpectrumGlide,
    SweepView,
    DistortionUnit,
    Coherence,
    ResolutionMarker,
    CursorJump,
    LevelAxes,
}

impl DisplayRow {
    pub const ALL: [DisplayRow; 15] = [
        DisplayRow::Theme,
        DisplayRow::KeyHints,
        DisplayRow::WarningToasts,
        DisplayRow::SplHold,
        DisplayRow::LeqStyle,
        DisplayRow::LeqHistory,
        DisplayRow::LeqBars,
        DisplayRow::SpectrumView,
        DisplayRow::SpectrumGlide,
        DisplayRow::SweepView,
        DisplayRow::DistortionUnit,
        DisplayRow::Coherence,
        DisplayRow::ResolutionMarker,
        DisplayRow::CursorJump,
        DisplayRow::LevelAxes,
    ];

    pub fn title(self) -> &'static str {
        match self {
            DisplayRow::Theme => "Theme",
            DisplayRow::KeyHints => "Key hints",
            DisplayRow::WarningToasts => "Warning toasts",
            DisplayRow::SplHold => "SPL number holds",
            DisplayRow::LeqStyle => "Leq windows",
            DisplayRow::LeqHistory => "Leq history strip",
            DisplayRow::LeqBars => "Leq bars",
            DisplayRow::SpectrumView => "Spectrum pane shows",
            DisplayRow::SpectrumGlide => "Spectrum",
            DisplayRow::SweepView => "Sweep pane shows",
            DisplayRow::DistortionUnit => "Distortion in",
            DisplayRow::Coherence => "Coherence",
            DisplayRow::ResolutionMarker => "Resolution marker",
            DisplayRow::CursorJump => "Peak / notch jumps",
            DisplayRow::LevelAxes => "Level axes",
        }
    }
}

/// The SPL number's hold choices, ms (`None`: by the meter's time weighting).
pub const SPL_HOLDS_MS: [Option<u32>; 5] = [None, Some(250), Some(500), Some(1000), Some(2000)];

/// The next theme ←/→ choose.
pub fn step_theme(t: ThemeName, d: i32) -> ThemeName {
    const ALL: [ThemeName; 3] = [ThemeName::Dark, ThemeName::Light, ThemeName::HighContrast];
    let i = ALL.iter().position(|x| *x == t).unwrap_or(0) as i32;
    ALL[(i + d).rem_euclid(3) as usize]
}

pub fn theme_name(t: ThemeName) -> &'static str {
    match t {
        ThemeName::Dark => "dark",
        ThemeName::Light => "light",
        ThemeName::HighContrast => "high contrast (sunlight)",
    }
}

/// The SPL hold ←/→ choose.
pub fn step_hold(h: Option<u32>, d: i32) -> Option<u32> {
    let i = SPL_HOLDS_MS.iter().position(|x| *x == h).unwrap_or(0) as i32;
    SPL_HOLDS_MS[(i + d).clamp(0, SPL_HOLDS_MS.len() as i32 - 1) as usize]
}

/// What the spectrum pane's view is called (G steps it).
pub fn spectrum_view_name(m: SpectrumMode) -> &'static str {
    match m {
        SpectrumMode::Spectrum => "the spectrum",
        SpectrumMode::Split => "the spectrum over its spectrograph",
        SpectrumMode::Spectrograph => "the spectrograph",
    }
}

/// What the sweep pane's view is called (G steps it).
pub fn sweep_view_name(m: SweepMode) -> &'static str {
    match m {
        SweepMode::Response => "response and distortion",
        SweepMode::Ir(IrMode::Linear) => "the impulse response",
        SweepMode::Ir(IrMode::Log) => "the impulse response in dB",
        SweepMode::Ir(IrMode::Etc) => "the energy-time curve",
        SweepMode::Room => "the room parameters",
    }
}

/// The views of the panes the Display page shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneViews {
    pub spectrum: SpectrumMode,
    pub sweep: SweepMode,
    pub leq: LeqLayout,
    pub distortion_unit: DistortionUnit,
    pub coherence: CoherencePlacement,
}

/// What the Warning toasts setting says, on the Display page and in its toast.
pub fn warning_toasts_text(on: bool) -> &'static str {
    if on {
        "on: warnings and Leq limit alarms pop up in the corner"
    } else {
        "off: warnings and Leq limit alarms go only to the notification log"
    }
}

/// What the Resolution marker setting says, on the Display page and in its toast.
pub fn resolution_marker_text(on: bool) -> &'static str {
    if on {
        "on: shades where a curve is coarser than its resolution"
    } else {
        "off"
    }
}

/// What the Leq bars setting says, on the Display page and in its toast.
pub fn bar_ease_text(on: bool) -> &'static str {
    if on {
        "glide to each new reading"
    } else {
        "jump to each new reading"
    }
}

/// What the spectrum glide setting says, on the Display page and in its toast.
pub fn spec_ease_text(on: bool) -> &'static str {
    if on {
        "glides to each new frame"
    } else {
        "jumps to each new frame"
    }
}

/// The Display page's on / off lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplaySwitches {
    pub key_hints: bool,
    pub warning_toasts: bool,
    pub resolution_marker: bool,
    pub bar_ease: bool,
    pub spec_ease: bool,
}

/// What the Display page shows for each line: title, value, whose.
pub fn display_rows(
    theme: ThemeName,
    switches: DisplaySwitches,
    spl_hold_ms: Option<u32>,
    views: PaneViews,
    jump_prominence_db: f64,
) -> Vec<(DisplayRow, String)> {
    DisplayRow::ALL
        .iter()
        .map(|r| {
            let v = match r {
                DisplayRow::Theme => theme_name(theme).to_owned(),
                DisplayRow::KeyHints => {
                    if switches.key_hints {
                        "shown under the focused pane".into()
                    } else {
                        "hidden".into()
                    }
                }
                DisplayRow::WarningToasts => warning_toasts_text(switches.warning_toasts).into(),
                DisplayRow::SplHold => match spl_hold_ms {
                    None => "by the time weighting".into(),
                    Some(ms) => {
                        format!("{} s", ac2_scene::format::fixed(f64::from(ms) / 1e3, 2))
                    }
                },
                DisplayRow::LeqStyle => match views.leq.style {
                    LeqStyle::Columns => "columns".into(),
                    LeqStyle::Tiles => "tiles".into(),
                },
                DisplayRow::LeqHistory => {
                    if views.leq.history {
                        "shown under the windows".into()
                    } else {
                        "hidden".into()
                    }
                }
                DisplayRow::LeqBars => bar_ease_text(switches.bar_ease).into(),
                DisplayRow::SpectrumGlide => spec_ease_text(switches.spec_ease).into(),
                DisplayRow::DistortionUnit => match views.distortion_unit {
                    DistortionUnit::Db => "dB re fundamental".into(),
                    DistortionUnit::Percent => "% of fundamental".into(),
                },
                DisplayRow::Coherence => match views.coherence {
                    CoherencePlacement::Pane => "its own view (G)".into(),
                    CoherencePlacement::OverlayOnMagnitude => "over the magnitude".into(),
                },
                DisplayRow::ResolutionMarker => {
                    resolution_marker_text(switches.resolution_marker).into()
                }
                DisplayRow::CursorJump => {
                    ac2_scene::cursor_jump::prominence_text(jump_prominence_db)
                }
                DisplayRow::SpectrumView => spectrum_view_name(views.spectrum).to_owned(),
                DisplayRow::SweepView => sweep_view_name(views.sweep).to_owned(),
                DisplayRow::LevelAxes => {
                    "each pane's as last left · Enter resets them to the defaults".into()
                }
            };
            (*r, v)
        })
        .collect()
}

/// A line of the Recording page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingRow {
    Limit,
    Folder,
}

/// The Recording page.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordingPage {
    pub focus: RecordingRow,
    /// Minutes being typed; empty shows the limit in force.
    pub text: String,
    pub error: Option<String>,
}

impl Default for RecordingPage {
    fn default() -> Self {
        Self {
            focus: RecordingRow::Limit,
            text: String::new(),
            error: None,
        }
    }
}

impl RecordingPage {
    /// Enter on the limit: the new limit, minutes, or why not.
    pub fn enter(&mut self) -> Option<u32> {
        self.error = None;
        if self.focus != RecordingRow::Limit {
            return None;
        }
        let t = self.text.trim().to_ascii_lowercase();
        let t = t.strip_suffix("min").unwrap_or(&t).trim().to_owned();
        match t.parse::<u32>() {
            Ok(m) if crate::prefs::RECORD_LIMIT_MIN.contains(&m) => {
                self.text.clear();
                Some(m)
            }
            _ => {
                self.error = Some(format!(
                    "type whole minutes, {} … {}",
                    crate::prefs::RECORD_LIMIT_MIN.start(),
                    crate::prefs::RECORD_LIMIT_MIN.end()
                ));
                None
            }
        }
    }
}

/// The Recording page's lines: title, value, whose.
pub fn recording_rows(limit_min: u32, server: Option<&ServerInfo>) -> Vec<(RecordingRow, String)> {
    let folder = match server {
        None => "asking the daemon…".to_owned(),
        Some(s) => match &s.recording_dir {
            Some(d) => format!("{d} (ac2d --recordings)"),
            None => "this daemon does not record".to_owned(),
        },
    };
    vec![
        (
            RecordingRow::Limit,
            format!("the record toggle stops by itself after {limit_min} min"),
        ),
        (RecordingRow::Folder, folder),
    ]
}

/// The Settings view's own state.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub page: Page,
    /// The Audio I/O page's device and channel rows.
    pub session: SessionDialog,
    /// The max level row is focused on the Audio I/O page (below the channels).
    pub on_ceiling: bool,
    pub ceiling: CeilingEdit,
    pub cal: CalView,
    /// The Leq windows of the SPL pane's meter; `None`: there is no SPL meter.
    pub leq: Option<LeqDialog>,
    pub display: DisplayRow,
    pub recording: RecordingPage,
    pub connection: ConnectionPage,
    pub rig: RigPage,
    /// Opened for channel rows before the device's rows arrived: the focus goes to them
    /// once they do.
    pub focus_want: Option<FocusWant>,
    /// Opened by the Sessions dialog's New: the Audio I/O page says it starts a new session.
    pub new_session: bool,
}

/// Channel rows a command opened the Audio I/O page for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusWant {
    /// The first input.
    Inputs,
    /// The first ticked stimulus output, else the first output.
    StimulusOutputs,
}

impl Settings {
    pub fn new(page: Page, session: SessionDialog, cal: CalView, leq: Option<LeqDialog>) -> Self {
        let mut s = Self {
            page,
            session,
            on_ceiling: false,
            ceiling: CeilingEdit::default(),
            cal,
            leq,
            display: DisplayRow::Theme,
            recording: RecordingPage::default(),
            connection: ConnectionPage::default(),
            rig: RigPage::default(),
            focus_want: None,
            new_session: false,
        };
        s.show(page);
        s
    }

    /// Focuses the channel rows `want` names; until the device's rows arrive the wish waits
    /// in [`Self::focus_want`] (and [`Self::follow_focus_want`] fulfils it).
    pub fn focus_channels(&mut self, want: FocusWant) {
        self.focus_want = Some(want);
        self.follow_focus_want();
    }

    /// The device's rows arrived (or changed): a waiting focus wish lands on them.
    pub fn follow_focus_want(&mut self) {
        use crate::session_dialog::Row;
        let Some(want) = self.focus_want else {
            return;
        };
        let d = &self.session;
        let row = match want {
            FocusWant::Inputs => (!d.inputs.is_empty()).then_some(Row::Input(0)),
            FocusWant::StimulusOutputs => d
                .outputs
                .iter()
                .position(|o| o.stimulus)
                .or((!d.outputs.is_empty()).then_some(0))
                .map(Row::Output),
        };
        if let Some(row) = row {
            self.focus_want = None;
            self.focus_session_row(row);
        }
    }

    /// Shows the Audio I/O page focused on `row`.
    pub fn focus_session_row(&mut self, row: crate::session_dialog::Row) {
        self.show(Page::AudioIo);
        self.on_ceiling = false;
        self.session.detect_cancel();
        self.session.focus_row(row);
    }

    /// Shows `page`.
    pub fn show(&mut self, page: Page) {
        self.page = page;
    }

    /// The shown page's heading: its title, or on Audio I/O after the Sessions dialog's
    /// New, that it opens a new session.
    pub fn heading(&self) -> &'static str {
        if self.new_session && self.page == Page::AudioIo {
            NEW_SESSION_AUDIO
        } else {
            self.page.title()
        }
    }

    /// Whether a typed edit or text row owns the keys that otherwise act on the page.
    pub fn typing(&self) -> bool {
        match self.page {
            Page::AudioIo if self.on_ceiling => true,
            Page::AudioIo => self.session.text_focus(),
            Page::Calibration => self.cal.typing(),
            Page::Leq => self.leq.is_some(),
            Page::Recording => self.recording.focus == RecordingRow::Limit,
            Page::Display => false,
            Page::Connection => self.connection.typing(),
            Page::Rig => self.rig.typing(),
        }
    }

    /// An inner window is open over the page (Esc closes it, not the view).
    pub fn inner_open(&self) -> bool {
        match self.page {
            Page::Calibration => self.cal.electrical.is_some() || self.cal.acoustic.is_some(),
            Page::AudioIo => self.ceiling.confirm.is_some(),
            Page::Leq => self.leq.as_ref().is_some_and(|d| d.transfer.is_some()),
            _ => false,
        }
    }

    /// Esc on an inner window: closes it. `false` when there is none.
    pub fn close_inner(&mut self) -> bool {
        match self.page {
            Page::Calibration => self.cal.close_dialog(),
            Page::AudioIo if self.ceiling.confirm.is_some() => {
                self.ceiling = CeilingEdit::default();
                true
            }
            Page::Leq => self
                .leq
                .as_mut()
                .is_some_and(|d| d.transfer.take().is_some()),
            _ => false,
        }
    }

    /// ↑/↓ on the Audio I/O page: the device rows, the channel rows, then the max level
    /// row, wrapping.
    pub fn io_move(&mut self, d: i32) {
        let rows = self.session.rows();
        if self.on_ceiling {
            self.on_ceiling = false;
            self.ceiling.confirm = None;
            self.ceiling.error = None;
            match (d > 0, rows.first(), rows.last()) {
                (true, Some(first), _) => self.session.focus = *first,
                (false, _, Some(last)) => self.session.focus = *last,
                _ => self.on_ceiling = true,
            }
            return;
        }
        let i = rows.iter().position(|r| *r == self.session.focus);
        let leaving = match i {
            Some(i) => (d > 0 && i + 1 == rows.len()) || (d < 0 && i == 0),
            None => true,
        };
        if leaving {
            self.session.finish_edit();
            self.session.detect_cancel();
            self.on_ceiling = true;
        } else {
            self.session.detect_cancel();
            self.session.move_focus(d);
        }
    }

    /// ↑/↓ on the Display page.
    pub fn display_move(&mut self, d: i32) {
        let n = DisplayRow::ALL.len() as i32;
        let i = DisplayRow::ALL
            .iter()
            .position(|r| *r == self.display)
            .unwrap_or(0) as i32;
        self.display = DisplayRow::ALL[(i + d).rem_euclid(n) as usize];
    }

    /// ↑/↓ on the Recording page.
    pub fn recording_move(&mut self) {
        self.recording.focus = match self.recording.focus {
            RecordingRow::Limit => RecordingRow::Folder,
            RecordingRow::Folder => RecordingRow::Limit,
        };
        self.recording.text.clear();
        self.recording.error = None;
    }
}

#[cfg(test)]
mod tests;
