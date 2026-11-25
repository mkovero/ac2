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
//! Pure data like the dialogs it hosts ([`StreamDialog`], [`CalView`], [`LeqDialog`]): the
//! lines and texts come from the mirrored state on every call, and the reducer turns the
//! actions into requests.

use ac2_proto::model::{Generator, Recordings, ServerInfo};
use ac2_proto::units::Dbfs;
use ac2_scene::rig::{RAISE_WHILE_LIVE, RAISE_WORD};
use ac2_scene::stimulus::SignalRow;

use crate::cal_view::CalView;
use crate::leq_dialog::LeqDialog;
use crate::stream_dialog::StreamDialog;

mod connection;
mod display;
mod network;
mod pairing;
mod rig;
mod signal;
pub use connection::{
    ClientKey, ConnAction, ConnEdit, ConnLine, ConnectionPage, SERVER_INFO_EVERY_S, conn_line_text,
    conn_lines, link_lines, start_local_text, stop_on_quit_text,
};
pub use display::{
    DisplayRow, DisplaySwitches, PaneViews, QUALITY_3D_UNAVAILABLE, SPL_HOLDS_MS, bar_ease_text,
    display_rows, ember_decay_steps, ember_decay_text, ember_glow_steps, ember_glow_text,
    invert_drag_3d_text, level_follow_text, plate_background_text, quality_3d_text,
    range_band_text, resolution_marker_text, shared_scale_text, sim_hints_text, spec_ease_text,
    spectrum_view_name, step_ember_decay, step_ember_glow, step_hold, step_theme, sweep_view_name,
    theme_name, warning_toasts_text,
};
pub use network::{IfacePick, NetPicks, Switch, read_only_row, switch};
pub use pairing::{PairOffer, PairedRow, Pairing};
pub use rig::{RigPage, RigRow, levels_set};
pub use signal::SignalRows;

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
            Page::AudioIo => "the rig — all clients · stimulus outputs and signal: this app",
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
                let pair = crate::keys::Chord::shift(eframe::egui::Key::P).label_in(style);
                return format!(
                    "↑↓ move · ←→ backend / input / output device, a mic's curve · type the rate \
                     or buffer · Space in stream · R reference · M mic · L line · A digital · \
                     {pair} pair · N name (the mic, the pair or an output) · S stimulus output · \
                     D detects the loopback · P opens the driver's settings (ASIO panel, Audio \
                     MIDI Setup) · Delete on a remote input removes it (twice) · on the \
                     stimulus signal: ←→ change, type the sine's frequency, Enter · on the max \
                     level: type dBFS, Enter · Enter opens the stream (on Add an input from \
                     another rig…: picks one) · Esc closes"
                );
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
                "↑↓ move · Enter acts on the line · ←→ or Enter on the ac2d rows · A authorizes a \
                 refused key (type its name and viewer or operator) · ←→ on a client changes its \
                 role (twice) · Delete revokes a client (twice) · Enter on Pair \
                 with rig… lists the rigs held, Enter on one pairs both ways · Delete unpairs a \
                 paired rig (twice) · Esc closes"
            }
            Page::Rig => {
                "↑↓ Tab move · type a target or a test, ←→ Home End move the caret, Delete \
                 deletes at it · Enter applies the target, sends the test · ←→ or Enter on \
                 Monitor at start and the programme meters' rows · Esc closes"
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
        Some(s) => match &s.recordings {
            Recordings::Dir { dir } => format!("{dir} (ac2d --recordings)"),
            Recordings::Kept => "on the rig's host".to_owned(),
            Recordings::Off => "this daemon does not record".to_owned(),
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
    pub stream: StreamDialog,
    /// The max level row is focused on the Audio I/O page (below the channels).
    pub on_ceiling: bool,
    pub ceiling: CeilingEdit,
    /// The stimulus signal rows on the Audio I/O page (between the channels and the max
    /// level), and which of them is focused.
    pub signal: SignalRows,
    /// The remote inputs' add dialog and removal on the Audio I/O page.
    pub remote: crate::remote_inputs::RemoteRows,
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
    pub fn new(page: Page, stream: StreamDialog, cal: CalView, leq: Option<LeqDialog>) -> Self {
        let mut s = Self {
            page,
            stream,
            on_ceiling: false,
            ceiling: CeilingEdit::default(),
            signal: SignalRows::default(),
            remote: Default::default(),
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
        use crate::stream_dialog::Row;
        let Some(want) = self.focus_want else {
            return;
        };
        let d = &self.stream;
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
            self.focus_stream_row(row);
        }
    }

    /// Shows the Audio I/O page focused on `row`.
    pub fn focus_stream_row(&mut self, row: crate::stream_dialog::Row) {
        self.show(Page::AudioIo);
        self.on_ceiling = false;
        self.signal.leave();
        self.stream.detect_cancel();
        self.stream.focus_row(row);
    }

    /// The max level row takes the focus.
    pub fn focus_ceiling(&mut self) {
        self.stream.finish_edit();
        self.signal.leave();
        self.on_ceiling = true;
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
            Page::AudioIo if self.signal.focus.is_some() => {
                self.signal.focus == Some(SignalRow::Frequency)
            }
            Page::AudioIo => self.stream.text_focus(),
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
            Page::AudioIo => self.ceiling.confirm.is_some() || self.remote.pick.is_some(),
            Page::Leq => self.leq.as_ref().is_some_and(|d| d.transfer.is_some()),
            Page::Connection => self.connection.pairing.offers.is_some(),
            _ => false,
        }
    }

    /// Esc on an inner window: closes it. `false` when there is none.
    pub fn close_inner(&mut self) -> bool {
        match self.page {
            Page::Calibration => self.cal.close_dialog(),
            Page::AudioIo if self.remote.pick.is_some() => {
                self.remote.pick = None;
                true
            }
            Page::AudioIo if self.ceiling.confirm.is_some() => {
                self.ceiling = CeilingEdit::default();
                true
            }
            Page::Leq => self
                .leq
                .as_mut()
                .is_some_and(|d| d.transfer.take().is_some()),
            Page::Connection => self.connection.pairing.offers.take().is_some(),
            _ => false,
        }
    }

    /// ↑/↓ on the Audio I/O page: the device rows, the channel rows, the stimulus signal
    /// rows (`sine`: the sine's own are shown), then the max level row, wrapping.
    pub fn io_move(&mut self, d: i32, sine: bool) {
        let rows = self.stream.rows();
        let signal = SignalRow::shown(sine);
        if self.on_ceiling {
            self.on_ceiling = false;
            self.ceiling.confirm = None;
            self.ceiling.error = None;
            match (d > 0, rows.first()) {
                (true, Some(first)) => self.stream.focus = *first,
                (true, None) => self.signal.focus = signal.first().copied(),
                (false, _) => self.signal.focus = signal.last().copied(),
            }
            return;
        }
        if let Some(now) = self.signal.focus {
            let i = signal.iter().position(|r| *r == now).unwrap_or(0) as i32 + d;
            self.signal.leave();
            match usize::try_from(i).ok().and_then(|i| signal.get(i)) {
                Some(r) => self.signal.focus = Some(*r),
                None if d > 0 => self.on_ceiling = true,
                None => match rows.last() {
                    Some(last) => self.stream.focus = *last,
                    None => self.on_ceiling = true,
                },
            }
            return;
        }
        let i = rows.iter().position(|r| *r == self.stream.focus);
        let leaving = match i {
            Some(i) => (d > 0 && i + 1 == rows.len()) || (d < 0 && i == 0),
            None => true,
        };
        if leaving {
            self.stream.finish_edit();
            self.stream.detect_cancel();
            if d > 0 {
                self.signal.focus = signal.first().copied();
            } else {
                self.on_ceiling = true;
            }
        } else {
            self.stream.detect_cancel();
            self.stream.move_focus(d);
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
