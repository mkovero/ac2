//! Key commands for sessions, recording, measurements, inputs and calibration, the delay
//! finder and averaging state.

use super::*;

impl AppState {
    /// The session, measurement, input and finder [`CommandId`]s [`AppState::command`] routes here.
    pub(super) fn command_meas(&mut self, c: CommandId, keymap: &Keymap, out: &mut Vec<Request>) {
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
            C::OpenSession => {
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
                match self.open_session().cloned() {
                    _ if recording => {
                        self.call(out, Command::RecStop, "recording stopped".into());
                    }
                    None => self.warn(format!(
                        "no audio session to record: {}",
                        open_session_hint(keymap)
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
            C::CloseSession => {
                if self.open_session().is_none() {
                    self.warn("no audio session is open");
                } else {
                    self.call(out, Command::SessionClose, "audio session closed".into());
                }
            }
            C::NewTransfer | C::NewSpectrum | C::NewRta | C::NewSpl => {
                let kind = match c {
                    C::NewSpectrum => FormKind::Spectrum,
                    C::NewRta => FormKind::Rta,
                    C::NewSpl => FormKind::Spl,
                    _ => FormKind::Transfer,
                };
                match self.open_session().cloned() {
                    None => self.warn(format!(
                        "no audio session to measure: {}",
                        open_session_hint(keymap)
                    )),
                    Some(o) => {
                        let names = self.session_input_names();
                        let mics: Vec<u16> = self
                            .daemon()
                            .map(|s| {
                                s.inputs
                                    .iter()
                                    .filter(|i| i.mic.is_some())
                                    .map(|i| i.channel)
                                    .collect()
                            })
                            .unwrap_or_default();
                        let f =
                            Form::measurement(kind, Some(&o), &self.measurements(), &names, &mics);
                        self.overlay = Overlay::Form(Box::new(f));
                        if self.devices.is_none() {
                            // Channel names arrive with the device list.
                            out.push(Request::Devices);
                        }
                    }
                }
            }
            C::NewMath => {
                // The channel lives under the measurement selected now (a selected trace's
                // or math channel's owner): its live curve and traces are offered first.
                let owner = self
                    .selected_group()
                    .unwrap_or(ac2_proto::model::TraceOwner::Imported);
                // A starts as what the focused pane shows: its measurement, or the selected
                // trace.
                let first = self
                    .selected_trace_meta()
                    .map(|t| Operand::Trace { trace: t.id })
                    .or_else(|| {
                        let p = match self.layout.focus_kind() {
                            PaneKind::Spectrum => PaneKind::Spectrum,
                            _ => PaneKind::Transfer,
                        };
                        // A sweep has no live curve to be an operand: its runs are traces.
                        self.kind_meas(p)
                            .filter(|m| m.config.kind.is_job())
                            .map(|m| Operand::Meas { meas: m.id })
                    });
                match Form::math(self.math_candidates(Some(owner)), first, owner) {
                    Some(f) => self.overlay = Overlay::Form(Box::new(f)),
                    None => self.warn(format!(
                        "a math channel needs two transfer functions, spectra or RTAs of one \
                         kind, live or stored: make a measurement or capture one ({}) first",
                        self.capture_keys()
                    )),
                }
            }
            C::NextColour | C::PrevColour => self.step_colour(c == C::NextColour, out),
            C::EditMeas => {
                // A sweep measurement selected (or the sweep pane's) edits its settings.
                let sweep = self
                    .selected_meas()
                    .filter(|m| matches!(m.config.kind, MeasKind::Sweep { .. }))
                    .or_else(|| {
                        (self.layout.focus_kind() == PaneKind::Distortion)
                            .then(|| self.sweep_meas())
                            .flatten()
                    })
                    .cloned();
                if let Some(m) = sweep {
                    let open = self.open_session().cloned();
                    let (inputs, outputs) =
                        (self.session_input_names(), self.session_output_names());
                    match Form::edit_sweep(&m, open.as_ref(), &inputs, &outputs) {
                        Ok(f) => self.overlay = Overlay::Form(Box::new(f)),
                        Err(e) => self.warn(e),
                    }
                    return;
                }
                // A transfer, spectrum or RTA selected edits its settings.
                let spec = self
                    .selected_meas()
                    .filter(|m| {
                        matches!(
                            m.config.kind,
                            MeasKind::Transfer { .. }
                                | MeasKind::Spectrum { .. }
                                | MeasKind::Rta { .. }
                        )
                    })
                    .cloned();
                if let Some(m) = spec {
                    let rate = self.open_session().map(|o| o.sample_rate_hz);
                    match Form::edit_spec(&m, &self.session_input_names(), rate) {
                        Ok(f) => self.overlay = Overlay::Form(Box::new(f)),
                        Err(e) => self.warn(e),
                    }
                    return;
                }
                let m = self
                    .selected_meas()
                    .filter(|m| matches!(m.config.kind, MeasKind::Math { .. }))
                    .or_else(|| {
                        [PaneKind::Transfer, PaneKind::Spectrum]
                            .into_iter()
                            .filter_map(|p| self.kind_meas(p))
                            .find(|m| matches!(m.config.kind, MeasKind::Math { .. }))
                    })
                    .cloned();
                let owner = m.as_ref().and_then(|m| match &m.config.kind {
                    MeasKind::Math { config } => Some(config.owner),
                    _ => None,
                });
                let candidates = self.math_candidates(owner);
                match m.map(|m| Form::edit_math(&m, candidates)) {
                    Some(Ok(f)) => self.overlay = Overlay::Form(Box::new(f)),
                    Some(Err(e)) => self.warn(e),
                    None => self.warn(
                        "select a measurement first (a transfer, sweep, spectrum, RTA or math channel)",
                    ),
                }
            }
            C::HideGroup => self.toggle_group_shown(out),
            C::MoveTrace => self.ask_move(),
            C::ToggleGroup => match self.selected_group() {
                Some(g) => {
                    if !self.collapsed.remove(&g) {
                        self.collapsed.insert(g);
                    }
                }
                None => {
                    self.warn("select a measurement first (click it in the list, or Tab to it)")
                }
            },
            C::InputMics => {
                let rows: Vec<InputSetup> = self
                    .daemon()
                    .map(|s| {
                        s.inputs
                            .iter()
                            .filter(|i| i.mic.is_some())
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
                None => self.warn("select a stored trace first (V, or click it in the list)"),
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
                let text = match self.finder.band {
                    FinderBand::Custom { lo_hz, hi_hz } => format!("{}-{}", lo_hz.0, hi_hz.0),
                    _ => String::new(),
                };
                self.prompt(PromptKind::FinderBand, text);
            }
            C::FinderObservation => {
                let text = self
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
                    let Some(mic) = row.mic.clone() else {
                        self.warn(format!(
                            "input {} has no mic name: name it first (palette: Input setup…)",
                            u32::from(input) + 1
                        ));
                        return;
                    };
                    let mics = self.daemon().map(|s| s.mics.clone()).unwrap_or_default();
                    ac2_proto::cal::settle(&mut row, &mics);
                    row.curve =
                        ac2_proto::cal::step(&row.curve, ac2_proto::cal::mic(&mics, &mic), true);
                    let what = curve_what(&row);
                    self.call(out, Command::SessionInputs { inputs: vec![row] }, what);
                }
            }

            C::ResetAverage => {
                let m = self.focused_pane_meas();
                if let Some(m) = &m
                    && let MeasKind::Math { config } = &m.config.kind
                {
                    // A math channel holds no averaging of its own: starting it over means
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
