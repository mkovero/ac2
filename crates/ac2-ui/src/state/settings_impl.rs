//! The reducer's side of the Settings view ([`crate::settings`]): opening it at a page, its
//! keys, typed text and mouse, and the requests its pages make.

use ac2_proto::Command;
use ac2_proto::model::OutputSetup;
use ac2_scene::view::{CoherencePlacement, DistortionUnit, LeqStyle};
use eframe::egui::Key;

use super::{AppState, Overlay, PaneKind, StimPhase, typed_char};
use crate::cal_view::CalView;
use crate::conn::Request;
use crate::keys::{Chord, CommandId, Keymap, Scope};
use crate::leq_dialog::{Focus, LeqDialog};
use crate::session_dialog::{Edit, RoleKey, Row, SessionDialog};
use crate::settings::{
    ConnAction, DisplayRow, Page, RecordingRow, RigPage, RigRow, SERVER_INFO_EVERY_S, Settings,
    step_hold, step_theme,
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
    /// A row of the Rig monitor page focused.
    RigFocus(RigRow),
    /// A click on a text row of the Rig monitor page: focused, the caret before its
    /// character `usize`.
    RigCaret(RigRow, usize),
    /// A row of the Rig monitor page acted on (Enter).
    RigApply(RigRow),
    /// Monitor at start set.
    RigMonitor(bool),
}

impl AppState {
    /// Opens Settings at `page` (or shows `page` of the open view).
    pub(super) fn open_settings(&mut self, page: Page, out: &mut Vec<Request>) {
        self.settings_page_last = page;
        self.settings_reveal = true;
        if let Overlay::Settings(s) = &mut self.overlay {
            s.show(page);
            self.settings_entered(out);
            return;
        }
        let open = self.open_session().cloned();
        let setup = self
            .daemon()
            .map(|s| ac2_proto::model::InputSetups {
                inputs: s.inputs.clone(),
                pairs: s.pairs.clone(),
            })
            .unwrap_or_default();
        let labels = self.daemon().map(|s| s.outputs.clone()).unwrap_or_default();
        let mut session = SessionDialog::new(open.as_ref(), &setup);
        session.set_labels(&labels);
        if let Some(d) = &self.rig.devices {
            session.set_backends(d.clone(), self.prefs.of_rig(self.rig.id));
        }
        // The calibrations start on the input the selected measurement listens on.
        let on = self
            .selected_meas()
            .and_then(|m| super::meas_input(&m.config.kind));
        let cal = self
            .daemon()
            .map_or_else(CalView::default, |s| CalView::new(s, on));
        let leq = self.kind_meas(PaneKind::Spl).cloned().and_then(|m| {
            let calibrated = self.leq_calibrated(m.id);
            LeqDialog::new(&m, calibrated, self.leq_panes.cues_shown)
        });
        let mut settings = Settings::new(page, session, cal, leq);
        settings.rig = RigPage::new(self.daemon().map(|s| &s.rig));
        self.overlay = Overlay::Settings(Box::new(settings));
        if self.connected() {
            out.push(Request::Devices);
        }
        self.settings_entered(out);
    }

    /// Settings on the Audio I/O page focused on the input device: where a fresh start
    /// on a daemon without an audio I/O, a new session and the open-session command
    /// land.
    pub fn open_audio_setup(&mut self, out: &mut Vec<Request>) {
        self.open_settings(Page::AudioIo, out);
        if let Overlay::Settings(s) = &mut self.overlay {
            s.focus_want = None;
            s.focus_session_row(crate::session_dialog::Row::Device);
        }
    }

    /// A page was shown: it asks for what it shows.
    fn settings_entered(&mut self, out: &mut Vec<Request>) {
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        if matches!(s.page, Page::Connection | Page::Recording)
            && matches!(self.rig.conn, super::ConnState::Connected { .. })
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

    /// The pages a mirror change reaches: the rig's output labels, the Rig monitor page.
    pub(super) fn follow_settings(&mut self) {
        let labels = self.daemon().map(|s| s.outputs.clone()).unwrap_or_default();
        let rig = self.daemon().map(|s| s.rig.clone());
        if let Overlay::Settings(s) = &mut self.overlay {
            s.session.set_labels(&labels);
            if let Some(r) = &rig {
                s.rig.follow(r);
            }
        }
        // A page kept after a conflict follows the meter's configuration until applied.
        let fresh = self
            .overlay
            .leq()
            .filter(|d| d.notice.is_some())
            .and_then(|d| self.meas(d.meas).cloned());
        if let (Some(m), Some(d)) = (fresh, self.overlay.leq_mut()) {
            d.rebase(&m);
        }
    }

    /// The daemon refused `what` as made on settings another client changed first. The SPL
    /// / Leq page or the Rig monitor target it came from comes back with the operator's
    /// edits, made on the newer settings so Apply again overwrites; otherwise the Rig
    /// monitor page shows the rig's settings again. A toast says which, even with the
    /// warning toasts off: the edit did not land.
    pub(super) fn conflict(&mut self, what: &str, out: &mut Vec<Request>) {
        let rig = self.daemon().map(|s| s.rig.clone());
        let mut kept = self.leq_conflict(what, out);
        if let (Overlay::Settings(s), Some(r)) = (&mut self.overlay, &rig)
            && !kept
        {
            kept = s.rig.rebase(what, r);
            if !kept {
                s.rig.follow(r);
            }
        }
        let text = if kept {
            ac2_scene::osc::conflict_kept_toast(what)
        } else {
            ac2_scene::osc::conflict_toast(what)
        };
        self.notify(super::Severity::Warning, text, super::Muted::Never);
    }

    /// The SPL / Leq page whose Apply `what` was refused: open again with its edits, on the
    /// meter's current configuration. False when no such Apply is waiting, or another
    /// window is up.
    fn leq_conflict(&mut self, what: &str, out: &mut Vec<Request>) -> bool {
        let Some((_, d)) = self.rig.leq.sent.take_if(|(w, _)| w == what) else {
            return false;
        };
        let Some(m) = self.meas(d.meas).cloned() else {
            return false;
        };
        if !matches!(self.overlay, Overlay::None | Overlay::Settings(_)) {
            return false;
        }
        let mut d = *d;
        d.rebase(&m);
        self.open_settings(Page::Leq, out);
        if let Overlay::Settings(s) = &mut self.overlay {
            s.leq = Some(d);
        }
        true
    }

    /// A reply's warnings, the rig meters by name: shown even with the warning toasts off,
    /// as the meters going blind or moving is what the operator did not ask for.
    pub(super) fn reply_warnings(&mut self, warnings: &[ac2_proto::Warning]) {
        for w in warnings {
            let names: Vec<String> = ac2_scene::osc::warning_meas(w)
                .iter()
                .map(|id| {
                    self.meas(*id)
                        .map_or_else(|| id.0.to_string(), |m| m.config.name.clone())
                })
                .collect();
            self.notify(
                super::Severity::Warning,
                ac2_scene::osc::reply_warning(w, &names),
                super::Muted::Never,
            );
        }
    }

    /// Enter (or a click) on a row of the Rig monitor page.
    fn rig_apply(&mut self, row: RigRow, out: &mut Vec<Request>) {
        let rig = self.daemon().map(|s| s.rig.clone()).unwrap_or_default();
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        s.rig.focus_row(row);
        if let Some((cmd, what)) = s.rig.enter(&rig) {
            self.call(out, cmd, what);
        }
    }

    fn rig_monitor(&mut self, on: bool, out: &mut Vec<Request>) {
        let rig = self.daemon().map(|s| s.rig.clone()).unwrap_or_default();
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        if let Some((cmd, what)) = s.rig.monitor(&rig, on) {
            self.call(out, cmd, what);
        }
    }

    fn rig_levels(&mut self, row: RigRow, d: i32, out: &mut Vec<Request>) {
        let rig = self.daemon().map(|s| s.rig.clone()).unwrap_or_default();
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        if let Some((cmd, what)) = s.rig.levels_step(&rig, row, d) {
            self.call(out, cmd, what);
        }
    }

    fn rig_key(&mut self, chord: Chord, swallow: Option<char>, out: &mut Vec<Request>) {
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        let focus = s.rig.focus;
        match chord.key {
            Key::ArrowUp => s.rig.move_row(-1),
            Key::Tab if chord.shift => s.rig.move_row(-1),
            Key::ArrowDown | Key::Tab => s.rig.move_row(1),
            Key::Enter => self.rig_apply(focus, out),
            Key::ArrowLeft if focus == RigRow::Monitor => self.rig_monitor(false, out),
            Key::ArrowRight if focus == RigRow::Monitor => self.rig_monitor(true, out),
            Key::ArrowLeft if focus.is_levels() => self.rig_levels(focus, -1, out),
            Key::ArrowRight if focus.is_levels() => self.rig_levels(focus, 1, out),
            Key::ArrowLeft => s.rig.caret_step(-1),
            Key::ArrowRight => s.rig.caret_step(1),
            Key::Home => s.rig.caret_edge(false),
            Key::End => s.rig.caret_edge(true),
            Key::Delete => s.rig.delete_char(),
            _ => self.swallow_text = swallow,
        }
    }

    fn settings_page(&mut self, page: Page, out: &mut Vec<Request>) {
        self.settings_page_last = page;
        self.settings_reveal = true;
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
        if !matches!(self.overlay, Overlay::Settings(_)) {
            return;
        }
        self.settings_reveal = true;
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        let page = s.page;
        // Pages: Ctrl+PageUp / PageDown and Alt+← / → step, Alt+digit jumps. No Ctrl+Tab:
        // desktops take it for themselves.
        let step = match chord.key {
            Key::PageUp if chord.command && !chord.alt => Some(-1),
            Key::PageDown if chord.command && !chord.alt => Some(1),
            Key::ArrowLeft if chord.alt && !chord.command && !chord.shift => Some(-1),
            Key::ArrowRight if chord.alt && !chord.command && !chord.shift => Some(1),
            _ => None,
        };
        if let Some(d) = step {
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
            if let Some(&p) = n.and_then(|n| Page::ALL.get(n)) {
                self.settings_page(p, out);
                return;
            }
        }
        // The Settings key itself closes the view, as the palette's key closes the palette.
        if !s.typing() && keymap.lookup(Scope::Global, chord) == Some(CommandId::Settings) {
            self.overlay = Overlay::None;
            return;
        }
        match page {
            Page::AudioIo => self.session_key(chord, swallow, out),
            Page::Calibration => self.cal_view_key(chord, swallow, out),
            Page::Leq => self.leq_key(chord, swallow, out),
            Page::Recording => self.recording_key(chord, swallow),
            Page::Display => self.display_key(chord),
            Page::Connection => self.connection_key(chord, swallow, out),
            Page::Rig => self.rig_key(chord, swallow, out),
        }
    }

    fn leq_key(&mut self, chord: Chord, swallow: Option<char>, out: &mut Vec<Request>) {
        let Some(d) = self.overlay.leq_mut() else {
            self.swallow_text = swallow;
            return;
        };
        if d.transfer.is_some() {
            self.band_transfer_key(chord, out);
            return;
        }
        let plain = !(chord.command || chord.alt || chord.shift);
        match chord.key {
            Key::T if plain && matches!(d.focus, Focus::Band(_)) => self.open_band_transfer(),
            // On the range row Enter adds its bands; the dialog is sent from elsewhere.
            Key::Enter if d.on_range() => d.add_range(),
            Key::Enter => self.submit_leq(out),
            Key::ArrowUp => d.move_row(-1),
            Key::ArrowDown => d.move_row(1),
            Key::Tab if chord.shift => d.move_cell(-1),
            Key::Tab => d.move_cell(1),
            // A cue line is edited in place: ←/→, Home/End and Delete work on its text.
            Key::ArrowLeft if d.on_caret_text() => d.caret_step(-1),
            Key::ArrowRight if d.on_caret_text() => d.caret_step(1),
            Key::Home if d.on_caret_text() => d.caret_edge(false),
            Key::End if d.on_caret_text() => d.caret_edge(true),
            Key::Delete if d.on_caret_text() => d.delete_char(),
            Key::ArrowLeft => d.cycle(-1),
            Key::ArrowRight => d.cycle(1),
            Key::Insert if chord.shift => d.open_range(),
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
            // Two rows: back (↑, Shift+Tab) and on (↓, Tab) both reach the other one.
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

    fn display_key(&mut self, chord: Chord) {
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        let row = s.display;
        match chord.key {
            Key::ArrowUp => s.display_move(-1),
            Key::Tab if chord.shift => s.display_move(-1),
            Key::ArrowDown | Key::Tab => s.display_move(1),
            Key::ArrowLeft => self.display_change(row, -1),
            Key::ArrowRight | Key::Space => self.display_change(row, 1),
            Key::Enter => self.display_change(row, 0),
            _ => {}
        }
    }

    /// ←/→ (`d`) or Enter (`0`) on a line of the Display page: changes it at once.
    fn display_change(&mut self, row: DisplayRow, d: i32) {
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
            DisplayRow::WarningToasts if d != 0 => self.toggle_warning_toasts(),
            DisplayRow::SplHold if d != 0 => {
                self.prefs.spl_hold_ms = step_hold(self.prefs.spl_hold_ms, d);
                self.prefs_dirty = true;
            }
            DisplayRow::EmberDecay if d != 0 => {
                self.prefs.ember_decay_s =
                    crate::settings::step_ember_decay(self.prefs.ember_decay_s, d);
                self.prefs_dirty = true;
            }
            DisplayRow::EmberGlow if d != 0 => {
                self.prefs.ember_glow = crate::settings::step_ember_glow(self.prefs.ember_glow, d);
                self.prefs_dirty = true;
            }
            // The G keys of the pane of that kind worked in last, so the spectrograph's
            // history comes and goes as it does there; the focus stays where it was.
            DisplayRow::SpectrumView | DisplayRow::SweepView if d != 0 => {
                let kind = if row == DisplayRow::SpectrumView {
                    PaneKind::Spectrum
                } else {
                    PaneKind::Distortion
                };
                let Some(pane) = self.lead(kind) else {
                    self.warn(format!(
                        "no {} pane: a {} measurement selected in the list shows one in the \
                         focused pane",
                        kind.title(),
                        kind.what()
                    ));
                    return;
                };
                let focus = self.layout.focus;
                self.layout.set_focus(pane);
                self.step_view(d > 0);
                self.layout.set_focus(focus);
            }
            // The palette's commands without focusing a pane: the page stays in front.
            DisplayRow::LeqStyle if d != 0 => self.toggle_leq_layout(false),
            DisplayRow::LeqHistory if d != 0 => self.toggle_leq_layout(true),
            DisplayRow::LeqBars if d != 0 => self.toggle_bar_ease(),
            DisplayRow::LeqScale if d != 0 => self.toggle_shared_scale(),
            DisplayRow::SpectrumGlide if d != 0 => self.toggle_spec_ease(),
            DisplayRow::DistortionUnit if d != 0 => self.toggle_distortion_unit(),
            DisplayRow::Coherence if d != 0 => self.toggle_coherence_placement(),
            DisplayRow::ResolutionMarker if d != 0 => self.toggle_resolution_marker(),
            DisplayRow::RangeBand if d != 0 => self.toggle_range_band(),
            DisplayRow::PlateBackground if d != 0 => self.toggle_plate_background(),
            DisplayRow::CursorJump if d != 0 => {
                let db = ac2_scene::cursor_jump::step_prominence(self.view.jump_prominence_db, d);
                self.view.jump_prominence_db = db;
                self.prefs.jump_prominence_db = db;
                self.prefs_dirty = true;
            }
            DisplayRow::Advice
            | DisplayRow::AdviceRefHeadroom
            | DisplayRow::AdviceLowCoherence
            | DisplayRow::AdviceLowLevel
                if d != 0 =>
            {
                self.step_advice(row, d);
            }
            DisplayRow::LevelAxes if d == 0 => {
                self.pane_levels.clear();
                self.level_set.clear();
                self.hold_all_levels();
                crate::prefs::IrLevelPrefs::default().apply(&mut self.view);
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
            Key::Tab if chord.shift => {
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

    /// Keys of the Audio I/O page (the session model and the max level row). Esc never gets
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
        if s.on_ceiling {
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
                } else if matches!(d.edit, Some(Edit::Mic(_) | Edit::PairName(_))) {
                    d.finish_edit();
                } else {
                    self.submit_session(out);
                }
            }
            Key::ArrowUp | Key::ArrowDown | Key::Tab => {
                let back = chord.key == Key::ArrowUp || (chord.key == Key::Tab && chord.shift);
                s.io_move(if back { -1 } else { 1 });
            }
            Key::ArrowLeft | Key::ArrowRight if d.edit.is_none() => {
                let forward = chord.key == Key::ArrowRight;
                if matches!(d.focus, Row::Input(_)) {
                    let st = self.rig.mirror.as_ref().and_then(|m| m.state.clone());
                    let mics = st.as_ref().map(|s| s.mics.as_slice()).unwrap_or_default();
                    let live = st.as_ref().map(|s| s.inputs.as_slice()).unwrap_or_default();
                    if let Some(row) = d.step_curve(forward, mics, live) {
                        // The session already captures this mic: the choice applies now.
                        let what = super::curve_what(&row);
                        out.push(Request::Call {
                            what,
                            cmd: self.inputs_cmd(vec![row]),
                        });
                    }
                } else {
                    d.cycle(if forward { 1 } else { -1 }, self.prefs.of_rig(self.rig.id));
                }
            }
            // Everything below types text while a text row or edit has the keyboard.
            _ if d.text_focus() => self.swallow_text = swallow,
            Key::Space if plain => d.toggle(),
            Key::R if plain => d.assign(RoleKey::Reference),
            Key::M if plain => d.assign(RoleKey::Mic),
            Key::L if plain => d.assign(RoleKey::Line),
            Key::A if plain => d.assign(RoleKey::Digital),
            Key::P if plain && chord.shift => d.assign(RoleKey::Pair),
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
            Key::P if plain => self.session_msg(super::SessionMsg::Panel, out),
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
        let device = d.output_info().map(|x| x.id.0.clone());
        let Some(n) = open_outputs.filter(|_| d.is_open_output()) else {
            d.notice = Some("the stimulus outputs apply when the audio I/O opens (Enter)".into());
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
            self.prefs
                .outputs
                .insert((self.rig.id, dev.clone()), ticked.clone());
            self.prefs_dirty = true;
            if self.rig.stimulus.phase == StimPhase::Idle
                || matches!(
                    self.rig.stimulus.phase,
                    StimPhase::Armed | StimPhase::Firing
                )
            {
                self.rig.stimulus.outputs = ticked;
                self.rig.stim_device = Some(dev);
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
            Page::AudioIo if s.on_ceiling => s.ceiling.type_text(t),
            Page::AudioIo => s.session.type_text(t),
            Page::Calibration => s.cal.type_text(t),
            Page::Leq => {
                // The band transfer step takes keys, not text, but for the place's name.
                if let Some(d) = &mut s.leq {
                    match &mut d.transfer {
                        Some(step) => step.type_place(t),
                        None => d.type_text(t),
                    }
                }
            }
            Page::Recording => {
                if s.recording.focus == RecordingRow::Limit {
                    s.recording.text.push_str(t);
                    s.recording.error = None;
                }
            }
            Page::Connection => s.connection.type_text(t),
            Page::Rig => s.rig.type_text(t),
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
            Page::AudioIo if s.on_ceiling => s.ceiling.backspace(),
            Page::AudioIo => s.session.backspace(),
            Page::Calibration if s.cal.typing() => s.cal.backspace(),
            Page::Calibration => self.cal_view_key(Chord::key(Key::Delete), None, out),
            Page::Leq if s.leq.as_ref().is_some_and(|d| d.transfer.is_some()) => {
                match s.leq.as_mut().and_then(|d| d.transfer.as_mut()) {
                    Some(step) if step.on_place => step.backspace_place(),
                    _ => self.band_transfer_key(Chord::key(Key::Delete), out),
                }
            }
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
            Page::Rig => s.rig.backspace(),
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
            SettingsMsg::Display(row, d) => self.display_change(row, d),
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
            SettingsMsg::RigFocus(row) => {
                if let Overlay::Settings(s) = &mut self.overlay {
                    s.rig.focus_row(row);
                }
            }
            SettingsMsg::RigCaret(row, n) => {
                if let Overlay::Settings(s) = &mut self.overlay {
                    s.rig.caret_at(row, n);
                }
            }
            SettingsMsg::RigApply(row) => self.rig_apply(row, out),
            SettingsMsg::RigMonitor(on) => self.rig_monitor(on, out),
        }
    }
}

impl AppState {
    /// The Display setting and its palette command. Its own toast is information, so it
    /// shows either way.
    pub(super) fn toggle_warning_toasts(&mut self) {
        self.prefs.warning_toasts = !self.prefs.warning_toasts;
        self.prefs_dirty = true;
        self.toast(format!(
            "warning toasts {}",
            crate::settings::warning_toasts_text(self.prefs.warning_toasts)
        ));
    }

    /// The Display page's spectrum glide line; remembered in the prefs.
    pub(super) fn toggle_spec_ease(&mut self) {
        self.prefs.spec_ease = !self.prefs.spec_ease;
        self.prefs_dirty = true;
        self.toast(format!(
            "Spectrum {}",
            crate::settings::spec_ease_text(self.prefs.spec_ease)
        ));
    }

    /// The Display page's Leq bars line; remembered in the prefs.
    pub(super) fn toggle_bar_ease(&mut self) {
        self.prefs.bar_ease = !self.prefs.bar_ease;
        self.prefs_dirty = true;
        self.toast(format!(
            "Leq bars {}",
            crate::settings::bar_ease_text(self.prefs.bar_ease)
        ));
    }

    /// The Display page's Leq scale line; remembered in the prefs.
    pub(super) fn toggle_shared_scale(&mut self) {
        self.prefs.shared_scale = !self.prefs.shared_scale;
        self.prefs_dirty = true;
        self.toast(format!(
            "Leq scale: {}",
            crate::settings::shared_scale_text(self.prefs.shared_scale)
        ));
    }

    /// The Display page's Resolution marker line; remembered in the prefs.
    pub(super) fn toggle_resolution_marker(&mut self) {
        self.view.unresolved = !self.view.unresolved;
        self.prefs.resolution_marker = self.view.unresolved;
        self.prefs_dirty = true;
        self.toast(format!(
            "resolution marker {}",
            crate::settings::resolution_marker_text(self.view.unresolved)
        ));
    }

    /// The Display page's Range across positions line; remembered in the prefs.
    pub(super) fn toggle_range_band(&mut self) {
        self.view.range_band = !self.view.range_band;
        self.prefs.range_band = self.view.range_band;
        self.prefs_dirty = true;
        self.toast(format!(
            "range across positions {}",
            crate::settings::range_band_text(self.view.range_band)
        ));
    }

    /// The Display page's Plate background line, for every plate of every pane; remembered
    /// in the prefs.
    pub(super) fn toggle_plate_background(&mut self) {
        self.prefs.plate_background = self.prefs.plate_background.toggled();
        self.prefs_dirty = true;
        self.toast(format!(
            "plate background {}",
            crate::settings::plate_background_text(self.prefs.plate_background)
        ));
    }

    /// The Display page's headroom advice lines; remembered in the prefs. A new threshold
    /// starts every measurement's advice afresh: what was shown was judged on the old one.
    pub(super) fn step_advice(&mut self, row: DisplayRow, d: i32) {
        use ac2_scene::advice::{
            LOW_COHERENCE, LOW_LEVEL_DBFS, REF_HEADROOM_DB, low_coherence_text, low_level_text,
            on_text, ref_headroom_text, step_choice,
        };
        let a = &mut self.prefs.advice;
        let said = match row {
            DisplayRow::Advice => {
                a.on = !a.on;
                format!("headroom advice {}", on_text(a.on))
            }
            DisplayRow::AdviceRefHeadroom => {
                a.ref_headroom_db = step_choice(&REF_HEADROOM_DB, a.ref_headroom_db, d);
                format!(
                    "advise lowering at {}",
                    ref_headroom_text(a.ref_headroom_db)
                )
            }
            DisplayRow::AdviceLowCoherence => {
                a.low_coherence = step_choice(&LOW_COHERENCE, a.low_coherence, d);
                format!("advise raising at {}", low_coherence_text(a.low_coherence))
            }
            DisplayRow::AdviceLowLevel => {
                a.low_level_dbfs = step_choice(&LOW_LEVEL_DBFS, a.low_level_dbfs, d);
                format!(
                    "advise raising with the {}",
                    low_level_text(a.low_level_dbfs)
                )
            }
            _ => return,
        };
        self.rig.advice.clear();
        self.prefs_dirty = true;
        self.toast(said);
    }

    /// Columns ↔ tiles, or the history strip on / off; remembered in the prefs.
    pub(super) fn toggle_leq_layout(&mut self, history: bool) {
        let l = &mut self.view.spl.layout;
        if history {
            l.history = !l.history;
        } else {
            l.style = match l.style {
                LeqStyle::Columns => LeqStyle::Tiles,
                LeqStyle::Tiles => LeqStyle::Columns,
            };
        }
        self.prefs.leq = self.view.spl.layout;
        self.prefs_dirty = true;
    }

    pub(super) fn toggle_distortion_unit(&mut self) {
        let u = &mut self.view.distortion.unit;
        *u = match *u {
            DistortionUnit::Db => DistortionUnit::Percent,
            DistortionUnit::Percent => DistortionUnit::Db,
        };
    }

    pub(super) fn toggle_coherence_placement(&mut self) {
        let c = &mut self.view.tf.coherence_placement;
        *c = match *c {
            CoherencePlacement::Pane => CoherencePlacement::OverlayOnMagnitude,
            CoherencePlacement::OverlayOnMagnitude => CoherencePlacement::Pane,
        };
    }
}
