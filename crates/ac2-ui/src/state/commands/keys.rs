//! Keyboard input: the scoped key dispatch, typed text and the prompts it fills.

use crate::state::*;

impl AppState {
    /// The wheel moves a list window's highlight; it never reaches what is behind it.
    pub(in crate::state) fn wheel(&mut self, rows: i32, keymap: &Keymap) {
        let scope = self.layout.focused().scope();
        match &mut self.overlay {
            Overlay::Palette(p) => {
                let n = p.entries(keymap, scope).len();
                p.move_by(rows, n);
            }
            Overlay::RigMenu(m) => {
                let m = *m;
                self.rig_menu_wheel(m, rows);
            }
            Overlay::PaneMenu(m) => {
                let mut m = *m;
                let list = self.pane_menu_rows(m.pane);
                let n = list.len();
                if n > 0 {
                    let to = (m.index as i64 + i64::from(rows)).clamp(0, n as i64 - 1) as usize;
                    m.index = land(&list, to, rows.signum().into());
                    self.overlay = Overlay::PaneMenu(m);
                }
            }
            _ => {}
        }
    }

    /// Backspace in an open window: edits typed text, never deletes a measurement or a
    /// trace behind the window. Where nothing is typed it is Delete (Settings, the delete
    /// confirmation), as on keyboards without a Delete key.
    pub(in crate::state) fn backspace(&mut self, out: &mut Vec<Request>) {
        match &mut self.overlay {
            Overlay::Palette(p) => p.backspace(),
            Overlay::Form(f) => f.backspace(),
            Overlay::Settings(_) => self.settings_backspace(out),
            Overlay::Sessions(_) => self.sessions_backspace(),
            Overlay::Eq(_) => self.eq_backspace(out),
            Overlay::Delete(_) => self.delete(true, out),
            Overlay::NewLog(_) => self.new_log(false, out),
            Overlay::Prompt(p) => {
                p.text.pop();
                p.error = None;
            }
            _ => {}
        }
    }

    pub(in crate::state) fn key(&mut self, chord: Chord, keymap: &Keymap, out: &mut Vec<Request>) {
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
        // stops the stimulus and nothing else: the stop reflex never changes the selection
        // (End goes back to the live measurement).
        if chord == Chord::key(Key::Escape) {
            // A row being dragged owns Esc as a window does: it cancels the drag only.
            if self.cancel_row_drag() {
                return;
            }
            if self.overlay == Overlay::None {
                self.command(CommandId::StimulusStop, keymap, out);
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
                let scope = self.layout.focused().scope();
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
            Overlay::Sessions(_) => {
                self.sessions_key(chord, swallow, out);
                return;
            }
            Overlay::Eq(_) => {
                self.eq_key(chord, swallow, keymap, out);
                return;
            }
            // The key that asked confirms (Enter is the stimulus's: never a discard by reflex).
            Overlay::NewLog(_) => {
                if self.new_log_confirms(chord) {
                    self.new_log(true, out);
                } else if chord.key == Key::N {
                    self.new_log(false, out);
                }
                return;
            }
            Overlay::Choose(c) => {
                let n = c.choices.len();
                match chord.key {
                    Key::Tab if chord.shift && n > 0 => c.index = c.index.saturating_sub(1),
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
            // Delete twice deletes, as in the calibrations view (Backspace is Delete here, the
            // app routes it above). Enter is the stimulus's: it deletes nothing by reflex, and
            // only closes the window that refuses.
            Overlay::Delete(p) => {
                if chord.key == Key::Delete || (chord.key == Key::Enter && p.confirm.refused) {
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
            Overlay::RigMenu(menu) => {
                let menu = *menu;
                self.rig_menu_key(menu, chord, out);
                return;
            }
            Overlay::PaneMenu(menu) => {
                let mut menu = *menu;
                let list = self.pane_menu_rows(menu.pane);
                let n = list.len();
                match chord.key {
                    Key::ArrowDown | Key::ArrowUp if n > 0 => {
                        // A rig's heading is stepped over, wrapping as the rows do.
                        let d = if chord.key == Key::ArrowDown {
                            1
                        } else {
                            n - 1
                        };
                        let mut i = (menu.index + d) % n;
                        for _ in 0..n {
                            if list[i].0.pickable() {
                                break;
                            }
                            i = (i + d) % n;
                        }
                        menu.index = i;
                        self.overlay = Overlay::PaneMenu(menu);
                    }
                    Key::Home | Key::PageUp => {
                        menu.index = land(&list, 0, 1);
                        self.overlay = Overlay::PaneMenu(menu);
                    }
                    Key::End | Key::PageDown => {
                        menu.index = land(&list, n.saturating_sub(1), -1);
                        self.overlay = Overlay::PaneMenu(menu);
                    }
                    Key::Enter => {
                        self.overlay = Overlay::None;
                        let row = self
                            .pane_menu_rows(menu.pane)
                            .get(menu.index)
                            .map(|(r, _)| *r);
                        if let Some(row) = row {
                            self.pane_pick(menu.pane, row);
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
    pub(in crate::state) fn overlay_fallthrough(
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
    pub(in crate::state) fn run_key_command(
        &mut self,
        c: CommandId,
        chord: Chord,
        keymap: &Keymap,
        out: &mut Vec<Request>,
    ) {
        let before = std::mem::discriminant(&self.overlay);
        self.command(c, keymap, out);
        let opened_text = self.overlay.takes_text();
        if opened_text && std::mem::discriminant(&self.overlay) != before {
            self.swallow_text = typed_char(&chord);
        }
    }

    /// Esc with a window open: the topmost window closes (a dialog over a view closes back
    /// to the view) and the stimulus is left alone: no dialog arms one.
    pub(in crate::state) fn close_overlay(&mut self) {
        if matches!(self.overlay, Overlay::Sessions(_)) {
            self.sessions_close();
            return;
        }
        match &mut self.overlay {
            Overlay::Settings(s) if s.inner_open() => {
                s.close_inner();
            }
            // Esc cancels the mark question, not the editor.
            Overlay::Eq(e) if e.confirm_mark => e.confirm_mark = false,
            _ => self.overlay = Overlay::None,
        }
    }

    /// Typed text, to the open window; the EQ editor sends a typed value at once.
    pub(in crate::state) fn text_with(&mut self, t: &str, out: &mut Vec<Request>) {
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
            Overlay::Sessions(_) => self.sessions_text(&t),
            Overlay::Eq(_) => self.eq_text(&t, out),
            Overlay::Prompt(p) => {
                p.text.push_str(&t);
                p.error = None;
            }
            _ => {}
        }
    }

    pub(in crate::state) fn prompt(&mut self, kind: PromptKind, text: String) {
        self.overlay = Overlay::Prompt(Prompt {
            kind,
            text,
            error: None,
        });
    }

    pub(in crate::state) fn apply_prompt(&mut self, out: &mut Vec<Request>) {
        let Overlay::Prompt(p) = &self.overlay else {
            return;
        };
        let kind = p.kind;
        let text = p.text.clone();
        let mut ir_form = None;
        let r = match kind {
            PromptKind::StimulusLevel => self.set_level_text(&text, out),
            PromptKind::CalDelete => self.cal_delete(&text, out),
            PromptKind::TraceMicCurve(id) => {
                let mics = self.daemon().map(|s| s.mics.clone()).unwrap_or_default();
                trace_mic_request(&mics, id, &text).map(|r| out.push(r))
            }
            PromptKind::MicCurveInput => parse_input_curve(&text).and_then(|(channel, curve)| {
                let mut row = self.input_setup(channel);
                let Some(mic) = row.mic().map(str::to_owned) else {
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
                row.set_curve(curve);
                let what = curve_what(&row);
                out.push(Request::Call {
                    what,
                    cmd: self.inputs_cmd(vec![row]),
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
                ac2_scene::unit_text::in_field::<ac2_scene::unit_text::Gain>(&text, "db")
                    .map_err(String::from)
                    .map(|v| self.set_live_offset(id, v.0.0, out))
            }
            PromptKind::TraceOffset(id) => {
                ac2_scene::unit_text::in_field::<ac2_scene::unit_text::Gain>(&text, "db")
                    .map_err(String::from)
                    .and_then(|v| {
                        let v = v.0.0;
                        let t = self.trace_meta(id)?;
                        let mut edit = t.edit.clone();
                        edit.offset = Db(v);
                        let what = format!("{}: offset {}", trace_label(&t), format::db_readout(v));
                        out.push(Request::Call {
                            cmd: Command::TraceUpdate { trace: id, edit },
                            what,
                        });
                        Ok(())
                    })
            }
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
            PromptKind::LeqExport { meas, previous } => {
                self.leq_export_entered(meas, previous, &text, out)
            }
            PromptKind::PaneExport(pane) => self.pane_export_entered(pane, &text),
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
                let path = std::path::PathBuf::from(text.trim());
                // A target is for the system being tuned: listed with its measurement. A
                // measured file is of a system ac2 does not know: Imported, until moved.
                let owner = match role {
                    ImportRole::Target => self.curve_owner(),
                    ImportRole::Trace => ac2_proto::model::TraceOwner::Imported,
                };
                if path.as_os_str().is_empty() {
                    Err("type a file path".to_string())
                } else {
                    // A WAV file is an impulse response: its options are asked first.
                    crate::ir_import_dialog::probe(&path).map(|header| match header {
                        Some(header) => {
                            ir_form = Some(crate::ir_import_dialog::form(
                                crate::ir_import_dialog::IrTarget {
                                    path,
                                    role,
                                    header,
                                    owner,
                                },
                            ));
                        }
                        None => out.push(Request::Import {
                            path,
                            role,
                            owner,
                            ir: None,
                        }),
                    })
                }
            }
            PromptKind::ReplayRecording => parse_session_ref(&text).and_then(|r| {
                let (recording, what) = match r {
                    SessionRef::Name { name } => (
                        ac2_proto::model::RecordingRef::Name { name: name.clone() },
                        name,
                    ),
                    SessionRef::Path { path } => (
                        ac2_proto::model::RecordingRef::Path { path: path.clone() },
                        path,
                    ),
                    SessionRef::Upload { .. } => {
                        return Err("a recording is a name or a path".to_string());
                    }
                };
                out.push(Request::Call {
                    cmd: Command::SessionReplay {
                        recording,
                        pace: ac2_proto::model::ReplayPace::Realtime,
                    },
                    what: format!("replaying {what:?}"),
                });
                Ok(())
            }),
            PromptKind::InputMics => parse_mics(&text).map(|mics| {
                let inputs: Vec<InputSetup> = mics
                    .into_iter()
                    .map(|(channel, mic)| {
                        let mut row = self.input_setup(channel);
                        // Another capsule: its own curves, chosen again.
                        row.set_mic(mic);
                        row
                    })
                    .collect();
                out.push(Request::Call {
                    what: format!("input setup ({})", mics_text(&inputs)),
                    cmd: self.inputs_cmd(inputs),
                });
            }),
            PromptKind::Delay(id) => {
                ac2_scene::unit_text::in_field::<ac2_scene::unit_text::DelayAmount>(&text, "ms")
                    .map_err(String::from)
                    .and_then(|v| {
                        // Negative is the measurement leading the reference (a mic ahead of the
                        // reference tap): the aligner buffers the measurement leg instead. The
                        // shared parser bounds a time to ±10 s.
                        let ac2_scene::unit_text::DelayAmount::Time(delay) = v else {
                            return Err("type the delay as a time, e.g. 12.5 ms".to_string());
                        };
                        let delay = delay.0;
                        // A typed delay keeps the arrival: the daemon moves the offset from it.
                        let (name, offset) = self.meas(id).map_or((String::new(), 0.0), |m| {
                            let offset = m
                                .delay
                                .as_ref()
                                .map_or(0.0, |d| d.nudged.0 + delay - d.applied.0);
                            (m.config.name.clone(), offset)
                        });
                        out.push(Request::Call {
                            cmd: Command::DelaySet {
                                meas: id,
                                delay: Seconds(delay),
                            },
                            what: format::typed_delay_toast(&name, delay, offset),
                        });
                        Ok(())
                    })
            }
        };
        match r {
            Ok(()) => {
                self.overlay = ir_form.map_or(Overlay::None, |f| Overlay::Form(Box::new(f)));
            }
            Err(e) => {
                if let Overlay::Prompt(p) = &mut self.overlay {
                    p.error = Some(e);
                }
            }
        }
    }
}

/// The row of a pane's list nearest `at` a pick can land on, looking in direction `d` first
/// (a rig's heading names the rig, it is no row to take).
fn land(rows: &[(PaneMenuRow, String)], at: usize, d: i64) -> usize {
    let ok = |i: usize| rows.get(i).is_some_and(|(r, _)| r.pickable());
    let step = |i: usize, d: i64| {
        usize::try_from(i as i64 + d)
            .ok()
            .filter(|i| *i < rows.len())
    };
    let d = if d < 0 { -1 } else { 1 };
    let mut i = Some(at);
    while let Some(x) = i {
        if ok(x) {
            return x;
        }
        i = step(x, d);
    }
    let mut i = step(at, -d);
    while let Some(x) = i {
        if ok(x) {
            return x;
        }
        i = step(x, -d);
    }
    at
}
