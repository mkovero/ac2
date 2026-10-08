//! The Settings view (Ctrl+P, the palette's "Settings…", the gear in the top bar): every
//! setting in one full-window view, a page per area, instead of one dialog per setting
//! found — or not — under Ctrl+K.
//!
//! Keyboard-first like every window (decision K9): the view owns the keyboard while open;
//! ↑/↓ move within the page, ←/→ change a choice, Enter applies, Esc closes the topmost
//! window (a calibration dialog or a confirmation over a page, else the view);
//! Ctrl+PageUp / Ctrl+PageDown (or Ctrl+Tab / Ctrl+Shift+Tab) step through the pages and
//! Alt+1 … Alt+7 jump to one. Shift+Esc still stops the stimulus from anywhere. The keys
//! that used to open a dialog open its page: Shift+O the Audio page, Shift+L the SPL / Leq
//! page, the palette's input setup and calibrations their pages.
//!
//! Every setting says whose it is: [`THIS_APP`] (kept in `ui.toml` on this computer) or
//! [`THE_RIG`] (kept by the daemon; every client sees and changes it).
//!
//! Pure data like the dialogs it hosts ([`SessionDialog`], [`CalView`], [`LeqDialog`]): the
//! lines and texts come from the mirrored state on every call, and the reducer turns the
//! actions into requests.

use ac2_proto::model::{Generator, ServerInfo, ServerMode};
use ac2_proto::units::Dbfs;
use ac2_scene::rig::{RAISE_WHILE_LIVE, RAISE_WORD};
use ac2_scene::theme::ThemeName;
use ac2_scene::view::{SpectrumMode, SweepMode};

use crate::cal_view::CalView;
use crate::leq_dialog::LeqDialog;
use crate::session_dialog::{Part, SessionDialog};

/// Whose a setting is: this computer's app alone.
pub const THIS_APP: &str = "this app";
/// Whose a setting is: the rig, for every client.
pub const THE_RIG: &str = "the rig — all clients";

/// A page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// Inputs, outputs, stimulus outputs, the reference, the system max level.
    Io,
    /// Backend, device, rate, buffer.
    Audio,
    /// Mics, curves, sensitivity calibrations.
    Calibration,
    /// The SPL meter's Leq windows and limits.
    Leq,
    /// The record toggle's limit, where recordings go.
    Recording,
    /// Theme, key hints, the SPL hold, the spectrograph, the level axes.
    Display,
    /// The daemon link, this client's key, the server's mode and keys.
    Connection,
}

impl Page {
    pub const ALL: [Page; 7] = [
        Page::Io,
        Page::Audio,
        Page::Calibration,
        Page::Leq,
        Page::Recording,
        Page::Display,
        Page::Connection,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Page::Io => "Inputs & outputs",
            Page::Audio => "Audio",
            Page::Calibration => "Calibration",
            Page::Leq => "SPL / Leq",
            Page::Recording => "Recording",
            Page::Display => "Display",
            Page::Connection => "Connection",
        }
    }

    /// Whose the page's settings are, as its header says.
    pub fn whose(self) -> &'static str {
        match self {
            Page::Io => {
                "channels, labels, the reference and the max level: the rig — all clients · \
                 stimulus outputs: this app"
            }
            Page::Audio | Page::Calibration | Page::Leq => THE_RIG,
            Page::Recording => "the limit: this app · the folder: the rig — all clients",
            Page::Display => THIS_APP,
            Page::Connection => {
                "the link and this client's key: this app · server and client keys: the rig — \
                 all clients"
            }
        }
    }

    /// The page's keys, under it.
    pub fn keys(self) -> &'static str {
        match self {
            Page::Io => {
                "↑↓ move · Space in session · R reference · M mic · S stimulus output · N names a \
                 mic or an output · ←→ a mic's curve · D detects the loopback · on the max \
                 level: type dBFS, Enter · Enter opens the session · Esc closes"
            }
            Page::Audio => {
                "↑↓ move · ←→ backend / input / output device · type the rate or buffer · Enter opens the \
                 session · Esc closes"
            }
            Page::Calibration => {
                "↑↓ move (PgUp/PgDn, Home/End) · ←→ mic curve of an input · N names the mic · I \
                 imports a curve file · R renames a curve · C calibrates with a calibrator · E \
                 calibrates electrically · Delete deletes (twice) · Esc closes"
            }
            Page::Leq => {
                "Enter applies (the log and the windows carry on) · ↑↓ row · Tab cell · ←→ \
                 choose · Insert or + adds a window to the section · Shift+Insert or + range… \
                 a band window per band from … to · Delete or − removes the focused one · T on \
                 a band row: band transfer · Esc closes"
            }
            Page::Recording => "↑↓ move · type minutes, Enter applies · Esc closes",
            Page::Display => "↑↓ move · ←→ change · Enter resets the level axes · Esc closes",
            Page::Connection => {
                "↑↓ move · Enter acts on the line · A authorizes a refused key (type its name) · \
                 Delete revokes a client (twice) · Esc closes"
            }
        }
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

/// The system max level row of the Inputs & outputs page.
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
                    ac2_scene::rig::dbfs(c.from.0)
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
        let v = match crate::state::parse_number(text, &["dbfs", "db"]) {
            Ok(v) => v,
            Err(e) => {
                self.error = Some(e);
                return None;
            }
        };
        if v > g.ceiling_bound.0 {
            self.error = Some(format!(
                "above this rig's bound {} (ac2d --max-level; only a restart with a higher bound \
                 raises it)",
                ac2_scene::rig::dbfs(g.ceiling_bound.0)
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
    PanesFollow,
    SplHold,
    SpectrumView,
    Spectrograph,
    SweepView,
    LevelAxes,
}

impl DisplayRow {
    pub const ALL: [DisplayRow; 8] = [
        DisplayRow::Theme,
        DisplayRow::KeyHints,
        DisplayRow::PanesFollow,
        DisplayRow::SplHold,
        DisplayRow::SpectrumView,
        DisplayRow::Spectrograph,
        DisplayRow::SweepView,
        DisplayRow::LevelAxes,
    ];

    pub fn title(self) -> &'static str {
        match self {
            DisplayRow::Theme => "Theme",
            DisplayRow::KeyHints => "Key hints",
            DisplayRow::PanesFollow => "Panes follow selection",
            DisplayRow::SplHold => "SPL number holds",
            DisplayRow::SpectrumView => "Spectrum pane shows",
            DisplayRow::SweepView => "Sweep pane shows",
            DisplayRow::Spectrograph => "Spectrograph history",
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

/// The spectrograph span ←/→ choose, s.
pub fn step_span(s: u32, d: i32) -> u32 {
    let all = ac2_scene::view::SPECTROGRAPH_SPANS_S;
    let i = all.iter().position(|x| *x == s).unwrap_or(0) as i32;
    all[(i + d).clamp(0, all.len() as i32 - 1) as usize]
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
        SweepMode::Ir => "the impulse response",
        SweepMode::Room => "the room parameters",
    }
}

/// The views of the panes the Display page shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneViews {
    pub spectrum: SpectrumMode,
    pub sweep: SweepMode,
}

/// What the Display page shows for each line: title, value, whose.
pub fn display_rows(
    theme: ThemeName,
    key_hints: bool,
    panes_follow: bool,
    spl_hold_ms: Option<u32>,
    span_s: u32,
    views: PaneViews,
) -> Vec<(DisplayRow, String)> {
    DisplayRow::ALL
        .iter()
        .map(|r| {
            let v = match r {
                DisplayRow::Theme => theme_name(theme).to_owned(),
                DisplayRow::KeyHints => {
                    if key_hints {
                        "shown under the focused pane".into()
                    } else {
                        "hidden".into()
                    }
                }
                DisplayRow::PanesFollow => {
                    if panes_follow {
                        "on: only the panes that draw the selected measurement".into()
                    } else {
                        "off: every pane".into()
                    }
                }
                DisplayRow::SplHold => match spl_hold_ms {
                    None => "by the time weighting".into(),
                    Some(ms) => {
                        format!("{} s", ac2_scene::format::fixed(f64::from(ms) / 1e3, 2))
                    }
                },
                DisplayRow::Spectrograph => format!("last {span_s} s"),
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

/// A line of the Connection page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnLine {
    /// Drop the link and connect again now.
    Reconnect,
    /// Choose another daemon (or pair with one) in the connect dialog.
    ConnectOther,
    /// An authorized client, by name.
    Authorized(String),
    /// A refused key (Z85), or a refused peer without CURVE (`None`).
    Refused(Option<String>, String),
    /// Authorize a key typed with its name.
    AddKey,
}

/// What the Connection page asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnAction {
    Reconnect,
    ConnectOther,
    Authorize { name: String, key: String },
    Revoke(String),
}

/// A typed edit of the Connection page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnEdit {
    /// The name a refused key is authorized under.
    Name { key: String },
    /// `name key`, for a key not seen yet.
    NameAndKey,
}

/// The Connection page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConnectionPage {
    pub focus: usize,
    /// The daemon's last `server.info` (or why it could not be asked).
    pub server: Option<Result<ServerInfo, String>>,
    /// When it was last asked for, `now_s`.
    pub asked_s: Option<f64>,
    pub edit: Option<(ConnEdit, String)>,
    /// The authorized client Delete was pressed on once; a second press revokes.
    pub confirm: Option<String>,
    pub notice: Option<String>,
    pub error: Option<String>,
}

/// Seconds between `server.info` while the Connection page is shown: refused keys appear
/// while the operator watches for a new client to knock.
pub const SERVER_INFO_EVERY_S: f64 = 2.0;

/// The page's lines for `server` (the actions, then the network mode's keys).
pub fn conn_lines(server: Option<&ServerInfo>) -> Vec<ConnLine> {
    let mut v = vec![ConnLine::Reconnect, ConnLine::ConnectOther];
    if let Some(ServerInfo {
        mode:
            ServerMode::Network {
                authorized,
                refused,
                ..
            },
        ..
    }) = server
    {
        v.extend(
            authorized
                .iter()
                .map(|a| ConnLine::Authorized(a.name.clone())),
        );
        v.extend(
            refused
                .iter()
                .map(|r| ConnLine::Refused(r.key.clone(), r.address.clone())),
        );
        v.push(ConnLine::AddKey);
    }
    v
}

impl ConnectionPage {
    fn server_info(&self) -> Option<&ServerInfo> {
        self.server.as_ref().and_then(|r| r.as_ref().ok())
    }

    /// The lines as of the last answer.
    pub fn lines(&self) -> Vec<ConnLine> {
        conn_lines(self.server_info())
    }

    /// The focused line (clamped: lines come and go with the daemon's answers).
    pub fn focused(&self) -> Option<ConnLine> {
        let l = self.lines();
        l.get(self.focus.min(l.len().saturating_sub(1))).cloned()
    }

    fn clear(&mut self) {
        self.confirm = None;
        self.notice = None;
        self.error = None;
    }

    pub fn move_focus(&mut self, d: i32) {
        self.clear();
        self.edit = None;
        let n = self.lines().len() as i32;
        if n > 0 {
            self.focus = (self.focus.min(n as usize - 1) as i32 + d).rem_euclid(n) as usize;
        }
    }

    /// Enter: what the focused line does, or the typed edit's result.
    pub fn enter(&mut self) -> Option<ConnAction> {
        self.notice = None;
        self.error = None;
        if let Some((kind, text)) = self.edit.take() {
            let t = text.trim();
            return match kind {
                ConnEdit::Name { key } => {
                    if t.is_empty() || t.contains(char::is_whitespace) {
                        self.error = Some("type a name without spaces (e.g. tablet)".into());
                        self.edit = Some((ConnEdit::Name { key }, text));
                        None
                    } else {
                        Some(ConnAction::Authorize {
                            name: t.to_owned(),
                            key,
                        })
                    }
                }
                ConnEdit::NameAndKey => {
                    let mut parts = t.split_whitespace();
                    match (parts.next(), parts.next(), parts.next()) {
                        (Some(name), Some(key), None) => Some(ConnAction::Authorize {
                            name: name.to_owned(),
                            key: key.to_owned(),
                        }),
                        _ => {
                            self.error = Some(
                                "type the name, a space and the client's key (40 characters, as \
                                 `ac2 auth show` prints it)"
                                    .into(),
                            );
                            self.edit = Some((ConnEdit::NameAndKey, text));
                            None
                        }
                    }
                }
            };
        }
        match self.focused()? {
            ConnLine::Reconnect => Some(ConnAction::Reconnect),
            ConnLine::ConnectOther => Some(ConnAction::ConnectOther),
            ConnLine::AddKey => {
                self.edit = Some((ConnEdit::NameAndKey, String::new()));
                None
            }
            ConnLine::Refused(..) => {
                self.start_authorize();
                None
            }
            ConnLine::Authorized(_) => {
                self.notice = Some("Delete twice revokes this client's key".into());
                None
            }
        }
    }

    /// A on a refused key: types the name it is authorized under.
    pub fn start_authorize(&mut self) -> bool {
        self.clear();
        match self.focused() {
            Some(ConnLine::Refused(Some(key), _)) => {
                self.edit = Some((ConnEdit::Name { key }, String::new()));
                true
            }
            Some(ConnLine::Refused(None, _)) => {
                self.error = Some(
                    "that peer did not use CURVE: it has no key to authorize (an old or foreign \
                     client)"
                        .into(),
                );
                false
            }
            _ => {
                self.notice =
                    Some("A authorizes a refused key: move to one under Refused keys".into());
                false
            }
        }
    }

    /// Delete: revokes the focused authorized client on the second press.
    pub fn delete(&mut self) -> Option<ConnAction> {
        self.notice = None;
        self.error = None;
        let Some(ConnLine::Authorized(name)) = self.focused() else {
            self.notice = Some("Delete revokes an authorized client: move to one".into());
            return None;
        };
        if self.confirm.as_deref() == Some(name.as_str()) {
            self.confirm = None;
            return Some(ConnAction::Revoke(name));
        }
        self.confirm = Some(name.clone());
        self.notice = Some(format!(
            "Delete again revokes {name}: it is refused at once and cannot connect again"
        ));
        None
    }

    pub fn typing(&self) -> bool {
        self.edit.is_some()
    }

    pub fn type_text(&mut self, s: &str) {
        if let Some((_, t)) = &mut self.edit {
            t.push_str(s);
            self.error = None;
        }
    }

    pub fn backspace(&mut self) {
        if let Some((_, t)) = &mut self.edit {
            t.pop();
        }
    }

    /// What the edit line says before the typed text.
    pub fn edit_label(&self) -> Option<&'static str> {
        match &self.edit {
            Some((ConnEdit::Name { .. }, _)) => Some("Authorize this key as (name):"),
            Some((ConnEdit::NameAndKey, _)) => Some("Authorize a client (name, space, key):"),
            None => None,
        }
    }
}

/// This client's key, as the Connection page shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientKey {
    pub fingerprint: String,
    /// Z85, for the daemon operator to authorize.
    pub key: String,
}

/// The Settings view's own state.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub page: Page,
    /// Inputs & outputs and Audio.
    pub session: SessionDialog,
    /// The max level row is focused on the Inputs & outputs page (below the channels).
    pub on_ceiling: bool,
    pub ceiling: CeilingEdit,
    pub cal: CalView,
    /// The Leq windows of the SPL pane's meter; `None`: there is no SPL meter.
    pub leq: Option<LeqDialog>,
    pub display: DisplayRow,
    pub recording: RecordingPage,
    pub connection: ConnectionPage,
    /// Opened for the stimulus outputs before the device's rows arrived: the focus goes to
    /// them once they do.
    pub focus_outputs: bool,
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
            focus_outputs: false,
        };
        s.show(page);
        s
    }

    /// Focuses the stimulus output (the first ticked one, else the first output); `false`
    /// while there are no output rows yet.
    pub fn focus_stimulus_outputs(&mut self) -> bool {
        let d = &mut self.session;
        let i = d
            .outputs
            .iter()
            .position(|o| o.stimulus)
            .or((!d.outputs.is_empty()).then_some(0));
        match i {
            Some(i) => {
                self.show(Page::Io);
                self.on_ceiling = false;
                self.session
                    .focus_row(crate::session_dialog::Row::Output(i));
                true
            }
            None => false,
        }
    }

    /// Shows `page`.
    pub fn show(&mut self, page: Page) {
        self.page = page;
        match page {
            Page::Io => self.session.show_part(Part::Channels),
            Page::Audio => self.session.show_part(Part::Device),
            _ => {}
        }
    }

    /// Shows the page `r` is a row of (Inputs & outputs or Audio).
    pub fn show_row(&mut self, r: crate::session_dialog::Row) {
        use crate::session_dialog::Row;
        let page = match r {
            Row::Input(_) | Row::Output(_) => Page::Io,
            Row::Backend | Row::Device | Row::OutputDevice | Row::Rate | Row::Buffer => Page::Audio,
        };
        if self.page != page {
            self.show(page);
        }
        if page == Page::Io {
            self.on_ceiling = false;
        }
    }

    /// Whether a typed edit or text row owns the keys that otherwise act on the page.
    pub fn typing(&self) -> bool {
        match self.page {
            Page::Io if self.on_ceiling => true,
            Page::Io | Page::Audio => self.session.text_focus(),
            Page::Calibration => self.cal.typing(),
            Page::Leq => self.leq.is_some(),
            Page::Recording => self.recording.focus == RecordingRow::Limit,
            Page::Display => false,
            Page::Connection => self.connection.typing(),
        }
    }

    /// An inner window is open over the page (Esc closes it, not the view).
    pub fn inner_open(&self) -> bool {
        match self.page {
            Page::Calibration => self.cal.electrical.is_some() || self.cal.acoustic.is_some(),
            Page::Io => self.ceiling.confirm.is_some(),
            Page::Leq => self.leq.as_ref().is_some_and(|d| d.transfer.is_some()),
            _ => false,
        }
    }

    /// Esc on an inner window: closes it. `false` when there is none.
    pub fn close_inner(&mut self) -> bool {
        match self.page {
            Page::Calibration => self.cal.close_dialog(),
            Page::Io if self.ceiling.confirm.is_some() => {
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

    /// ↑/↓ on the Inputs & outputs page: the channel rows, then the max level row.
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

/// The Connection page's "this app" lines: what the link is, who this client is, its key.
pub fn link_lines(
    target: &str,
    state: &str,
    server: Option<&str>,
    client_id: Option<&str>,
    key: Option<&ClientKey>,
) -> Vec<String> {
    let mut v = vec![format!("Daemon: {target} · {state}")];
    if let Some(s) = server {
        v.push(format!("Server software: {s}"));
    }
    if let Some(c) = client_id {
        v.push(format!("This client is {c} on the daemon"));
    }
    v.push(match key {
        Some(k) => format!(
            "This client's key: fingerprint {} · {} (what a rig's operator authorizes)",
            k.fingerprint, k.key
        ),
        None => "This client has no key yet: pairing with a rig (connect dialog) makes one; local \
                 and embedded daemons need none"
            .into(),
    });
    v
}

/// The Connection page's action lines.
pub fn conn_line_text(l: &ConnLine, server: Option<&ServerInfo>) -> String {
    match l {
        ConnLine::Reconnect => "Reconnect now (Enter)".into(),
        ConnLine::ConnectOther => "Connect to another daemon, or pair with a rig… (Enter)".into(),
        ConnLine::Authorized(name) => server
            .and_then(|s| match &s.mode {
                ServerMode::Network { authorized, .. } => authorized
                    .iter()
                    .find(|a| &a.name == name)
                    .map(ac2_scene::rig::authorized_row),
                _ => None,
            })
            .unwrap_or_else(|| name.clone()),
        ConnLine::Refused(..) => String::new(),
        ConnLine::AddKey => "Authorize a client by its key… (Enter)".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::GenAudit;

    fn generator(ceiling: f64, bound: f64) -> Generator {
        Generator {
            owner: None,
            armed: false,
            firing: false,
            settings: None,
            ceiling: Dbfs(ceiling),
            ceiling_bound: Dbfs(bound),
            last_action: None::<GenAudit>,
        }
    }

    #[test]
    fn pages_step_and_wrap() {
        assert_eq!(Page::Io.step(1), Page::Audio);
        assert_eq!(Page::Io.step(-1), Page::Connection);
        assert_eq!(Page::Connection.step(1), Page::Io);
        for p in Page::ALL {
            assert!(!p.title().is_empty() && !p.keys().is_empty());
            assert!(p.whose().contains(THIS_APP) || p.whose().contains(THE_RIG));
        }
    }

    #[test]
    fn lowering_goes_out_at_once_a_raise_asks_for_the_word() {
        let g = generator(-40.0, -10.0);
        let mut e = CeilingEdit {
            text: "-50".into(),
            ..CeilingEdit::default()
        };
        assert_eq!(e.enter(Some(&g), true), Some((Dbfs(-50.0), false)));
        assert!(e.text.is_empty());

        // Above the bound: refused here, saying where the bound comes from.
        e.type_text("-6");
        assert_eq!(e.enter(Some(&g), false), None);
        assert!(
            e.error
                .as_deref()
                .expect("set")
                .contains("ac2d --max-level")
        );

        // A raise: refused while live, else a confirmation that needs the word.
        e.text = "-20 dBFS".into();
        assert_eq!(e.enter(Some(&g), true), None);
        assert_eq!(e.error.as_deref(), Some(RAISE_WHILE_LIVE));
        assert_eq!(e.enter(Some(&g), false), None);
        let c = e.confirm.clone().expect("set");
        assert_eq!((c.from, c.to), (Dbfs(-40.0), Dbfs(-20.0)));
        e.type_text("yes");
        assert_eq!(e.enter(Some(&g), false), None);
        assert!(e.error.as_deref().expect("set").starts_with("type raise"));
        e.confirm.as_mut().expect("set").typed.clear();
        e.type_text("Raise");
        assert_eq!(e.enter(Some(&g), false), Some((Dbfs(-20.0), true)));
        assert!(e.confirm.is_none());

        e.type_text("loud");
        assert_eq!(e.enter(Some(&g), false), None);
        assert!(e.error.as_deref().expect("set").starts_with("not a number"));
    }

    #[test]
    fn display_and_recording_texts() {
        assert_eq!(step_theme(ThemeName::Dark, 1), ThemeName::Light);
        assert_eq!(step_theme(ThemeName::Dark, -1), ThemeName::HighContrast);
        assert_eq!(step_hold(None, 1), Some(250));
        assert_eq!(step_hold(None, -1), None);
        assert_eq!(step_span(10, 1), 30);
        assert_eq!(step_span(120, 1), 120);
        let views = PaneViews {
            spectrum: SpectrumMode::Split,
            sweep: SweepMode::Room,
        };
        let rows = display_rows(ThemeName::Light, false, false, Some(500), 30, views);
        let texts: Vec<&str> = rows.iter().map(|(_, v)| v.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "light",
                "hidden",
                "off: every pane",
                "0.50 s",
                "the spectrum over its spectrograph",
                "last 30 s",
                "the room parameters",
                "each pane's as last left · Enter resets them to the defaults",
            ]
        );
        let rows = recording_rows(60, None);
        assert_eq!(rows[0].1, "the record toggle stops by itself after 60 min");
        assert_eq!(rows[1].1, "asking the daemon…");
        let s = ac2_proto::samples::server_info();
        assert_eq!(
            recording_rows(60, Some(&s))[1].1,
            "/home/fohtech/.local/share/ac2/recordings (ac2d --recordings)"
        );
        let mut r = RecordingPage {
            text: "90 min".into(),
            ..RecordingPage::default()
        };
        assert_eq!(r.enter(), Some(90));
        r.text = "0".into();
        assert_eq!(r.enter(), None);
        assert_eq!(r.error.as_deref(), Some("type whole minutes, 1 … 480"));
    }

    #[test]
    fn connection_lines_authorize_and_revoke() {
        let s = ac2_proto::samples::server_info();
        let mut p = ConnectionPage {
            server: Some(Ok(s.clone())),
            ..ConnectionPage::default()
        };
        let lines = p.lines();
        assert_eq!(lines[0], ConnLine::Reconnect);
        assert_eq!(lines[1], ConnLine::ConnectOther);
        assert_eq!(lines[2], ConnLine::Authorized("laptop".into()));
        assert!(matches!(&lines[3], ConnLine::Refused(Some(_), a) if a == "192.168.1.40"));
        assert!(matches!(&lines[4], ConnLine::Refused(None, _)));
        assert_eq!(lines[5], ConnLine::AddKey);
        assert_eq!(p.enter(), Some(ConnAction::Reconnect));

        // Revoking takes two presses.
        p.focus = 2;
        assert_eq!(p.delete(), None);
        assert!(
            p.notice
                .as_deref()
                .expect("set")
                .starts_with("Delete again revokes laptop")
        );
        assert_eq!(p.delete(), Some(ConnAction::Revoke("laptop".into())));

        // A refused key: a name, then Enter authorizes it.
        p.focus = 3;
        assert!(p.start_authorize());
        p.type_text("tablet");
        let key = "D:)Q[IlAW!ahhC2ac:9*A}h:p?([4%wOTJ%JR%cs".to_owned();
        assert_eq!(
            p.enter(),
            Some(ConnAction::Authorize {
                name: "tablet".into(),
                key
            })
        );
        // A peer without CURVE has nothing to authorize.
        p.focus = 4;
        assert!(!p.start_authorize());

        // By hand: name and key.
        p.focus = 5;
        assert_eq!(p.enter(), None);
        p.type_text("phone");
        assert_eq!(p.enter(), None);
        assert!(p.error.is_some(), "a name alone is not enough");
        p.type_text(" KEY40");
        assert_eq!(
            p.enter(),
            Some(ConnAction::Authorize {
                name: "phone".into(),
                key: "KEY40".into()
            })
        );
        assert_eq!(
            conn_line_text(&ConnLine::Authorized("laptop".into()), Some(&s)),
            "laptop · fingerprint SHA256:b2aa 0c3d 9e41 7f60"
        );
    }

    #[test]
    fn link_lines_say_who_and_which_key() {
        let k = ClientKey {
            fingerprint: "1a2b-3c4d".into(),
            key: "Z85KEY".into(),
        };
        assert_eq!(
            link_lines(
                "foh-rig",
                "connected",
                Some("ac2d 0.1"),
                Some("laptop"),
                Some(&k)
            ),
            vec![
                "Daemon: foh-rig · connected",
                "Server software: ac2d 0.1",
                "This client is laptop on the daemon",
                "This client's key: fingerprint 1a2b-3c4d · Z85KEY (what a rig's operator \
                 authorizes)",
            ]
        );
    }
}
