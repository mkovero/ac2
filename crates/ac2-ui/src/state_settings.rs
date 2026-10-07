//! The reducer's side of the Settings view ([`crate::settings`]): opening it at a page, its
//! keys, typed text and mouse, and the requests its pages make.

use ac2_proto::Command;
use ac2_proto::model::OutputSetup;
use eframe::egui::Key;

use super::{AppState, Overlay, PaneKind, StimPhase, typed_char};
use crate::cal_view::CalView;
use crate::conn::Request;
use crate::keys::{Chord, CommandId, Keymap, Scope};
use crate::leq_dialog::LeqDialog;
use crate::session_dialog::{Edit, RoleKey, Row, SessionDialog};
use crate::settings::{
    ConnAction, DisplayRow, Page, RecordingRow, SERVER_INFO_EVERY_S, Settings, step_hold,
    step_span, step_theme,
};

/// What the mouse does on the Settings view (the hosted pages keep their own messages).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsMsg {
    /// A page in the sidebar.
    Page(Page),
    /// The close button: as Esc.
    Close,
    /// The system max level row.
    Ceiling,
    /// Apply what is typed on the max level row (or its confirmation).
    CeilingApply,
    /// ‹/› on a line of the Display page (0: the line's action).
    Display(DisplayRow, i32),
    /// A line of the Connection page: focused and acted on.
    Connection(usize),
}

impl AppState {
    /// Opens Settings at `page` (or shows `page` of the open view).
    pub(super) fn open_settings(&mut self, page: Page, out: &mut Vec<Request>) {
        self.settings_page_last = page;
        if let Overlay::Settings(s) = &mut self.overlay {
            s.show(page);
            self.settings_entered(out);
            return;
        }
        let open = self.open_session().cloned();
        let setup = self.daemon().map(|s| s.inputs.clone()).unwrap_or_default();
        let labels = self.daemon().map(|s| s.outputs.clone()).unwrap_or_default();
        let mut session = SessionDialog::new(open.as_ref(), &setup);
        session.set_labels(&labels);
        if let Some(d) = &self.devices {
            session.set_backends(d.clone(), &self.prefs);
        }
        // The calibrations start on the input the selected measurement listens on.
        let on = self
            .selected_meas()
            .and_then(|m| super::meas_input(&m.config.kind));
        let cal = self
            .daemon()
            .map_or_else(CalView::default, |s| CalView::new(s, on));
        let leq = self.pane_meas(PaneKind::Spl).cloned().and_then(|m| {
            let calibrated = self.leq_calibrated(m.id);
            LeqDialog::new(&m, calibrated)
        });
        self.overlay = Overlay::Settings(Box::new(Settings::new(page, session, cal, leq)));
        if self.connected() {
            out.push(Request::Devices);
        }
        self.settings_entered(out);
    }

    /// A page was shown: it asks for what it shows.
    fn settings_entered(&mut self, out: &mut Vec<Request>) {
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        if matches!(s.page, Page::Connection | Page::Recording)
            && matches!(self.conn, super::ConnState::Connected { .. })
        {
            s.connection.asked_s = Some(self.now_s);
            out.push(Request::ServerInfo);
        }
    }

    /// Asks for the daemon's server info again while the Connection page is shown (a new
    /// client's refused key appears while the operator waits for it).
    pub(super) fn poll_settings(&mut self, out: &mut Vec<Request>) {
        let connected = self.connected();
        let now = self.now_s;
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        if s.page == Page::Connection
            && connected
            && s.connection
                .asked_s
                .is_none_or(|t| now - t >= SERVER_INFO_EVERY_S)
        {
            s.connection.asked_s = Some(now);
            out.push(Request::ServerInfo);
        }
    }

    /// The pages a mirror change reaches: the rig's output labels.
    pub(super) fn follow_settings(&mut self) {
        let labels = self.daemon().map(|s| s.outputs.clone()).unwrap_or_default();
        if let Overlay::Settings(s) = &mut self.overlay {
            s.session.set_labels(&labels);
        }
    }

    fn settings_page(&mut self, page: Page, out: &mut Vec<Request>) {
        self.settings_page_last = page;
        if let Overlay::Settings(s) = &mut self.overlay {
            s.show(page);
        }
        self.settings_entered(out);
    }

    /// Keys of the Settings view. Esc never gets here: it closes the topmost window.
    pub(super) fn settings_key(
        &mut self,
        chord: Chord,
        swallow: Option<char>,
        keymap: &Keymap,
        out: &mut Vec<Request>,
    ) {
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        let page = s.page;
        // Pages: Ctrl+PageUp / PageDown and Ctrl+Tab step, Alt+digit jumps.
        if chord.command
            && !chord.alt
            && matches!(chord.key, Key::PageUp | Key::PageDown | Key::Tab)
        {
            let d = if chord.key == Key::PageUp || (chord.key == Key::Tab && chord.shift) {
                -1
            } else {
                1
            };
            self.settings_page(page.step(d), out);
            return;
        }
        if chord.alt && !chord.command {
            let n = match chord.key {
                Key::Num1 => Some(0),
                Key::Num2 => Some(1),
                Key::Num3 => Some(2),
                Key::Num4 => Some(3),
                Key::Num5 => Some(4),
                Key::Num6 => Some(5),
                Key::Num7 => Some(6),
                _ => None,
            };
            if let Some(n) = n {
                self.settings_page(Page::ALL[n], out);
                return;
            }
        }
        // The Settings key itself closes the view, as the palette's key closes the palette.
        if !s.typing() && keymap.lookup(Scope::Global, chord) == Some(CommandId::Settings) {
            self.overlay = Overlay::None;
            return;
        }
        match page {
            Page::Io | Page::Audio => self.session_key(chord, swallow, out),
            Page::Calibration => self.cal_view_key(chord, swallow, out),
            Page::Leq => self.leq_key(chord, swallow, out),
            Page::Recording => self.recording_key(chord, swallow),
            Page::Display => self.display_key(chord, out),
            Page::Connection => self.connection_key(chord, swallow, out),
        }
    }

    fn leq_key(&mut self, chord: Chord, swallow: Option<char>, out: &mut Vec<Request>) {
        let Some(d) = self.overlay.leq_mut() else {
            self.swallow_text = swallow;
            return;
        };
        match chord.key {
            Key::Enter => self.submit_leq(out),
            Key::ArrowUp => d.move_row(-1),
            Key::ArrowDown => d.move_row(1),
            Key::Tab if chord.shift => d.move_cell(-1),
            Key::Tab => d.move_cell(1),
            Key::ArrowLeft => d.cycle(-1),
            Key::ArrowRight => d.cycle(1),
            Key::Insert => d.add_window(),
            Key::Delete => d.remove_window(),
            Key::A if chord.command => d.select_all(),
            _ => self.swallow_text = swallow,
        }
    }

    fn recording_key(&mut self, chord: Chord, swallow: Option<char>) {
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        match chord.key {
            Key::ArrowUp | Key::ArrowDown | Key::Tab => s.recording_move(),
            Key::Enter => {
                if let Some(m) = s.recording.enter() {
                    self.prefs.record_limit_min = Some(m);
                    self.prefs_dirty = true;
                    self.toast(format!("the record toggle now stops after {m} min"));
                }
            }
            _ => self.swallow_text = swallow,
        }
    }

    fn display_key(&mut self, chord: Chord, out: &mut Vec<Request>) {
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        let row = s.display;
        match chord.key {
            Key::ArrowUp => s.display_move(-1),
            Key::ArrowDown | Key::Tab => s.display_move(1),
            Key::ArrowLeft => self.display_change(row, -1, out),
            Key::ArrowRight | Key::Space => self.display_change(row, 1, out),
            Key::Enter => self.display_change(row, 0, out),
            _ => {}
        }
    }

    /// ←/→ (`d`) or Enter (`0`) on a line of the Display page: changes it at once.
    fn display_change(&mut self, row: DisplayRow, d: i32, out: &mut Vec<Request>) {
        if let Overlay::Settings(s) = &mut self.overlay {
            s.display = row;
        }
        match row {
            DisplayRow::Theme if d != 0 => {
                self.theme = step_theme(self.theme, d);
                self.prefs.theme = Some(self.theme);
                self.prefs_dirty = true;
            }
            DisplayRow::KeyHints if d != 0 => {
                self.prefs.key_hints = !self.prefs.key_hints;
                self.prefs_dirty = true;
            }
            DisplayRow::SplHold if d != 0 => {
                self.prefs.spl_hold_ms = step_hold(self.prefs.spl_hold_ms, d);
                self.prefs_dirty = true;
            }
            DisplayRow::Spectrograph if d != 0 => {
                let sg = &mut self.view.spectrum.spectrograph;
                let span = step_span(sg.span_s, d);
                if span != sg.span_s {
                    sg.span_s = span;
                    sg.cursor_s = sg.cursor_s.filter(|t| *t <= f64::from(span));
                    // Slots of another length cannot hold the frames already placed.
                    self.spectrographs.clear();
                    self.prefs.spectrograph_span_s = Some(span);
                    self.prefs_dirty = true;
                }
            }
            // The panes' G keys, so the spectrograph's history comes and goes as it does
            // there; which panes show and which has the focus stay as they were.
            DisplayRow::SpectrumView | DisplayRow::SweepView if d != 0 => {
                let layout = self.layout;
                let c = if row == DisplayRow::SpectrumView {
                    CommandId::Spectrograph
                } else {
                    CommandId::SweepView
                };
                // Three views: back one is forward two.
                for _ in 0..if d > 0 { 1 } else { 2 } {
                    self.command(c, &Keymap::default(), out);
                }
                self.layout = layout;
            }
            DisplayRow::LevelAxes if d == 0 => {
                crate::prefs::LevelPrefs::default().apply(&mut self.view);
                self.toast("every pane's level axis is back at its default");
            }
            _ => {}
        }
    }

    fn connection_key(&mut self, chord: Chord, swallow: Option<char>, out: &mut Vec<Request>) {
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        let plain = !(chord.command || chord.alt);
        let p = &mut s.connection;
        let action = match chord.key {
            Key::ArrowUp => {
                p.move_focus(-1);
                None
            }
            Key::ArrowDown | Key::Tab => {
                p.move_focus(1);
                None
            }
            Key::Enter => p.enter(),
            _ if p.typing() => {
                self.swallow_text = swallow;
                None
            }
            Key::A if plain => {
                if p.start_authorize() {
                    self.swallow_text = typed_char(&chord);
                }
                None
            }
            Key::Delete | Key::Backspace => p.delete(),
            _ => None,
        };
        if let Some(a) = action {
            self.connection_action(a, out);
        }
    }

    fn connection_action(&mut self, a: ConnAction, out: &mut Vec<Request>) {
        match a {
            ConnAction::Reconnect => out.push(Request::Reconnect),
            ConnAction::ConnectOther => self.want_connect_dialog = true,
            ConnAction::Authorize { name, key } => out.push(Request::ServerCall {
                what: format!("client {name} authorized"),
                cmd: Command::ServerAuthorize { name, key },
            }),
            ConnAction::Revoke(name) => out.push(Request::ServerCall {
                what: format!("client {name} revoked"),
                cmd: Command::ServerRevoke { name },
            }),
        }
    }

    /// Keys of the Inputs & outputs and Audio pages (the session model). Esc never gets
    /// here: it closes the topmost window.
    pub(super) fn session_key(
        &mut self,
        chord: Chord,
        swallow: Option<char>,
        out: &mut Vec<Request>,
    ) {
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        let io = s.page == Page::Io;
        if io && s.on_ceiling {
            match chord.key {
                Key::Enter => self.apply_ceiling(out),
                Key::ArrowUp => s.io_move(-1),
                Key::Tab if chord.shift => s.io_move(-1),
                Key::ArrowDown | Key::Tab => s.io_move(1),
                _ => self.swallow_text = swallow,
            }
            return;
        }
        let d = &mut s.session;
        let plain = !(chord.command || chord.alt);
        match chord.key {
            Key::Enter => {
                if d.edit == Some(Edit::DetectLevel) {
                    self.session_msg(super::SessionMsg::DetectConfirm, out);
                } else if matches!(d.edit, Some(Edit::OutputLabel(_))) {
                    match d.commit_label() {
                        Ok(Some(row)) => self.send_label(row, out),
                        Ok(None) => {}
                        Err(e) => d.error = Some(e),
                    }
                } else if matches!(d.edit, Some(Edit::Mic(_))) {
                    d.finish_edit();
                } else {
                    self.submit_session(out);
                }
            }
            Key::ArrowUp | Key::ArrowDown | Key::Tab => {
                let back = chord.key == Key::ArrowUp || (chord.key == Key::Tab && chord.shift);
                if io {
                    s.io_move(if back { -1 } else { 1 });
                } else {
                    d.detect_cancel();
                    d.move_focus(if back { -1 } else { 1 });
                }
            }
            Key::ArrowLeft | Key::ArrowRight if d.edit.is_none() => {
                let forward = chord.key == Key::ArrowRight;
                if matches!(d.focus, Row::Input(_)) {
                    let st = self.mirror.as_ref().and_then(|m| m.state.clone());
                    let mics = st.as_ref().map(|s| s.mics.as_slice()).unwrap_or_default();
                    let live = st.as_ref().map(|s| s.inputs.as_slice()).unwrap_or_default();
                    if let Some(row) = d.step_curve(forward, mics, live) {
                        // The session already captures this mic: the choice applies now.
                        let what = super::curve_what(&row);
                        out.push(Request::Call {
                            what,
                            cmd: Command::SessionInputs { inputs: vec![row] },
                        });
                    }
                } else {
                    d.cycle(if forward { 1 } else { -1 }, &self.prefs);
                }
            }
            // Everything below types text while a text row or edit has the keyboard.
            _ if d.text_focus() => self.swallow_text = swallow,
            Key::Space if plain => d.toggle(),
            Key::R if plain => d.assign(RoleKey::Reference),
            Key::M if plain => d.assign(RoleKey::Mic),
            Key::S if plain => {
                d.assign(RoleKey::Stimulus);
                if matches!(d.focus, Row::Output(_)) {
                    self.stimulus_ticks_now(out);
                }
            }
            Key::N | Key::F2 if plain => {
                let started = if matches!(d.focus, Row::Output(_)) {
                    d.start_label_edit()
                } else {
                    d.start_mic_edit()
                };
                if started {
                    // The N that started the edit also arrives as text.
                    self.swallow_text = typed_char(&chord);
                }
            }
            Key::D if plain => {
                self.session_msg(super::SessionMsg::Detect, out);
                if self
                    .overlay
                    .session()
                    .is_some_and(|d| d.edit == Some(Edit::DetectLevel))
                {
                    self.swallow_text = typed_char(&chord);
                }
            }
            _ => self.swallow_text = swallow,
        }
    }

    /// `session.outputs` with one label.
    fn send_label(&mut self, row: OutputSetup, out: &mut Vec<Request>) {
        let what = match &row.label {
            Some(l) => format!("output {}: named {l} (every client)", row.channel + 1),
            None => format!("output {}: label cleared", row.channel + 1),
        };
        out.push(Request::Call {
            what,
            cmd: Command::SessionOutputs { outputs: vec![row] },
        });
    }

    /// S on an output: the stimulus plays on the ticked outputs from now on when the open
    /// session already has them (no reopen: the stream carries every session output);
    /// otherwise from the next session open (Enter).
    pub(super) fn stimulus_ticks_now(&mut self, out: &mut Vec<Request>) {
        let open_outputs = self.open_session().map(|o| o.config.output_channels);
        let labels = self.daemon().map(|s| s.outputs.clone()).unwrap_or_default();
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        let d = &mut s.session;
        let ticked: Vec<u16> = d
            .outputs
            .iter()
            .filter(|o| o.stimulus && o.channel < d.out_count)
            .map(|o| o.channel)
            .collect();
        let device = d.device_info().map(|x| x.id.0.clone());
        let Some(n) = open_outputs.filter(|_| d.is_open_device()) else {
            d.notice = Some("the stimulus outputs apply when the session opens (Enter)".into());
            return;
        };
        if ticked.is_empty() || ticked.iter().any(|c| *c >= n) {
            d.notice = Some(
                "the session opens again with these outputs on Enter (no output ticked, or one \
                 it does not have yet)"
                    .into(),
            );
            return;
        }
        let names = ac2_scene::rig::outputs_text(&ticked, &labels, |c| {
            d.outputs
                .iter()
                .find(|o| o.channel == c)
                .and_then(|o| o.device_name.clone())
        });
        d.notice = Some(format!("the stimulus plays on {names} (this app)"));
        if let Some(dev) = device {
            self.prefs.outputs.insert(dev.clone(), ticked.clone());
            self.prefs_dirty = true;
            if self.stimulus.phase == StimPhase::Idle
                || matches!(self.stimulus.phase, StimPhase::Armed | StimPhase::Firing)
            {
                self.stimulus.outputs = ticked;
                self.stim_device = Some(dev);
                self.resend_stimulus(out);
            }
        }
    }

    /// Enter on the system max level row.
    fn apply_ceiling(&mut self, out: &mut Vec<Request>) {
        let live = self.stimulus_live();
        let g = self.daemon().map(|s| s.generator.clone());
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        if let (Some((to, confirm)), Some(g)) = (s.ceiling.enter(g.as_ref(), live), &g) {
            out.push(Request::Call {
                what: ac2_scene::rig::ceiling_changed(g.ceiling, to),
                cmd: Command::GenCeiling {
                    ceiling: to,
                    confirm_raise: confirm,
                },
            });
        }
    }

    /// Typed text on the Settings view.
    pub(super) fn settings_text(&mut self, t: &str) {
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        match s.page {
            Page::Io if s.on_ceiling => s.ceiling.type_text(t),
            Page::Io | Page::Audio => s.session.type_text(t),
            Page::Calibration => s.cal.type_text(t),
            Page::Leq => {
                if let Some(d) = &mut s.leq {
                    d.type_text(t);
                }
            }
            Page::Recording => {
                if s.recording.focus == RecordingRow::Limit {
                    s.recording.text.push_str(t);
                    s.recording.error = None;
                }
            }
            Page::Connection => s.connection.type_text(t),
            Page::Display => {}
        }
    }

    /// Backspace on the Settings view: deletes typed text, or (not typing) acts as Delete
    /// where Delete deletes.
    pub(super) fn settings_backspace(&mut self, out: &mut Vec<Request>) {
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        match s.page {
            Page::Io if s.on_ceiling => s.ceiling.backspace(),
            Page::Io | Page::Audio => s.session.backspace(),
            Page::Calibration if s.cal.typing() => s.cal.backspace(),
            Page::Calibration => self.cal_view_key(Chord::key(Key::Delete), None, out),
            Page::Leq => {
                if let Some(d) = &mut s.leq {
                    d.backspace();
                }
            }
            Page::Recording => {
                s.recording.text.pop();
            }
            Page::Connection if s.connection.typing() => s.connection.backspace(),
            Page::Connection => {
                if let Some(a) = s.connection.delete() {
                    self.connection_action(a, out);
                }
            }
            Page::Display => {}
        }
    }

    /// The mouse on the Settings view.
    pub(super) fn settings_msg(&mut self, m: SettingsMsg, out: &mut Vec<Request>) {
        match m {
            SettingsMsg::Page(p) => self.settings_page(p, out),
            SettingsMsg::Close => self.close_overlay(),
            SettingsMsg::Ceiling => {
                if let Overlay::Settings(s) = &mut self.overlay {
                    s.session.finish_edit();
                    s.on_ceiling = true;
                }
            }
            SettingsMsg::CeilingApply => {
                if let Overlay::Settings(s) = &mut self.overlay {
                    s.on_ceiling = true;
                }
                self.apply_ceiling(out);
            }
            SettingsMsg::Display(row, d) => self.display_change(row, d, out),
            SettingsMsg::Connection(i) => {
                let a = match &mut self.overlay {
                    Overlay::Settings(s) => {
                        s.connection.focus = i;
                        s.connection.enter()
                    }
                    _ => None,
                };
                if let Some(a) = a {
                    self.connection_action(a, out);
                }
            }
        }
    }
}
