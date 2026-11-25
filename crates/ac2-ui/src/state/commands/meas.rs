//! Key commands for sessions, recording, measurements, inputs and calibration, the delay
//! finder and averaging state.

use crate::state::*;

impl AppState {
    /// A new transfer form's default name: the reference → the measurement input's name.
    fn name_transfer(&self, f: &mut Form) {
        let mic = f.channel(crate::forms::FieldId::Measurement).and_then(|c| {
            self.stream_input_labels()
                .into_iter()
                .find(|(ch, _)| *ch == c)
                .map(|(_, l)| l)
        });
        let ms = self.measurements();
        let taken: Vec<&str> = ms.iter().map(|m| m.config.name.as_str()).collect();
        let n = ms
            .iter()
            .filter(|m| matches!(m.config.kind, MeasKind::Transfer { .. }))
            .count()
            + 1;
        let name = ac2_scene::new_meas::transfer_name(mic.as_deref(), &taken, n);
        f.set_text(crate::forms::FieldId::Name, &name);
    }

    /// Opens what edits `m`: the form that made it, on its settings; an SPL meter's
    /// Settings › SPL/Leq page, on that meter whatever the SPL pane shows.
    fn open_meas_edit(&mut self, m: &Measurement, out: &mut Vec<Request>) {
        let form = match &m.config.kind {
            MeasKind::Dynamics { .. } => {
                let open = self.open_stream().cloned();
                let (inputs, outputs) = (self.stream_input_names(), self.stream_output_names());
                Form::edit_dynamics(m, open.as_ref(), &inputs, &outputs)
            }
            MeasKind::Sweep { .. } => {
                let open = self.open_stream().cloned();
                let (inputs, outputs) = (self.stream_input_names(), self.stream_output_names());
                Form::edit_sweep(m, open.as_ref(), &inputs, &outputs)
            }
            MeasKind::Transfer { .. } | MeasKind::Spectrum { .. } | MeasKind::Rta { .. } => {
                Form::edit_spec(m, &self.stream_input_names(), self.open_stream())
            }
            MeasKind::Math { config } => {
                let candidates = self.math_candidates(Some(config.owner));
                let levels = self.transfer_levels();
                Form::edit_math(m, candidates).map(|mut f| {
                    f.set_levels(levels);
                    f
                })
            }
            MeasKind::Spl { .. } => {
                let leq = crate::leq_dialog::LeqDialog::new(
                    m,
                    self.leq_calibrated(m.id),
                    self.leq_panes.cues_shown,
                );
                self.open_settings(crate::settings::Page::Leq, out);
                if let Overlay::Settings(s) = &mut self.overlay {
                    s.leq = leq;
                }
                return;
            }
        };
        match form {
            Ok(f) => self.overlay = Overlay::Form(Box::new(f)),
            Err(e) => self.warn(e),
        }
    }

    /// A measurement's row asked to be edited (its edit button): selected as a click on
    /// the row selects it, then its form opened. The row names the measurement,
    /// so the focused pane has no say (unlike the command, which reads what is in focus).
    pub(in crate::state) fn edit_row(
        &mut self,
        key: ac2_scene::meas_list::TreeKey,
        out: &mut Vec<Request>,
    ) {
        use ac2_scene::meas_list::TreeKey;
        match key {
            TreeKey::Meas(id) => self.select_meas_row(id),
            TreeKey::Live(id) => self.select_live_row(id),
            TreeKey::Math(id) => self.select_math_row(id),
            TreeKey::Trace(_)
            | TreeKey::Imported
            | TreeKey::Rig
            | TreeKey::Session
            | TreeKey::RigName(_) => return,
        }
        let Some(m) = key.edits().and_then(|id| self.meas(id)).cloned() else {
            return;
        };
        self.open_meas_edit(&m, out);
    }

    /// The session, measurement, input and finder [`CommandId`]s [`AppState::command`] routes here.
    pub(in crate::state) fn command_meas(
        &mut self,
        c: CommandId,
        keymap: &Keymap,
        out: &mut Vec<Request>,
    ) {
        use CommandId as C;
        match c {
            C::ImportTrace => self.prompt(PromptKind::ImportFile(ImportRole::Trace), String::new()),
            C::Sessions => self.open_sessions(crate::sessions::Page::Main, out),
            C::SessionNew
            | C::SessionSave
            | C::SessionSaveAs
            | C::SessionLoad
            | C::SessionExport => {
                self.session_command(c, out);
            }
            C::Reconnect => out.push(Request::Reconnect),
            C::OpenStream => {
                if !self.connected() {
                    self.warn("not connected");
                } else {
                    self.open_audio_setup(out);
                }
            }
            C::Record => {
                let recording = self
                    .daemon()
                    .and_then(|s| s.recording.as_ref())
                    .is_some_and(ac2_proto::model::RecordingRun::active);
                match self.open_stream().cloned() {
                    _ if recording => {
                        self.call(out, Command::RecStop, "recording stopped".into());
                    }
                    None => self.warn(format!(
                        "no audio I/O to record: {}",
                        open_stream_hint(keymap)
                    )),
                    Some(o) => self.call(
                        out,
                        Command::RecStart {
                            request: ac2_proto::model::RecordRequest {
                                inputs: o.config.input_channels.clone(),
                                name: None,
                                max_duration: Seconds(
                                    self.prefs
                                        .record_limit_min
                                        .map_or(RECORD_MAX_S, |m| f64::from(m) * 60.0),
                                ),
                                max_bytes: None,
                            },
                        },
                        "recording started".into(),
                    ),
                }
            }
            C::ReplayRecording => {
                if self.connected() {
                    self.prompt(PromptKind::ReplayRecording, String::new());
                } else {
                    self.warn("not connected");
                }
            }
            C::CloseStream => {
                if self.open_stream().is_none() {
                    self.warn("no audio I/O is open");
                } else {
                    self.call(out, Command::StreamClose, "audio I/O closed".into());
                }
            }
            C::NewTransfer | C::NewSpectrum | C::NewRta | C::NewSpl => {
                let kind = match c {
                    C::NewSpectrum => FormKind::Spectrum,
                    C::NewRta => FormKind::Rta,
                    C::NewSpl => FormKind::Spl,
                    _ => FormKind::Transfer,
                };
                match self.open_stream().cloned() {
                    None => self.warn(format!(
                        "no audio I/O to measure: {}",
                        open_stream_hint(keymap)
                    )),
                    Some(_)
                        if let Some(why) = (kind == FormKind::Transfer)
                            .then(|| self.two_input_refusal(false, keymap))
                            .flatten() =>
                    {
                        self.warn(why)
                    }
                    Some(o) => {
                        let names = self.stream_input_names();
                        let mics: Vec<u16> = self
                            .daemon()
                            .map(|s| {
                                s.inputs
                                    .iter()
                                    .filter(|i| matches!(i.kind, InputKind::Mic { .. }))
                                    .map(|i| i.channel)
                                    .collect()
                            })
                            .unwrap_or_default();
                        let mut f =
                            Form::measurement(kind, Some(&o), &self.measurements(), &names, &mics);
                        if kind == FormKind::Transfer {
                            self.name_transfer(&mut f);
                        }
                        self.overlay = Overlay::Form(Box::new(f));
                        if self.rig.devices.is_none() {
                            // Channel names arrive with the device list.
                            out.push(Request::Devices);
                        }
                    }
                }
            }
            C::NewMath => match self.math_form() {
                Ok(f) => self.overlay = Overlay::Form(Box::new(f)),
                Err(why) => self.warn(why),
            },
            C::NewAlignment => match self.alignment_form() {
                Ok(f) => self.overlay = Overlay::Form(Box::new(f)),
                Err(why) => self.warn(why),
            },
            C::NewMeasurement => self.open_new_measurement(self.layout.focus, keymap),
            C::NextColour | C::PrevColour => self.step_colour(c == C::NextColour, out),
            C::EditMeas => {
                // A focused Programme pane shows no measurement, so the selection is not what
                // the operator is pointing at; what it meters changes by a row dropped on it.
                if self.layout.focus_kind() == PaneKind::Programme {
                    self.warn(ac2_scene::pane_edit::PROGRAMME_EDIT);
                    return;
                }
                // The selected sweep, else the sweep pane's; else the selected measurement;
                // else a math trace a pane shows.
                let selected = self.selected_meas();
                let target = selected
                    .filter(|m| matches!(m.config.kind, MeasKind::Sweep { .. }))
                    .or_else(|| {
                        (self.layout.focus_kind() == PaneKind::Distortion)
                            .then(|| self.sweep_meas())
                            .flatten()
                    })
                    .or(selected)
                    .or_else(|| {
                        [PaneKind::Transfer, PaneKind::Spectrum]
                            .into_iter()
                            .filter_map(|p| self.kind_meas(p))
                            .find(|m| matches!(m.config.kind, MeasKind::Math { .. }))
                    })
                    .cloned();
                match target {
                    Some(m) => self.open_meas_edit(&m, out),
                    None => self.warn(
                        "select a measurement first (a transfer, sweep, spectrum, RTA, SPL \
                         meter or math trace), or focus a Programme pane",
                    ),
                }
            }
            C::HideGroup => self.toggle_group_shown(out),
            C::MoveTrace => self.ask_move(),
            C::ToggleGroup => match self.selected_group() {
                Some(g) => {
                    if !self.rig.collapsed.remove(&g) {
                        self.rig.collapsed.insert(g);
                    }
                }
                None => self.warn(self.select_meas_first()),
            },
            C::InputMics => {
                let rows: Vec<InputSetup> = self
                    .daemon()
                    .map(|s| {
                        s.inputs
                            .iter()
                            .filter(|i| i.mic().is_some())
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                self.prompt(PromptKind::InputMics, mics_text(&rows));
            }
            C::TraceMicCurve => match self.selected_trace_meta().cloned() {
                Some(t) => {
                    // Start from what the trace knows: the applied curve, else the mic it
                    // was captured with.
                    let text = t
                        .mic_curve
                        .as_ref()
                        .map(|m| ac2_scene::cal::curve_name(&m.mic, &m.curve.label))
                        .or_else(|| t.mic.as_ref().map(|m| format!("{} ", m.name)))
                        .unwrap_or_default();
                    self.prompt(PromptKind::TraceMicCurve(t.id), text);
                }
                None => {
                    let v = self.keys.reach(CommandId::NextTrace);
                    self.warn(format!(
                        "select a stored trace first ({v}, or click it in the list)"
                    ));
                }
            },
            C::CalDelete => {
                let text = self.cal_delete_text();
                self.prompt(PromptKind::CalDelete, text);
            }
            C::InputSetup | C::Calibrations => match self.daemon() {
                Some(_) if c == C::InputSetup => {
                    self.open_settings(Page::AudioIo, out);
                    if let Overlay::Settings(s) = &mut self.overlay {
                        s.focus_channels(FocusWant::Inputs);
                    }
                }
                Some(_) => self.open_settings(Page::Calibration, out),
                None => self.warn("not connected to a daemon"),
            },
            C::MicCurveInput => {
                let text = self
                    .selected_meas()
                    .and_then(|m| meas_input(&m.config.kind))
                    .map(|i| format!("{}=", u32::from(i) + 1))
                    .unwrap_or_default();
                self.prompt(PromptKind::MicCurveInput, text);
            }
            C::FinderAuto | C::FinderFull | C::FinderMid | C::FinderSub => {
                let band = match c {
                    C::FinderFull => FinderBand::Full,
                    C::FinderMid => FinderBand::Mid,
                    C::FinderSub => FinderBand::Sub,
                    _ => FinderBand::Auto,
                };
                self.set_finder(FinderChoice {
                    band,
                    observation: None,
                });
            }
            C::FinderCustom => {
                let text = match self.rig.finder.band {
                    FinderBand::Custom { lo_hz, hi_hz } => format!("{}-{}", lo_hz.0, hi_hz.0),
                    _ => String::new(),
                };
                self.prompt(PromptKind::FinderBand, text);
            }
            C::FinderObservation => {
                let text = self
                    .rig
                    .finder
                    .observation
                    .map(|o| o.0.to_string())
                    .unwrap_or_default();
                self.prompt(PromptKind::FinderObservation, text);
            }
            C::MicCurve => {
                if let Some(m) = self.need_meas(
                    &[|k| {
                        matches!(
                            k,
                            MeasKind::Transfer { .. }
                                | MeasKind::Spectrum { .. }
                                | MeasKind::Rta { .. }
                                | MeasKind::Spl { .. }
                        )
                    }],
                    "transfer, spectrum, RTA or SPL",
                ) && let Some(input) = meas_input(&m.config.kind)
                {
                    let mut row = self.input_setup(input);
                    let Some(mic) = row.mic().map(str::to_owned) else {
                        self.warn(format!(
                            "input {} has no mic name: name it first (palette: Input setup…)",
                            u32::from(input) + 1
                        ));
                        return;
                    };
                    let mics = self.daemon().map(|s| s.mics.clone()).unwrap_or_default();
                    ac2_proto::cal::settle(&mut row, &mics);
                    row.set_curve(ac2_proto::cal::step(
                        &row.curve(),
                        ac2_proto::cal::mic(&mics, &mic),
                        true,
                    ));
                    let what = curve_what(&row);
                    let cmd = self.inputs_cmd(vec![row]);
                    self.call(out, cmd, what);
                }
            }

            C::ResetAverage => {
                let m = self.focused_pane_meas();
                // A peak hold starts over with the averages: what it held is of the
                // curve before the reset.
                if let Some(m) = &m {
                    self.rig.folded.peaks.remove(&m.id);
                }
                if let Some(m) = &m
                    && let MeasKind::Math { config } = &m.config.kind
                {
                    // A math trace holds no averaging of its own: starting it over means
                    // starting its live operands over.
                    for o in config.expr.operands() {
                        if let Operand::Meas { meas } = o {
                            self.call(
                                out,
                                Command::MeasReset { meas },
                                format!("{}: operands' averaging reset", m.config.name),
                            );
                        }
                    }
                } else if let Some(m) = m {
                    self.call(
                        out,
                        Command::MeasReset { meas: m.id },
                        format!("{} averaging reset", m.config.name),
                    );
                }
            }
            C::StartStop => {
                if let Some(m) = self.focused_pane_meas() {
                    if m.running {
                        let stim = if self.stop_stimulus_with(&m, out) {
                            STIMULUS_STOPPED_TOO
                        } else {
                            ""
                        };
                        self.call(
                            out,
                            Command::MeasStop { meas: m.id },
                            format!("{} stopped{stim}", m.config.name),
                        );
                    } else {
                        self.call(
                            out,
                            Command::MeasStart { meas: m.id },
                            format!("{} started", m.config.name),
                        );
                    }
                }
            }
            _ => unreachable!("{c:?} is not a measurement command: routed by command"),
        }
    }
}
