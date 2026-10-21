//! Dialog submissions: forms, the calibration view, the session dialog, the new-log and measurement offers, averaging and math operands.

use crate::state::*;

impl AppState {
    pub(in crate::state) fn form_msg(&mut self, m: FormMsg, out: &mut Vec<Request>) {
        match m {
            FormMsg::Submit => self.submit_form(out),
            // As Esc: the sweep dialog leaves nothing armed behind it.
            FormMsg::Cancel => {
                if matches!(self.overlay, Overlay::Form(_)) {
                    self.close_overlay();
                }
            }
            FormMsg::Focus(i) => {
                if let Overlay::Form(f) = &mut self.overlay {
                    f.focus_field(i);
                }
            }
            FormMsg::Cycle(i, d) => {
                if let Overlay::Form(f) = &mut self.overlay {
                    f.focus_field(i);
                    f.cycle(d);
                }
            }
        }
    }

    /// Enter on a dialog: the command goes out and the dialog closes, or the dialog says
    /// what is wrong and stays.
    pub(in crate::state) fn submit_form(&mut self, out: &mut Vec<Request>) {
        let open = self.open_session().cloned();
        let ceiling = self.ceiling();
        if let Overlay::Form(f) = &mut self.overlay
            && matches!(f.kind, FormKind::Sweep | FormKind::SweepEdit)
        {
            // A sweep measurement is made (or changed) without arming: Space on the sweep
            // pane arms its run, Enter plays it.
            match f.sweep_meas(open.as_ref(), ceiling) {
                Ok(config) => {
                    let edit = f.sweep_edit;
                    let req = match edit {
                        Some(meas) => Request::Call {
                            what: format!("{} changed: its next run uses it", config.name),
                            cmd: self.meas_update(meas, config),
                        },
                        None => {
                            self.pane_on_create = Some(config.name.clone());
                            Request::CreateMeas { config }
                        }
                    };
                    out.push(req);
                    self.overlay = Overlay::None;
                    // A sweep is run from the sweep pane. A new one's pane comes with it,
                    // once made: the panes changed now would be stored as the selected
                    // measurement's own.
                    if edit.is_some() {
                        self.focus_kind(PaneKind::Distortion);
                    }
                }
                Err(e) => f.error = Some(e),
            }
            return;
        }
        let Overlay::Form(f) = &mut self.overlay else {
            return;
        };
        if matches!(f.kind, FormKind::TargetTilt | FormKind::TargetLoudness) {
            match crate::target_dialog::curve(f) {
                Ok(curve) => {
                    self.overlay = Overlay::None;
                    self.add_target(curve, out);
                }
                Err(e) => f.error = Some(e),
            }
            return;
        }
        if f.kind == FormKind::IrImport {
            match crate::ir_import_dialog::request(f) {
                Ok(req) => {
                    out.push(req);
                    self.overlay = Overlay::None;
                }
                Err(e) => f.error = Some(e),
            }
            return;
        }
        let edit = f.edits();
        let transfer = f.kind == FormKind::Transfer && edit.is_none();
        let config = match f.meas_config(open.as_ref()) {
            Ok(c) => c,
            Err(e) => {
                f.error = Some(e);
                return;
            }
        };
        let req = match edit {
            Some(meas) => Request::Call {
                what: format!("{} changed", config.name),
                cmd: self.meas_update(meas, config),
            },
            None => {
                if transfer {
                    self.stack_on_create = Some(config.name.clone());
                } else if matches!(&config.kind, MeasKind::Math { config: m } if matches!(m.expr, ac2_proto::model::MathExpr::Align { .. }))
                {
                    self.align_on_create = Some(config.name.clone());
                } else if !matches!(config.kind, MeasKind::Math { .. }) {
                    // A math channel is drawn on the pane of what it computes from.
                    self.pane_on_create = Some(config.name.clone());
                }
                Request::CreateMeas { config }
            }
        };
        out.push(req);
        self.overlay = Overlay::None;
    }

    /// Keys of the calibrations view.
    pub(in crate::state) fn cal_view_key(
        &mut self,
        chord: Chord,
        swallow: Option<char>,
        out: &mut Vec<Request>,
    ) {
        use eframe::egui::Key;
        let Some(st) = self.mirror.as_ref().and_then(|m| m.state.clone()) else {
            self.overlay = Overlay::None;
            return;
        };
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        let v = &mut s.cal;
        if let Some(d) = &mut v.acoustic {
            if let Some(a) = crate::acoustic_dialog::key(d, &chord) {
                self.cal_action(a, out);
            } else {
                self.swallow_text = swallow;
            }
            return;
        }
        if let Some(d) = &mut v.electrical {
            if let Some(a) = crate::electrical_dialog::key(d, &chord) {
                self.cal_action(a, out);
            } else {
                self.swallow_text = swallow;
            }
            return;
        }
        let plain = !(chord.command || chord.alt);
        let action = match chord.key {
            Key::Enter if v.edit.is_some() => v.finish(&st),
            Key::Enter => None,
            Key::ArrowUp => {
                v.move_focus(&st, -1);
                None
            }
            Key::Tab if chord.shift => {
                v.move_focus(&st, -1);
                None
            }
            Key::ArrowDown | Key::Tab => {
                v.move_focus(&st, 1);
                None
            }
            Key::PageUp | Key::PageDown | Key::Home | Key::End if v.edit.is_none() => {
                let d = match chord.key {
                    Key::PageUp => -CAL_PAGE,
                    Key::PageDown => CAL_PAGE,
                    Key::Home => i32::MIN / 2,
                    _ => i32::MAX / 2,
                };
                v.jump_focus(&st, d);
                None
            }
            Key::ArrowLeft | Key::ArrowRight if v.edit.is_none() => {
                v.step_curve(&st, chord.key == Key::ArrowRight)
            }
            _ if v.edit.is_some() => {
                self.swallow_text = swallow;
                None
            }
            Key::N | Key::F2 if plain => {
                if v.start_mic_name(&st) {
                    self.swallow_text = typed_char(&chord);
                }
                None
            }
            Key::I if plain => {
                if v.start_import(&st) {
                    self.swallow_text = typed_char(&chord);
                }
                None
            }
            Key::R if plain => {
                if v.start_rename(&st) {
                    self.swallow_text = typed_char(&chord);
                }
                None
            }
            Key::E if plain => {
                if v.start_electrical(&st) {
                    self.swallow_text = typed_char(&chord);
                }
                None
            }
            Key::C if plain => {
                if v.start_acoustic(&st) {
                    self.swallow_text = typed_char(&chord);
                }
                None
            }
            Key::Delete | Key::Backspace => v.delete(&st),
            _ => {
                self.swallow_text = swallow;
                None
            }
        };
        if let Some(a) = action {
            self.cal_action(a, out);
        }
    }

    /// A calibrations-view action as requests.
    pub(in crate::state) fn cal_action(&mut self, a: CalAction, out: &mut Vec<Request>) {
        match a {
            CalAction::Inputs(row, what) => out.push(Request::Call {
                what,
                cmd: Command::SessionInputs { inputs: vec![row] },
            }),
            CalAction::Import { path, mic, input } => {
                out.push(Request::ImportCurve { path, mic, input })
            }
            CalAction::Rename(id, label) => out.push(Request::Call {
                what: format!(
                    "curve {} renamed {label}",
                    ac2_scene::cal::curve_name(&id.mic, &id.label)
                ),
                cmd: Command::CalCurveRename { curve: id, label },
            }),
            CalAction::DeleteCurve(id) => out.push(Request::Call {
                what: format!(
                    "curve {} deleted",
                    ac2_scene::cal::curve_name(&id.mic, &id.label)
                ),
                cmd: Command::CalCurveDelete { curve: id },
            }),
            CalAction::Calibrate(cmd, what) => out.push(Request::Call { what, cmd }),
            CalAction::DeleteSensitivity(key) => out.push(Request::Call {
                what: format!(
                    "sensitivity calibration of {} on input {} deleted",
                    key.mic,
                    u32::from(key.channel) + 1
                ),
                cmd: Command::CalDelete { key },
            }),
        }
    }

    pub(in crate::state) fn session_msg(&mut self, m: SessionMsg, out: &mut Vec<Request>) {
        let level = self.stimulus.level;
        let ceiling = self.ceiling();
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        // A row the mouse acts on takes the focus off the max level row.
        if matches!(
            m,
            SessionMsg::Focus(_)
                | SessionMsg::Cycle(..)
                | SessionMsg::Toggle(_)
                | SessionMsg::Role(..)
                | SessionMsg::EditMic(_)
        ) {
            s.on_ceiling = false;
        }
        let d = &mut s.session;
        match m {
            SessionMsg::Focus(r) => {
                s.on_ceiling = false;
                d.detect_cancel();
                d.focus_row(r);
            }
            SessionMsg::Cycle(r, step) => {
                d.detect_cancel();
                d.focus_row(r);
                d.cycle(step, &self.prefs);
            }
            SessionMsg::Toggle(r) => {
                s.on_ceiling = false;
                d.focus_row(r);
                d.toggle();
            }
            SessionMsg::Role(r, k) => {
                s.on_ceiling = false;
                d.focus_row(r);
                d.assign(k);
                if k == crate::session_dialog::RoleKey::Stimulus {
                    self.stimulus_ticks_now(out);
                }
            }
            SessionMsg::EditMic(r) => {
                s.on_ceiling = false;
                d.focus_row(r);
                if matches!(r, Row::Output(_)) {
                    d.start_label_edit();
                } else {
                    d.start_mic_edit();
                }
            }
            SessionMsg::Detect => {
                if let Err(e) = d.detect_start(level) {
                    d.error = Some(e);
                }
            }
            SessionMsg::Panel => {
                if let Some((backend, device)) = d.panel_target() {
                    let what = match backend {
                        ac2_proto::model::BackendKind::CoreAudio => {
                            format!("Audio MIDI Setup for {}", device.0)
                        }
                        _ => format!("driver control panel of {}", device.0),
                    };
                    out.push(Request::Call {
                        what,
                        cmd: Command::SessionControlPanel { backend, device },
                    });
                }
            }
            SessionMsg::DetectConfirm => {
                if let Some(req) = d.detect_confirm(ceiling) {
                    out.push(Request::DetectLoopback(req));
                }
            }
            SessionMsg::DetectCancel => d.detect_cancel(),
            SessionMsg::Submit => self.submit_session(out),
            SessionMsg::Cancel => self.overlay = Overlay::None,
        }
    }

    /// Enter on the session dialog: opens the session its roles describe, remembers them
    /// for the device, and points the stimulus at the chosen outputs (K4).
    pub(in crate::state) fn submit_session(&mut self, out: &mut Vec<Request>) {
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        let d = &mut s.session;
        d.finish_edit();
        let plan = match d.plan() {
            Ok(p) => p,
            Err(e) => {
                d.error = Some(e);
                return;
            }
        };
        let backend = plan
            .config
            .backend
            .unwrap_or(ac2_proto::model::BackendKind::Fake);
        self.prefs.sessions.insert(
            crate::prefs::UiPrefs::device_key(backend, &plan.device_id),
            plan.roles.clone(),
        );
        if !plan.roles.stimulus.is_empty() {
            self.prefs
                .outputs
                .insert(plan.output_device_id.clone(), plan.roles.stimulus.clone());
            if self.stimulus.phase == StimPhase::Idle {
                self.stimulus.outputs = plan.roles.stimulus.clone();
                self.stim_device = Some(plan.output_device_id.clone());
            }
        }
        self.prefs_dirty = true;
        out.push(Request::OpenSession {
            config: plan.config,
            inputs: plan.inputs,
            what: format!("audio I/O open on {}", plan.device_name),
        });
        self.overlay = Overlay::None;
    }

    /// Whether `chord` starts the new SPL log in its confirmation: the key that asked
    /// (Shift+R), pressed again; Enter only where the keymap leaves the command without a key.
    pub(in crate::state) fn new_log_confirms(&self, chord: Chord) -> bool {
        match self.keys.key_hint(CommandId::SplNewLog, Scope::Spl) {
            Some(_) => self.keys.lookup(Scope::Spl, chord) == Some(CommandId::SplNewLog),
            None => chord == Chord::key(eframe::egui::Key::Enter),
        }
    }

    /// The confirming key as the confirmation names it ([`Self::new_log_confirms`]).
    pub(crate) fn new_log_key(&self) -> String {
        self.keys
            .key_hint(CommandId::SplNewLog, Scope::Spl)
            .unwrap_or_else(|| Chord::key(eframe::egui::Key::Enter).label())
    }

    /// The new SPL log confirmation: start it, or keep the current log.
    pub(in crate::state) fn new_log(&mut self, go: bool, out: &mut Vec<Request>) {
        let Overlay::NewLog(p) = &self.overlay else {
            return;
        };
        if go {
            let (meas, what) = (p.meas, format!("{}: new SPL log started", p.meter));
            self.call(out, Command::SplLogNew { meas }, what);
        }
        self.overlay = Overlay::None;
    }

    /// M: averages the shown stored transfer traces, phase referred to the phase reference
    /// when it is one of them, else to the first (decision 8b).
    pub(in crate::state) fn average(&mut self, method: AverageMethod, out: &mut Vec<Request>) {
        let shown: Vec<&TraceMeta> = self
            .shown_transfer_traces()
            .into_iter()
            .filter(|t| matches!(t.kind, TraceKind::Transfer | TraceKind::Sweep))
            .collect();
        if shown.len() < 2 {
            self.warn("average: show at least two stored transfer traces (1…9)");
            return;
        }
        let ids: Vec<TraceId> = shown.iter().map(|t| t.id).collect();
        let reference = match self.view.tf.phase_reference {
            Some(TraceKey::Stored(r)) if ids.contains(&r) => r,
            _ => ids[0],
        };
        let label = |t: &TraceMeta| {
            t.edit
                .slot
                .map_or_else(|| t.edit.name.clone(), |s| format!("S{s}"))
        };
        let name = format!(
            "avg {}",
            shown.iter().map(|t| label(t)).collect::<Vec<_>>().join("+")
        );
        let m = method.label();
        let what = format!("{name} ({m}) created");
        self.call(
            out,
            Command::TraceAverage {
                traces: ids,
                method,
                reference: DelayReference::Trace { trace: reference },
                name,
            },
            what,
        );
    }

    /// Operand `o`'s name: the measurement's or the stored trace's.
    pub fn operand_name(&self, o: Operand) -> String {
        match o {
            Operand::Meas { meas } => self
                .meas(meas)
                .map_or_else(|| format!("measurement {meas}"), |m| m.config.name.clone()),
            Operand::Trace { trace } => self
                .daemon()
                .and_then(|s| s.traces.iter().find(|t| t.id == trace))
                .map_or_else(|| format!("trace {trace}"), |t| t.edit.name.clone()),
            Operand::Eq { meas, set } => self
                .meas(meas)
                .and_then(|m| m.eq.iter().find(|s| s.id == set))
                .map_or_else(|| format!("EQ {}", set.0), |s| s.body.name.clone()),
        }
    }

    /// The delay operand `o`'s phase is referred to, seconds: a live measurement's applied
    /// delay, a stored trace's delay at capture; `None` for a curve without one.
    pub fn operand_delay(&self, o: Operand) -> Option<f64> {
        match o {
            Operand::Meas { meas } => self.meas(meas)?.delay.as_ref().map(|d| d.applied.0),
            Operand::Trace { trace } => self
                .daemon()?
                .traces
                .iter()
                .find(|t| t.id == trace)
                .map(|t| t.delay.0),
            // A filter shares no arrival with a curve: it is applied, not aligned.
            Operand::Eq { .. } => None,
        }
    }

    /// What a math channel can combine: the live measurements and the stored traces.
    /// What a math channel can combine; `owner`'s live curve and traces first.
    pub(in crate::state) fn math_candidates(
        &self,
        owner: Option<ac2_proto::model::TraceOwner>,
    ) -> Vec<crate::math_dialog::Candidate> {
        crate::math_dialog::Candidate::all(&self.measurements(), &self.trace_list(), owner)
    }
}
