//! Dialog submissions: forms, the calibration view, the session dialog, the new-log and measurement offers, averaging and math operands.

use super::*;

impl AppState {
    pub(super) fn form_msg(&mut self, m: FormMsg, out: &mut Vec<Request>) {
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
    pub(super) fn submit_form(&mut self, out: &mut Vec<Request>) {
        let open = self.open_session().cloned();
        let ceiling = self.ceiling();
        if let Overlay::Form(f) = &mut self.overlay
            && matches!(f.kind, FormKind::Sweep | FormKind::SweepEdit)
        {
            // A sweep measurement is made (or changed) without arming: Space on the sweep
            // pane arms its run, Enter plays it.
            match f.sweep_meas(open.as_ref(), ceiling) {
                Ok(config) => {
                    let req = match f.sweep_edit {
                        Some(meas) => Request::Call {
                            what: format!("{} changed: its next run uses it", config.name),
                            cmd: Command::MeasUpdate { meas, config },
                        },
                        None => Request::CreateMeas { config },
                    };
                    out.push(req);
                    self.overlay = Overlay::None;
                    // A sweep is run from the sweep pane.
                    self.focus_kind(PaneKind::Distortion);
                }
                Err(e) => f.error = Some(e),
            }
            return;
        }
        let Overlay::Form(f) = &mut self.overlay else {
            return;
        };
        let edit = f.math_edit();
        let r = f.meas_config(open.as_ref()).map(|config| match edit {
            Some(meas) => Request::Call {
                what: format!("{} changed", config.name),
                cmd: Command::MeasUpdate { meas, config },
            },
            None => Request::CreateMeas { config },
        });
        match r {
            Ok(req) => {
                out.push(req);
                self.overlay = Overlay::None;
            }
            Err(e) => f.error = Some(e),
        }
    }

    /// Keys of the calibrations view.
    pub(super) fn cal_view_key(
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
    pub(super) fn cal_action(&mut self, a: CalAction, out: &mut Vec<Request>) {
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

    pub(super) fn session_msg(&mut self, m: SessionMsg, out: &mut Vec<Request>) {
        let level = self.stimulus.level;
        let ceiling = self.ceiling();
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        // The mouse reaches a row on the page it is drawn on.
        if let SessionMsg::Focus(r)
        | SessionMsg::Cycle(r, _)
        | SessionMsg::Toggle(r)
        | SessionMsg::Role(r, _)
        | SessionMsg::EditMic(r) = m
        {
            s.show_row(r);
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
    pub(super) fn submit_session(&mut self, out: &mut Vec<Request>) {
        let no_measurements = self.daemon().is_some_and(|s| s.measurements.is_empty());
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
            transfers: if no_measurements {
                plan.transfers
            } else {
                Vec::new()
            },
            what: format!("audio session open on {}", plan.device_name),
        });
        self.overlay = Overlay::None;
    }

    /// The new SPL log confirmation: start it, or keep the current log.
    pub(super) fn new_log(&mut self, go: bool, out: &mut Vec<Request>) {
        let Overlay::NewLog(p) = &self.overlay else {
            return;
        };
        if go {
            let (meas, what) = (p.meas, format!("{}: new SPL log started", p.meter));
            self.call(out, Command::SplLogNew { meas }, what);
        }
        self.overlay = Overlay::None;
    }

    /// The measurement offer: create every transfer measurement, or skip.
    pub(super) fn offer(&mut self, create: bool, out: &mut Vec<Request>) {
        let Overlay::Offer(o) = &self.overlay else {
            return;
        };
        if create {
            for config in o.transfers.clone() {
                out.push(Request::CreateMeas { config });
            }
        }
        self.overlay = Overlay::None;
    }

    /// M: averages the shown stored transfer traces, phase referred to the phase reference
    /// when it is one of them, else to the first (decision 8b).
    pub(super) fn average(&mut self, method: AverageMethod, out: &mut Vec<Request>) {
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
        let m = match method {
            AverageMethod::Power => "power",
            AverageMethod::Complex => "complex",
            AverageMethod::CoherenceWeighted => "coherence-weighted",
        };
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
        }
    }

    /// What a math channel can combine: the live measurements and the stored traces.
    /// What a math channel can combine; `owner`'s live curve and traces first.
    pub(super) fn math_candidates(
        &self,
        owner: Option<ac2_proto::model::TraceOwner>,
    ) -> Vec<crate::math_dialog::Candidate> {
        crate::math_dialog::Candidate::all(&self.measurements(), &self.trace_list(), owner)
    }
}
