//! Keyboard input: the scoped key dispatch, typed text and the prompts it fills.

use super::*;

impl AppState {
    /// The wheel moves a list window's highlight; it never reaches what is behind it.
    pub(super) fn wheel(&mut self, rows: i32, keymap: &Keymap) {
        let scope = self.layout.focus.scope();
        match &mut self.overlay {
            Overlay::Palette(p) => {
                let n = p.entries(keymap, scope).len();
                p.move_by(rows, n);
            }
            Overlay::PaneMenu(m) => {
                let mut m = *m;
                let n = self.pane_candidates(m.pane).len();
                if n > 0 {
                    m.index = (m.index as i64 + i64::from(rows)).clamp(0, n as i64 - 1) as usize;
                    self.overlay = Overlay::PaneMenu(m);
                }
            }
            _ => {}
        }
    }

    /// Backspace in an open window: edits typed text, never deletes a measurement or a
    /// trace behind the window. Where nothing is typed it is Delete (Settings, the delete
    /// confirmation), as on keyboards without a Delete key.
    pub(super) fn backspace(&mut self, out: &mut Vec<Request>) {
        match &mut self.overlay {
            Overlay::Palette(p) => p.backspace(),
            Overlay::Form(f) => f.backspace(),
            Overlay::Settings(_) => self.settings_backspace(out),
            Overlay::Delete(_) => self.delete(true, out),
            // The safe answer, as N.
            Overlay::Offer(_) => self.offer(false, out),
            Overlay::NewLog(_) => self.new_log(false, out),
            Overlay::Prompt(p) => {
                p.text.pop();
                p.error = None;
            }
            _ => {}
        }
    }

    pub(super) fn key(&mut self, chord: Chord, keymap: &Keymap, out: &mut Vec<Request>) {
        use eframe::egui::Key;
        // An open window owns Backspace, as the app routes it (the help leaves the keys
        // working, so it is no such window).
        if chord.key == Key::Backspace
            && !(chord.command || chord.alt)
            && !(self.overlay == Overlay::None || self.overlay.is_reading())
        {
            self.swallow_text = None;
            self.backspace(out);
            return;
        }
        let swallow = self.swallow_text.take();
        // The stop that works from anywhere, before any window sees the key.
        if chord == STOP_ANYWHERE {
            self.command(CommandId::StopAnywhere, keymap, out);
            return;
        }
        // An open window owns Esc: it closes the topmost window only. With nothing open Esc
        // stops the stimulus and hands the keys back from a selected slot to the live
        // measurement.
        if chord == Chord::key(Key::Escape) {
            if self.overlay == Overlay::None {
                self.command(CommandId::StimulusStop, keymap, out);
                self.selected_trace = None;
            } else {
                self.close_overlay();
            }
            return;
        }
        let plain = !(chord.command || chord.alt);
        match &mut self.overlay {
            Overlay::Help | Overlay::Notifications => {
                let page = self.help_page.max(HELP_LINE);
                match chord.key {
                    Key::ArrowDown | Key::ArrowUp if plain => {
                        let d = if chord.key == Key::ArrowDown {
                            HELP_LINE
                        } else {
                            -HELP_LINE
                        };
                        self.help_scroll = (self.help_scroll + d).max(0.0);
                    }
                    Key::PageDown if plain => self.help_scroll += page,
                    Key::PageUp if plain => self.help_scroll = (self.help_scroll - page).max(0.0),
                    Key::Home if plain => self.help_scroll = 0.0,
                    // The view clamps it to the end.
                    Key::End if plain => self.help_scroll = f32::MAX,
                    Key::Enter => self.overlay = Overlay::None,
                    // Other keys keep working with the keys shown (try them while reading),
                    // except the stimulus's.
                    _ => self.overlay_fallthrough(chord, keymap, out),
                }
                return;
            }
            Overlay::Palette(p) => {
                let scope = self.layout.focus.scope();
                let n = p.entries(keymap, scope).len();
                let page = crate::palette::PALETTE_ROWS as i32;
                match chord.key {
                    Key::ArrowDown | Key::ArrowUp => {
                        p.move_by(if chord.key == Key::ArrowDown { 1 } else { -1 }, n);
                    }
                    Key::PageDown => p.move_by(page, n),
                    Key::PageUp => p.move_by(-page, n),
                    Key::Home => p.move_by(-(n as i32), n),
                    Key::End => p.move_by(n as i32, n),
                    Key::Enter => {
                        let c = p.chosen(keymap, scope);
                        self.overlay = Overlay::None;
                        if let Some(c) = c {
                            self.command(c, keymap, out);
                        }
                    }
                    _ if keymap.lookup(Scope::Global, chord) == Some(CommandId::Palette) => {
                        self.overlay = Overlay::None;
                    }
                    _ => self.swallow_text = swallow,
                }
                return;
            }
            Overlay::Prompt(_) => {
                if chord.key == Key::Enter {
                    self.apply_prompt(out);
                } else {
                    self.swallow_text = swallow;
                }
                return;
            }
            Overlay::Settings(_) => {
                self.settings_key(chord, swallow, keymap, out);
                return;
            }
            Overlay::Offer(_) => {
                if chord.key == Key::Enter {
                    self.offer(true, out);
                } else if chord.key == Key::N {
                    self.offer(false, out);
                }
                return;
            }
            Overlay::NewLog(_) => {
                if chord.key == Key::Enter {
                    self.new_log(true, out);
                } else if chord.key == Key::N {
                    self.new_log(false, out);
                }
                return;
            }
            Overlay::Choose(c) => {
                let n = c.choices.len();
                match chord.key {
                    Key::ArrowDown | Key::ArrowRight | Key::Tab if n > 0 => {
                        c.index = (c.index + 1).min(n - 1);
                    }
                    Key::ArrowUp | Key::ArrowLeft if n > 0 => c.index = c.index.saturating_sub(1),
                    Key::Enter => {
                        let i = c.index;
                        self.choose(Some(i), out);
                    }
                    _ => {}
                }
                return;
            }
            // Delete twice deletes, as in the calibrations view (Backspace is Delete here).
            Overlay::Delete(_) => {
                if matches!(chord.key, Key::Enter | Key::Delete) {
                    self.delete(true, out);
                } else if chord.key == Key::N {
                    self.delete(false, out);
                }
                return;
            }
            Overlay::Form(f) => {
                match chord.key {
                    Key::Enter => self.submit_form(out),
                    Key::ArrowUp => f.move_focus(-1),
                    Key::Tab if chord.shift => f.move_focus(-1),
                    Key::ArrowDown | Key::Tab => f.move_focus(1),
                    Key::ArrowLeft => f.cycle(-1),
                    Key::ArrowRight => f.cycle(1),
                    Key::A if chord.command => f.select_all(),
                    _ => self.swallow_text = swallow,
                }
                return;
            }
            Overlay::DelayPick(choice) => {
                let n = choice.rows().len();
                if matches!(chord.key, Key::ArrowDown | Key::ArrowUp) && plain && !chord.shift {
                    if n > 0 {
                        let d = if chord.key == Key::ArrowDown {
                            1
                        } else {
                            n - 1
                        };
                        choice.selected = (choice.selected + d) % n;
                    }
                    return;
                }
                let index = match chord {
                    c if c == Chord::key(Key::Num1) => Some(0u8),
                    c if c == Chord::key(Key::Num2) => Some(1),
                    c if c == Chord::key(Key::Num3) => Some(2),
                    c if c == Chord::key(Key::Enter) => u8::try_from(choice.selected).ok(),
                    _ => None,
                };
                if let Some(index) = index
                    && usize::from(index) < n
                {
                    let (meas, name) = (choice.meas, choice.name.clone());
                    self.overlay = Overlay::None;
                    self.call(
                        out,
                        Command::DelayInsert {
                            meas,
                            pick: DelayPick::Ranked { index },
                        },
                        format!("{name}: candidate {} inserted", index + 1),
                    );
                    return;
                }
                // The list stays up over the plots and the other keys keep working, except
                // the stimulus's.
                self.overlay_fallthrough(chord, keymap, out);
                return;
            }
            Overlay::PaneMenu(menu) => {
                let mut menu = *menu;
                let n = self.pane_candidates(menu.pane).len();
                match chord.key {
                    Key::ArrowDown | Key::ArrowUp if n > 0 => {
                        let d = if chord.key == Key::ArrowDown {
                            1
                        } else {
                            n - 1
                        };
                        menu.index = (menu.index + d) % n;
                        self.overlay = Overlay::PaneMenu(menu);
                    }
                    Key::Home | Key::PageUp => {
                        menu.index = 0;
                        self.overlay = Overlay::PaneMenu(menu);
                    }
                    Key::End | Key::PageDown => {
                        menu.index = n.saturating_sub(1);
                        self.overlay = Overlay::PaneMenu(menu);
                    }
                    Key::Enter => {
                        self.overlay = Overlay::None;
                        let id = self
                            .pane_candidates(menu.pane)
                            .get(menu.index)
                            .map(|m| m.id);
                        if let Some(id) = id {
                            self.pane_show(menu.pane, id);
                        }
                    }
                    _ => {}
                }
                return;
            }
            Overlay::None => {}
        }
        if let Some(c) = keymap.lookup(self.scope(), chord) {
            self.run_key_command(c, chord, keymap, out);
        }
    }

    /// A key a window that stays out of the way (help, the delay candidates) does not use
    /// runs its command, unless it is the stimulus's: with a window open the stimulus keys
    /// never act.
    pub(super) fn overlay_fallthrough(
        &mut self,
        chord: Chord,
        keymap: &Keymap,
        out: &mut Vec<Request>,
    ) {
        let Some(c) = keymap.lookup(self.scope(), chord) else {
            return;
        };
        if RESERVED.iter().any(|(_, id)| *id == c) || c == CommandId::StimulusTakeOver {
            return;
        }
        self.run_key_command(c, chord, keymap, out);
    }

    /// Runs the command of a key; a text window it opens does not also type the key.
    pub(super) fn run_key_command(
        &mut self,
        c: CommandId,
        chord: Chord,
        keymap: &Keymap,
        out: &mut Vec<Request>,
    ) {
        let before = std::mem::discriminant(&self.overlay);
        self.command(c, keymap, out);
        let opened_text = matches!(
            self.overlay,
            Overlay::Palette(_) | Overlay::Prompt(_) | Overlay::Form(_) | Overlay::Settings(_)
        );
        if opened_text && std::mem::discriminant(&self.overlay) != before {
            self.swallow_text = typed_char(&chord);
        }
    }

    /// Esc with a window open: the topmost window closes (a dialog over a view closes back
    /// to the view) and the stimulus is left alone: no dialog arms one.
    pub(super) fn close_overlay(&mut self) {
        match &mut self.overlay {
            Overlay::Settings(s) if s.inner_open() => {
                s.close_inner();
            }
            _ => self.overlay = Overlay::None,
        }
    }

    pub(super) fn text(&mut self, t: &str) {
        let mut t = t.to_string();
        if let Some(c) = self.swallow_text.take() {
            let mut chars = t.chars();
            if chars
                .next()
                .is_some_and(|first| first.to_ascii_lowercase() == c)
            {
                t = chars.collect();
            }
        }
        match &mut self.overlay {
            Overlay::Palette(p) => p.type_text(&t),
            Overlay::Form(f) => f.type_text(&t),
            Overlay::Settings(_) => self.settings_text(&t),
            Overlay::Prompt(p) => {
                p.text.push_str(&t);
                p.error = None;
            }
            _ => {}
        }
    }

    pub(super) fn prompt(&mut self, kind: PromptKind, text: String) {
        self.overlay = Overlay::Prompt(Prompt {
            kind,
            text,
            error: None,
        });
    }

    pub(super) fn apply_prompt(&mut self, out: &mut Vec<Request>) {
        let Overlay::Prompt(p) = &self.overlay else {
            return;
        };
        let kind = p.kind;
        let text = p.text.clone();
        let r = match kind {
            PromptKind::StimulusLevel => self.set_level_text(&text, out),
            PromptKind::CalDelete => self.cal_delete(&text, out),
            PromptKind::TraceMicCurve(id) => {
                let mics = self.daemon().map(|s| s.mics.clone()).unwrap_or_default();
                trace_mic_request(&mics, id, &text).map(|r| out.push(r))
            }
            PromptKind::MicCurveInput => parse_input_curve(&text).and_then(|(channel, curve)| {
                let mut row = self.input_setup(channel);
                let Some(mic) = row.mic.clone() else {
                    return Err(format!(
                        "input {} has no mic name: name it first (palette: Input setup…)",
                        u32::from(channel) + 1
                    ));
                };
                if let CurveChoice::Curve { label } = &curve
                    && self
                        .daemon()
                        .is_none_or(|s| ac2_proto::cal::curve(&s.mics, &mic, label).is_none())
                {
                    let stored = self
                        .daemon()
                        .and_then(|s| ac2_proto::cal::mic(&s.mics, &mic))
                        .map_or_else(|| "none".to_owned(), |m| ac2_scene::cal::labels(&m.curves));
                    return Err(format!("{mic} has no curve {label:?} (stored: {stored})"));
                }
                row.curve = curve;
                let what = curve_what(&row);
                out.push(Request::Call {
                    what,
                    cmd: Command::SessionInputs { inputs: vec![row] },
                });
                Ok(())
            }),
            PromptKind::FinderBand => parse_band(&text).map(|(lo, hi)| {
                self.set_finder(FinderChoice {
                    band: FinderBand::Custom {
                        lo_hz: Hz(lo),
                        hi_hz: Hz(hi),
                    },
                    observation: None,
                });
            }),
            PromptKind::FinderObservation => self.set_observation(&text),
            PromptKind::Offset(id) => {
                parse_number(&text, &["db"]).map(|v| self.set_live_offset(id, v))
            }
            PromptKind::TraceOffset(id) => parse_number(&text, &["db"]).and_then(|v| {
                let t = self.trace_meta(id)?;
                let mut edit = t.edit.clone();
                edit.offset = Db(v);
                let what = format!("{}: offset {}", trace_label(&t), format::db_readout(v));
                out.push(Request::Call {
                    cmd: Command::TraceUpdate { trace: id, edit },
                    what,
                });
                Ok(())
            }),
            PromptKind::TraceRename(id) => {
                let name = text.trim();
                if name.is_empty() {
                    Err("type a name".to_string())
                } else {
                    self.trace_meta(id).map(|t| {
                        let mut edit = t.edit.clone();
                        edit.name = name.to_string();
                        let what = format!("{} renamed to {name}", t.edit.name);
                        out.push(Request::Call {
                            cmd: Command::TraceUpdate { trace: id, edit },
                            what,
                        });
                    })
                }
            }
            PromptKind::TraceExport(id) => {
                let path = text.trim();
                if path.is_empty() {
                    Err("type a file or folder path".to_string())
                } else {
                    self.trace_meta(id).map(|t| {
                        let path = std::path::PathBuf::from(path);
                        let path = match &self.export_dir {
                            Some(d) if path.is_relative() => d.join(path),
                            _ => path,
                        };
                        // The next export starts in the same folder.
                        self.export_dir = if path.is_dir() {
                            Some(path.clone())
                        } else {
                            path.parent()
                                .filter(|p| !p.as_os_str().is_empty())
                                .map(std::path::Path::to_path_buf)
                        };
                        out.push(Request::ExportTrace {
                            trace: id,
                            name: t.edit.name.clone(),
                            path,
                        });
                    })
                }
            }
            PromptKind::TraceSlot(id) => parse_slot(&text).and_then(|slot| {
                let t = self.trace_meta(id)?;
                let mut edit = t.edit.clone();
                edit.slot = slot;
                let what = match slot {
                    Some(n) => format!("{} in slot {n}", t.edit.name),
                    None => format!("{}: slot freed", t.edit.name),
                };
                out.push(Request::Call {
                    cmd: Command::TraceUpdate { trace: id, edit },
                    what,
                });
                Ok(())
            }),
            PromptKind::ImportFile(role) => {
                let path = text.trim();
                if path.is_empty() {
                    Err("type a file path".to_string())
                } else {
                    out.push(Request::Import {
                        path: std::path::PathBuf::from(path),
                        role,
                    });
                    Ok(())
                }
            }
            PromptKind::SessionSave | PromptKind::SessionLoad => {
                parse_session_ref(&text).map(|session| {
                    let load = kind == PromptKind::SessionLoad;
                    let what = match &session {
                        SessionRef::Name { name } => name.clone(),
                        SessionRef::Path { path } => path.clone(),
                    };
                    let (cmd, what) = if load {
                        (
                            Command::FileLoad { session },
                            format!("session {what:?} loaded (disarmed)"),
                        )
                    } else {
                        (
                            Command::FileSave { session },
                            format!("session {what:?} saved"),
                        )
                    };
                    out.push(Request::Call { cmd, what });
                })
            }
            PromptKind::ReplayRecording => parse_session_ref(&text).map(|r| {
                let (recording, what) = match r {
                    SessionRef::Name { name } => (
                        ac2_proto::model::RecordingRef::Name { name: name.clone() },
                        name,
                    ),
                    SessionRef::Path { path } => (
                        ac2_proto::model::RecordingRef::Path { path: path.clone() },
                        path,
                    ),
                };
                out.push(Request::Call {
                    cmd: Command::SessionReplay {
                        recording,
                        pace: ac2_proto::model::ReplayPace::Realtime,
                    },
                    what: format!("replaying {what:?}"),
                });
            }),
            PromptKind::InputMics => parse_mics(&text).map(|mics| {
                let inputs: Vec<InputSetup> = mics
                    .into_iter()
                    .map(|(channel, mic)| {
                        let mut row = self.input_setup(channel);
                        if row.mic != mic {
                            row.mic = mic;
                            // Another capsule: its own curves, chosen again.
                            row.curve = CurveChoice::NotChosen;
                        }
                        row
                    })
                    .collect();
                out.push(Request::Call {
                    what: format!("input setup ({})", mics_text(&inputs)),
                    cmd: Command::SessionInputs { inputs },
                });
            }),
            PromptKind::Delay(id) => parse_number(&text, &["ms"]).and_then(|v| {
                if !(0.0..=10_000.0).contains(&v) {
                    return Err("delay must be 0 … 10000 ms".to_string());
                }
                out.push(Request::Call {
                    cmd: Command::DelaySet {
                        meas: id,
                        delay: Seconds(v / 1000.0),
                    },
                    what: format!("delay {}", format::delay(v / 1000.0)),
                });
                Ok(())
            }),
        };
        match r {
            Ok(()) => self.overlay = Overlay::None,
            Err(e) => {
                if let Overlay::Prompt(p) = &mut self.overlay {
                    p.error = Some(e);
                }
            }
        }
    }
}
