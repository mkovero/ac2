//! Key commands ([`CommandId`]) and the selection, pane and trace steps they make.

use super::*;

impl AppState {
    /// The one trace selection: a sweep selected is also what the sweep pane shows.
    pub(super) fn select_trace(&mut self, id: Option<TraceId>) {
        self.selected_trace = id;
        if let Some(id) = id
            && self.daemon().is_some_and(|s| {
                s.traces
                    .iter()
                    .any(|t| t.id == id && t.kind == TraceKind::Sweep)
            })
        {
            self.sweep.shown = Some(id);
        }
    }

    pub(super) fn set_finder(&mut self, f: FinderChoice) {
        self.finder = f;
        self.toast(format!("delay finder: {} · X finds", f.describe()));
    }

    pub(super) fn set_observation(&mut self, text: &str) -> Result<(), String> {
        let observation = if text.trim().is_empty() {
            None
        } else {
            let v = parse_number(text, &["s"])?;
            if !(v > 0.0 && v <= MAX_OBSERVATION_S) {
                return Err(format!(
                    "observation must be above 0 and at most {MAX_OBSERVATION_S} s"
                ));
            }
            Some(Seconds(v))
        };
        let f = FinderChoice {
            observation,
            ..self.finder
        };
        if let Some(o) = observation
            && f.sub_like()
            && !SUB_OBSERVATIONS_S.contains(&o.0)
        {
            return Err("the sub band observes 2, 4 or 8 s".into());
        }
        self.set_finder(f);
        Ok(())
    }

    /// The prompt text of a calibration to delete: the selected measurement's input and its
    /// mic name.
    pub(super) fn cal_delete_text(&self) -> String {
        self.selected_meas()
            .and_then(|m| meas_input(&m.config.kind))
            .map(|input| {
                let mic = self.input_setup(input).mic.unwrap_or_default();
                format!("{}={mic}", u32::from(input) + 1)
            })
            .unwrap_or_default()
    }

    pub(super) fn cal_delete(&mut self, text: &str, out: &mut Vec<Request>) -> Result<(), String> {
        let (channel, mic) = match parse_mics(text)?.as_slice() {
            [(c, Some(m))] => (*c, m.clone()),
            [(c, None)] => {
                return Err(format!("type the mic name after {}=", u32::from(*c) + 1));
            }
            _ => return Err("one input=mic".into()),
        };
        let Some(device) = self
            .daemon()
            .and_then(|s| s.session.open.as_ref())
            .map(|o| o.input_device.clone())
        else {
            return Err(
                "no open session: calibrations of other devices are deleted with `ac2 cal rm --device`"
                    .into(),
            );
        };
        out.push(Request::Call {
            what: format!(
                "sensitivity calibration of {mic} on input {} deleted",
                u32::from(channel) + 1
            ),
            cmd: Command::CalDelete {
                key: CalKey {
                    device,
                    channel,
                    mic,
                },
            },
        });
        Ok(())
    }

    /// The stimulus outputs follow the open session's output device: the ones last used on
    /// it, output 1 on a device never used (decision K4). Outputs never change under a held
    /// stimulus; a device change re-opens the session, which disarms it anyway.
    pub(super) fn follow_output_device(&mut self) {
        let dev = self
            .daemon()
            .and_then(|s| s.session.open.as_ref())
            .map(|o| o.output_device.0.clone());
        let Some(dev) = dev else {
            return;
        };
        if self.stim_device.as_ref() == Some(&dev) || self.stimulus.phase != StimPhase::Idle {
            return;
        }
        self.stimulus.outputs = self
            .prefs
            .outputs_for(&dev)
            .map_or_else(|| vec![0], <[u16]>::to_vec);
        self.stim_device = Some(dev);
    }

    pub(super) fn need_meas(
        &mut self,
        want: &[fn(&MeasKind) -> bool],
        what: &str,
    ) -> Option<Measurement> {
        match self.selected_meas() {
            Some(m) if want.iter().any(|f| f(&m.config.kind)) => Some(m.clone()),
            _ => {
                self.warn(format!("select a {what} measurement first (N)"));
                None
            }
        }
    }

    /// The daemon's input setup row of `channel` (default: no mic name, no curve chosen).
    pub(super) fn input_setup(&self, channel: u16) -> InputSetup {
        ac2_proto::cal::input_setup(
            self.daemon()
                .map(|s| s.inputs.as_slice())
                .unwrap_or_default(),
            channel,
        )
    }

    /// Moves the selected transfer measurement's one delay by `step` (`delay.nudge`): plain
    /// `,` / `.` by 0.1 ms, Ctrl / Alt by a sample or a tenth. The daemon keeps the averages
    /// where it can, so the curve moves at once.
    ///
    /// A step is something the operator sees: its live curve moves (a later delay leads the
    /// phase, e^{+jωΔ}), and no other curve moves. With no live curve on the pane (stopped,
    /// or hidden) the step would change nothing visible now and surprise later, so it is
    /// refused with the reason.
    pub(super) fn step_delay(&mut self, step: format::DelayStep, out: &mut Vec<Request>) {
        let Some(m) = self.need_tf() else {
            return;
        };
        if !m.running {
            self.toast(format!(
                "{} is stopped \u{2014} its delay applies to the live curve; S starts it",
                m.config.name
            ));
            return;
        }
        if self.meas_hidden(&m) {
            self.toast(format!(
                "{} is hidden \u{2014} its delay applies to the live curve; show it first",
                m.config.name
            ));
            return;
        }
        let Some(rate) = self.open_session().map(|s| f64::from(s.sample_rate_hz)) else {
            self.warn(format!(
                "{}: no audio session (a delay is applied in samples of its rate)",
                m.config.name
            ));
            return;
        };
        let by = match step {
            format::DelayStep::Samples(n) => n / rate,
            format::DelayStep::Seconds(s) => s,
        };
        // The delay and its offset from the arrival after the step, as the row and the
        // legend will show them: the daemon moves both by the step.
        let (applied, offset) = m
            .delay
            .as_ref()
            .map_or((by, by), |d| (d.applied.0 + by, d.nudged.0 + by));
        let what = format::delay_step_toast(&m.config.name, step, applied, offset);
        self.call(
            out,
            Command::DelayNudge {
                meas: m.id,
                by: Seconds(by),
            },
            what,
        );
    }

    pub(super) fn need_tf(&mut self) -> Option<Measurement> {
        self.need_meas(
            &[|k| matches!(k, MeasKind::Transfer { .. })],
            "transfer-function",
        )
    }

    /// Focuses `p` and selects the measurement it shows.
    /// Focuses pane `p` from the keyboard: it selects the measurement the pane shows,
    /// unless the selected stored trace is drawn there (the sweep chosen on the sweep pane
    /// stays selected on the way to the transfer pane).
    pub(super) fn focus(&mut self, p: PaneKind) {
        self.layout.shown[p.index()] = true;
        self.layout.focus = p;
        let keep = self
            .selected_trace_meta()
            .filter(|t| drawn_in(t, p))
            .map(|t| t.id);
        self.select_shown(p);
        if keep.is_some() {
            self.selected_trace = keep;
        }
        // Panes following a selection the pane cannot draw (no measurement of its kind):
        // the focus goes back to a kept pane, so say why the key did not move it.
        if self.follow_set().is_some() && !self.laid_out_panes().contains(&p) {
            self.toast(format!(
                "{}: no {} measurement to select · panes follow selection",
                p.title(),
                p.what()
            ));
        }
    }

    /// Selects the measurement pane `p` shows, if any.
    pub(super) fn select_shown(&mut self, p: PaneKind) {
        if let Some(id) = self.pane_meas(p).map(|m| m.id) {
            self.select(id);
        }
    }

    /// Selects `id` (deselecting a slot) and makes it what its pane shows.
    pub(super) fn select(&mut self, id: MeasId) {
        self.selected = Some(id);
        self.selected_trace = None;
        if let Some(p) = self.meas(id).map(|m| PaneKind::for_kind(&m.config.kind)) {
            self.pane_meas.insert(p, id);
        }
    }

    /// Pane `p` shows `id`; it gets the focus and `id` is selected.
    pub(super) fn pane_show(&mut self, p: PaneKind, id: MeasId) {
        if matches!(self.overlay, Overlay::PaneMenu(_)) {
            self.overlay = Overlay::None;
        }
        let Some(m) = self.meas(id) else {
            return;
        };
        if !p.shows(&m.config.kind) {
            return;
        }
        self.layout.shown[p.index()] = true;
        self.layout.focus = p;
        self.select(id);
    }

    /// The measurement list of pane `p`, the shown one highlighted.
    pub(super) fn pane_menu(&mut self, p: PaneKind) -> Overlay {
        let c = self.pane_candidates(p);
        if c.is_empty() {
            self.warn(format!("no {} measurements", p.what()));
            return Overlay::None;
        }
        let shown = self.pane_meas(p).map(|m| m.id);
        let index = c.iter().position(|m| Some(m.id) == shown).unwrap_or(0);
        Overlay::PaneMenu(PaneMenu { pane: p, index })
    }

    /// Sets the smoothing of what the smoothing keys act on (`step`: +1 coarser / -1 finer
    /// through [`SMOOTHING_STEPS`]; `to`: an explicit setting instead).
    pub(super) fn smooth(
        &mut self,
        step: i32,
        to: Option<Option<SmoothingFraction>>,
        out: &mut Vec<Request>,
    ) {
        let Some(target) = self.smooth_target() else {
            self.warn("no transfer or spectrum measurement to smooth");
            return;
        };
        let label = target.label();
        match target.kind() {
            Smoothable::Transfer | Smoothable::Spectrum => {}
            Smoothable::Rta => {
                self.toast(format!(
                    "{label}: RTA bands already are fractional-octave; smoothing applies to \
                     spectra and transfer functions"
                ));
                return;
            }
            Smoothable::No => {
                self.warn(format!(
                    "{label}: smoothing applies to transfer and spectrum curves only"
                ));
                return;
            }
        }
        let cur = target.smoothing();
        let want = match to {
            Some(f) => f,
            None => {
                let i = SMOOTHING_STEPS
                    .iter()
                    .position(|f| *f == cur.map(|s| s.fraction))
                    .unwrap_or(0) as i32;
                let j = (i + step).clamp(0, SMOOTHING_STEPS.len() as i32 - 1);
                if i == j {
                    self.toast(format!(
                        "{label}: {} is the {}",
                        target.caption(),
                        if step > 0 { "widest" } else { "finest" }
                    ));
                    return;
                }
                SMOOTHING_STEPS[j as usize]
            }
        };
        let new = want.map(|fraction| match target.kind() {
            Smoothable::Spectrum => spectrum_smoothing(fraction),
            // Phase is smoothed with the magnitude unless the curve was set to keep it.
            _ => Smoothing {
                fraction,
                mode: cur.map_or(SmoothingMode::MagnitudePhase, |s| s.mode),
            },
        });
        match target {
            SmoothTarget::Trace(t) => {
                if t.edit.locked {
                    self.warn(format!("{label} is locked"));
                    return;
                }
                let mut t = t.clone();
                t.edit.smoothing = new;
                let what = format!("{label}: {}", SmoothTarget::Trace(t.clone()).caption());
                let (trace, edit) = (t.id, t.edit);
                self.call(out, Command::TraceUpdate { trace, edit }, what);
            }
            SmoothTarget::Meas(mut m) => {
                match &mut m.config.kind {
                    MeasKind::Transfer { config } => config.smoothing = new,
                    MeasKind::Spectrum { config } => config.smoothing = want,
                    MeasKind::Math { config } => config.smoothing = new,
                    _ => {}
                }
                let what = format!("{label}: {}", SmoothTarget::Meas(m.clone()).caption());
                let (meas, config) = (m.id, m.config);
                self.call(out, Command::MeasUpdate { meas, config }, what);
            }
        }
    }

    pub(super) fn cycle_pane(&mut self, d: i32) {
        let vis = self.laid_out_panes();
        if vis.is_empty() {
            return;
        }
        let i = vis
            .iter()
            .position(|p| *p == self.layout.focus)
            .unwrap_or(0) as i32;
        let n = vis.len() as i32;
        self.focus(vis[((i + d).rem_euclid(n)) as usize]);
    }

    /// N / Shift+N: the next / previous measurement the focused pane can show.
    pub(super) fn cycle_meas(&mut self, d: i32) {
        let p = self.layout.focus;
        let ids: Vec<MeasId> = self.pane_candidates(p).iter().map(|m| m.id).collect();
        if ids.is_empty() {
            self.warn(format!("no {} measurements", p.what()));
            return;
        }
        let i = self
            .pane_meas(p)
            .and_then(|s| ids.iter().position(|x| *x == s.id))
            .map_or(if d > 0 { -1 } else { 0 }, |i| i as i32);
        self.select(ids[((i + d).rem_euclid(ids.len() as i32)) as usize]);
    }

    /// V / Shift+V: the next / previous shown stored trace in the list's order (slotted
    /// or not), with the live measurement as the stop between the last and the first;
    /// `hidden` (Alt) steps through the hidden ones too.
    pub(super) fn cycle_trace(&mut self, d: i32, hidden: bool) {
        let ids: Vec<TraceId> = self
            .trace_list()
            .into_iter()
            .filter(|t| hidden || t.edit.visible)
            .map(|t| t.id)
            .collect();
        if ids.is_empty() {
            self.warn(if hidden {
                "no stored traces (Ctrl+1 … 9 capture one)"
            } else {
                "no shown stored traces (1 … 9 show a slot; Alt+V reaches hidden traces)"
            });
            return;
        }
        // Position 0 is the live measurement, the k-th trace is k + 1. A selected trace off
        // the cycle (hidden) steps from its place in the whole list.
        let n = ids.len() as i32 + 1;
        let i = self.selected_trace.and_then(|s| {
            if let Some(i) = ids.iter().position(|x| *x == s) {
                return Some(i as i32 + 1);
            }
            let all: Vec<TraceId> = self.trace_list().iter().map(|t| t.id).collect();
            let at = all.iter().position(|x| *x == s)?;
            // The traces of the cycle before it: stepping forward lands on the next one.
            let before = all[..at].iter().filter(|x| ids.contains(x)).count() as i32;
            Some(if d > 0 { before } else { before + 1 })
        });
        match (i.unwrap_or(0) + d).rem_euclid(n) {
            0 => self.select_live(),
            k => {
                self.select_trace(Some(ids[(k - 1) as usize]));
                self.reveal_trace();
                if let Some(t) = self.selected_trace_meta() {
                    let hidden = if t.edit.visible { "" } else { " (hidden)" };
                    let label = trace_label(t);
                    self.toast(format!("{label}{hidden} selected"));
                }
            }
        }
    }

    /// Shows or hides stored trace `id`.
    pub(super) fn toggle_shown(&mut self, id: TraceId, out: &mut Vec<Request>) {
        let Some(t) = self
            .daemon()
            .and_then(|s| s.traces.iter().find(|t| t.id == id))
        else {
            return;
        };
        let mut edit = t.edit.clone();
        edit.visible = !edit.visible;
        let what = format!(
            "{} {}",
            trace_label(t),
            if edit.visible { "shown" } else { "hidden" }
        );
        self.call(out, Command::TraceUpdate { trace: id, edit }, what);
    }

    /// The selected stored trace when its curve is on the transfer pane, where the trace
    /// keys (U, J, `,` `.`, E) act on it; `Ok(None)`: they act on the live measurement. A
    /// locked trace is refused here, saying so.
    pub(super) fn transfer_trace_for_edit(&mut self) -> Result<Option<TraceMeta>, ()> {
        let Some(t) = self.selected_trace_meta().cloned() else {
            return Ok(None);
        };
        if !matches!(
            t.kind,
            TraceKind::Transfer | TraceKind::Target | TraceKind::Sweep
        ) {
            return Ok(None);
        }
        if t.edit.locked {
            self.warn(format!("{} is locked", trace_label(&t)));
            return Err(());
        }
        Ok(Some(t))
    }

    pub(super) fn select_live(&mut self) {
        self.select_trace(None);
        self.toast("keys act on the live measurement");
    }

    pub(super) fn call(&mut self, out: &mut Vec<Request>, cmd: Command, what: String) {
        out.push(Request::Call { cmd, what });
    }

    pub(super) fn command(&mut self, c: CommandId, keymap: &Keymap, out: &mut Vec<Request>) {
        use CommandId as C;
        match c {
            C::Help => {
                self.overlay = if self.overlay == Overlay::Help {
                    Overlay::None
                } else {
                    self.help_scroll = 0.0;
                    Overlay::Help
                };
            }
            C::Notifications => {
                self.overlay = if self.overlay == Overlay::Notifications {
                    Overlay::None
                } else {
                    self.help_scroll = 0.0;
                    Overlay::Notifications
                };
            }
            C::Palette => {
                self.overlay = if matches!(self.overlay, Overlay::Palette(_)) {
                    Overlay::None
                } else {
                    Overlay::Palette(Palette::default())
                };
            }
            C::Quit => self.quit = true,
            C::Fullscreen => self.fullscreen = !self.fullscreen,
            C::KeyHints => {
                self.prefs.key_hints = !self.prefs.key_hints;
                self.prefs_dirty = true;
                // Off, the line is gone: say how it comes back.
                if !self.prefs.key_hints {
                    let key = keymap
                        .first_chord(CommandId::KeyHints, Scope::Global)
                        .map_or_else(|| "the palette".to_owned(), |c| c.label());
                    self.toast(format!("key hints off · {key} shows them again"));
                }
            }

            C::PanesFollow => self.toggle_panes_follow(),

            C::StimulusArm => self.space(false, keymap, out),
            C::StimulusTakeOver => self.space(true, keymap, out),
            C::StimulusFire => match self.stimulus.phase {
                StimPhase::Armed if self.sweep.plan.is_some() => self.fire_sweep(out),
                StimPhase::Armed => {
                    if let Some(settings) = self.stimulus.settings() {
                        self.stimulus.phase = StimPhase::FireRequested;
                        out.push(Request::StimSet(GeneratorDesired {
                            settings,
                            armed: true,
                            firing: true,
                        }));
                    }
                }
                StimPhase::Idle => self.warn("not armed: Space arms first"),
                _ => {}
            },
            C::StimulusStop | C::StopAnywhere => {
                self.stimulus.arm_after_stop = false;
                self.sweep.arm_after_stop = false;
                if self.stimulus_live() {
                    self.stimulus.phase = StimPhase::Stopping;
                    out.push(Request::StimStop);
                }
            }
            C::LevelUp => self.step_level(1.0, out),
            C::LevelDown => self.step_level(-1.0, out),
            C::LevelUpCoarse => self.step_level(3.0, out),
            C::LevelDownCoarse => self.step_level(-3.0, out),
            C::StimulusLevel => {
                let text = self
                    .stimulus
                    .level
                    .map(|l| format::fixed(l.0, 1).replace(format::MINUS, "-"))
                    .unwrap_or_default();
                self.prompt(PromptKind::StimulusLevel, text);
            }
            C::StimulusOutputs => {
                self.open_settings(Page::Io, out);
                // On the stimulus output: the first ticked one, else the first output.
                if let Overlay::Settings(s) = &mut self.overlay {
                    s.focus_outputs = !s.focus_stimulus_outputs();
                }
            }
            C::Settings => {
                let page = self.settings_page_last;
                self.open_settings(page, out);
            }

            C::FocusTransfer => self.focus(PaneKind::Transfer),
            C::FocusSpectrum => self.focus(PaneKind::Spectrum),
            C::FocusIr => self.focus(PaneKind::Ir),
            C::FocusSpl => self.focus(PaneKind::Spl),
            C::FocusDistortion => self.focus(PaneKind::Distortion),
            C::SweepNew => self.open_sweep_dialog(keymap, out),
            C::DistortionUnit => {
                self.view.distortion.unit = match self.view.distortion.unit {
                    DistortionUnit::Db => DistortionUnit::Percent,
                    DistortionUnit::Percent => DistortionUnit::Db,
                };
            }
            C::SweepView => {
                self.view.distortion.mode = self.view.distortion.mode.next();
                self.focus(PaneKind::Distortion);
            }
            C::SweepIr => {
                self.view.distortion.mode = match self.view.distortion.mode {
                    SweepMode::Ir => SweepMode::Response,
                    _ => SweepMode::Ir,
                };
            }
            C::HideDistortion => {
                self.layout.shown[PaneKind::Distortion.index()] = false;
                if self.layout.focus == PaneKind::Distortion {
                    self.layout.focus = PaneKind::Transfer;
                }
            }
            C::NextPane => self.cycle_pane(1),
            C::PrevPane => self.cycle_pane(-1),
            C::MaximizePane => self.cycle_layout(),
            C::NextMeasurement if self.layout.focus == PaneKind::Distortion => {
                self.cycle_sweep(1);
            }
            C::PrevMeasurement if self.layout.focus == PaneKind::Distortion => {
                self.cycle_sweep(-1);
            }
            C::NextMeasurement => self.cycle_meas(1),
            C::PrevMeasurement => self.cycle_meas(-1),
            C::PaneMeasurement => self.overlay = self.pane_menu(self.layout.focus),
            C::NextTrace => self.cycle_trace(1, false),
            C::PrevTrace => self.cycle_trace(-1, false),
            C::NextAnyTrace => self.cycle_trace(1, true),
            C::PrevAnyTrace => self.cycle_trace(-1, true),
            C::ToggleSelected => self.toggle_selected(keymap, out),
            C::TraceSlot => match self.selected_trace_meta().cloned() {
                Some(t) => {
                    let text = t.edit.slot.map(|n| n.to_string()).unwrap_or_default();
                    self.prompt(PromptKind::TraceSlot(t.id), text);
                }
                None => self.warn(SELECT_TRACE_FIRST),
            },
            C::TraceExport => match self.selected_trace_meta().map(|t| t.id) {
                Some(id) => {
                    // The folder of the last export (else the home directory), ready for a
                    // file name.
                    let text = self
                        .export_dir
                        .as_ref()
                        .map(|d| {
                            let mut s = d.display().to_string();
                            if !s.ends_with(std::path::MAIN_SEPARATOR) {
                                s.push(std::path::MAIN_SEPARATOR);
                            }
                            s
                        })
                        .unwrap_or_default();
                    self.prompt(PromptKind::TraceExport(id), text);
                }
                None => self.warn(SELECT_TRACE_FIRST),
            },
            C::TraceRename => match self.selected_trace_meta().cloned() {
                Some(t) => self.prompt(PromptKind::TraceRename(t.id), t.edit.name.clone()),
                None => self.warn(SELECT_TRACE_FIRST),
            },
            C::SelectLive => self.select_live(),
            C::DeleteSelected => self.ask_delete(),
            C::SmoothCoarser => self.smooth(1, None, out),
            C::SmoothFiner => self.smooth(-1, None, out),
            C::SmoothOff => self.smooth(0, Some(None), out),
            C::Smooth48 => self.smooth(0, Some(Some(SmoothingFraction::FortyEighth)), out),
            C::Smooth24 => self.smooth(0, Some(Some(SmoothingFraction::TwentyFourth)), out),
            C::Smooth12 => self.smooth(0, Some(Some(SmoothingFraction::Twelfth)), out),
            C::Smooth6 => self.smooth(0, Some(Some(SmoothingFraction::Sixth)), out),
            C::Smooth3 => self.smooth(0, Some(Some(SmoothingFraction::Third)), out),
            C::CycleTheme => {
                self.theme = match self.theme {
                    ThemeName::Dark => ThemeName::Light,
                    ThemeName::Light => ThemeName::HighContrast,
                    ThemeName::HighContrast => ThemeName::Dark,
                };
                self.prefs.theme = Some(self.theme);
                self.prefs_dirty = true;
            }
            // An IR picture navigates its time and value axes with the same keys.
            c if ir_nav::is_nav(c) && self.ir_target().is_some() => {
                if let Some(p) = self.ir_target() {
                    self.ir_key(p, c);
                }
            }
            C::ZoomIn | C::ZoomOut => {
                let t = self.nav.target;
                let about = self
                    .view
                    .cursor_hz
                    .filter(|h| *h > t.lo && *h < t.hi)
                    .unwrap_or((t.lo * t.hi).sqrt());
                let f = if c == C::ZoomIn {
                    ZOOM_FACTOR
                } else {
                    1.0 / ZOOM_FACTOR
                };
                self.nav.set_target(t.zoom(about, f));
            }
            C::PanLeft | C::PanRight => {
                let d = if c == C::PanLeft {
                    -PAN_OCTAVES
                } else {
                    PAN_OCTAVES
                };
                let t = self.nav.target.pan(d);
                self.nav.set_target(t);
            }
            C::ResetView => self.nav.set_target(FreqRange::default()),
            C::LevelZoomIn
            | C::LevelZoomOut
            | C::LevelPanUp
            | C::LevelPanDown
            | C::LevelFit
            | C::LevelReset => {
                // Fit and reset frame the whole picture: frequency back to 20 Hz – 20 kHz
                // too, as Home alone does.
                if matches!(c, C::LevelFit | C::LevelReset) {
                    self.nav.set_target(FreqRange::default());
                }
                self.level_key(c)
            }
            C::ToggleCursor => {
                let t = self.nav.target;
                self.view.cursor_hz = match self.view.cursor_hz {
                    Some(_) => None,
                    None => Some((t.lo * t.hi).sqrt()),
                };
                // Off takes the spectrograph's time with it; on starts on frequency alone.
                self.view.spectrum.spectrograph.cursor_s = None;
            }
            C::CursorLeft | C::CursorRight => {
                let t = self.nav.target;
                let k = if c == C::CursorLeft { -1.0 } else { 1.0 };
                let hz = self.view.cursor_hz.unwrap_or((t.lo * t.hi).sqrt());
                let hz = (hz * 2f64.powf(k / 12.0)).clamp(t.lo, t.hi);
                self.view.cursor_hz = Some(hz);
            }
            C::Slot1
            | C::Slot2
            | C::Slot3
            | C::Slot4
            | C::Slot5
            | C::Slot6
            | C::Slot7
            | C::Slot8
            | C::Slot9 => {
                let slot = slot_of(c);
                if let Some(m) = self.need_meas(
                    &[MeasKind::publishes_tf, MeasKind::publishes_levels],
                    "transfer, spectrum, RTA or math",
                ) {
                    // The slot's previous trace is replaced unless it is locked; a locked
                    // one only gives up the slot.
                    let replace = self.slots()[usize::from(slot - 1)]
                        .filter(|t| !t.edit.locked)
                        .map(|t| t.id);
                    out.push(Request::Capture {
                        meas: m.id,
                        slot,
                        name: format!("{} S{slot}", m.config.name),
                        replace,
                    });
                }
            }
            C::ShowSlot1
            | C::ShowSlot2
            | C::ShowSlot3
            | C::ShowSlot4
            | C::ShowSlot5
            | C::ShowSlot6
            | C::ShowSlot7
            | C::ShowSlot8
            | C::ShowSlot9 => {
                let slot = slot_of(c);
                match self.slots()[usize::from(slot - 1)].map(|t| t.id) {
                    None => self.warn(format!(
                        "slot {slot} is empty (Ctrl+{slot} captures into it)"
                    )),
                    Some(id) => self.toggle_shown(id, out),
                }
            }
            C::ImportTrace => self.prompt(PromptKind::ImportFile(ImportRole::Trace), String::new()),
            C::SessionSave => self.prompt(PromptKind::SessionSave, String::new()),
            C::SessionLoad => self.prompt(PromptKind::SessionLoad, String::new()),
            C::Reconnect => out.push(Request::Reconnect),
            C::OpenSession => {
                if !self.connected() {
                    self.warn("not connected");
                } else {
                    self.open_settings(Page::Audio, out);
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
                        let p = match self.layout.focus {
                            PaneKind::Spectrum => PaneKind::Spectrum,
                            _ => PaneKind::Transfer,
                        };
                        self.pane_meas(p).map(|m| Operand::Meas { meas: m.id })
                    });
                match Form::math(self.math_candidates(Some(owner)), first, owner) {
                    Ok(f) => self.overlay = Overlay::Form(Box::new(f)),
                    Err(e) => self.warn(e),
                }
            }
            C::EditMeas => {
                // A sweep measurement selected (or the sweep pane's) edits its settings.
                let sweep = self
                    .selected_meas()
                    .filter(|m| matches!(m.config.kind, MeasKind::Sweep { .. }))
                    .or_else(|| {
                        (self.layout.focus == PaneKind::Distortion)
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
                let m = self
                    .selected_meas()
                    .filter(|m| matches!(m.config.kind, MeasKind::Math { .. }))
                    .or_else(|| {
                        [PaneKind::Transfer, PaneKind::Spectrum]
                            .into_iter()
                            .filter_map(|p| self.pane_meas(p))
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
                    None => self.warn("select a math channel or a sweep measurement first"),
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
                None => self.warn("select a measurement first (click it in the list, N)"),
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
                Some(_) => {
                    let page = if c == C::InputSetup {
                        Page::Io
                    } else {
                        Page::Calibration
                    };
                    self.open_settings(page, out);
                }
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

            C::Freeze => {
                if let Some(m) = self.need_meas(
                    &[MeasKind::publishes_tf, MeasKind::publishes_levels],
                    "transfer or spectrum",
                ) {
                    let frozen = !m.frozen;
                    let what = if frozen { "frozen" } else { "unfrozen" };
                    self.call(
                        out,
                        Command::MeasFreeze { meas: m.id, frozen },
                        format!("{} {what}", m.config.name),
                    );
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

            C::InsertDelay | C::InsertStrongest => {
                if let Some(m) = self.need_tf() {
                    let pick = if c == C::InsertDelay {
                        DelayPick::FirstArrival
                    } else {
                        DelayPick::Strongest
                    };
                    if matches!(self.overlay, Overlay::DelayPick(_)) {
                        self.overlay = Overlay::None;
                    }
                    out.push(Request::FindDelay {
                        meas: m.id,
                        pick,
                        band: self.finder.band,
                        observation: self.finder.observation,
                    });
                }
            }
            C::TypeDelay => {
                if let Some(m) = self.need_tf() {
                    let text = m
                        .delay
                        .as_ref()
                        .map(|d| format::delay_entry_ms(d.applied.0))
                        .unwrap_or_default();
                    self.prompt(PromptKind::Delay(m.id), text);
                }
            }
            C::DelayDown | C::DelayUp | C::DelayDownFine | C::DelayUpFine => {
                let step = match c {
                    C::DelayDown => -1.0,
                    C::DelayUp => 1.0,
                    C::DelayDownFine => -DELAY_FINE_STEP,
                    _ => DELAY_FINE_STEP,
                };
                self.step_delay(format::DelayStep::Samples(step), out);
            }
            C::TrackDelay => {
                if let Some(m) = self.need_tf() {
                    let enabled = !m.delay.as_ref().is_some_and(|d| d.tracking);
                    let what = if enabled { "on" } else { "off" };
                    self.call(
                        out,
                        Command::DelayTrack {
                            meas: m.id,
                            enabled,
                        },
                        format!("{}: tracking {what}", m.config.name),
                    );
                }
            }
            C::Invert => match self.transfer_trace_for_edit() {
                Err(()) => {}
                Ok(Some(t)) if t.kind == TraceKind::Target => {
                    self.warn(format!("{}: a target curve has no phase", trace_label(&t)));
                }
                Ok(Some(t)) => {
                    let mut edit = t.edit.clone();
                    edit.polarity = match edit.polarity {
                        Polarity::Normal => Polarity::Inverted,
                        Polarity::Inverted => Polarity::Normal,
                    };
                    let what = format!(
                        "{}: polarity {}",
                        trace_label(&t),
                        match edit.polarity {
                            Polarity::Normal => "normal",
                            Polarity::Inverted => "inverted",
                        }
                    );
                    self.call(out, Command::TraceUpdate { trace: t.id, edit }, what);
                }
                Ok(None) => {
                    if let Some(m) = self.need_tf() {
                        let e = self.edits.entry(m.id).or_default();
                        e.inverted = !e.inverted;
                    }
                }
            },
            C::Offset => self.offset_prompt(),
            C::OffsetUp => self.step_offset(Some(1.0), out),
            C::OffsetDown => self.step_offset(Some(-1.0), out),
            C::OffsetUpCoarse => self.step_offset(Some(3.0), out),
            C::OffsetDownCoarse => self.step_offset(Some(-3.0), out),
            C::OffsetClear => self.step_offset(None, out),
            C::NudgeEarlier | C::NudgeLater => {
                let d = if c == C::NudgeEarlier {
                    -NUDGE_S
                } else {
                    NUDGE_S
                };
                // A trace's nudge in whole steps: repeated steps never accumulate float
                // error.
                let step = |v: f64| ((v + d) / NUDGE_S).round() * NUDGE_S;
                match self.transfer_trace_for_edit() {
                    Err(()) => {}
                    Ok(Some(t)) if t.kind == TraceKind::Target => {
                        self.warn(format!("{}: a target curve has no phase", trace_label(&t)));
                    }
                    Ok(Some(t)) => {
                        let mut edit = t.edit.clone();
                        edit.delay_nudge = Seconds(step(edit.delay_nudge.0));
                        let what =
                            format::trace_delay_toast(&trace_label(&t), d, edit.delay_nudge.0);
                        self.call(out, Command::TraceUpdate { trace: t.id, edit }, what);
                    }
                    // A measurement has one delay, the daemon's: the plain keys step it too.
                    Ok(None) => self.step_delay(format::DelayStep::Seconds(d), out),
                }
            }
            C::PhaseReference => match self.transfer_trace_for_edit() {
                Err(()) => {}
                Ok(Some(t)) if t.kind == TraceKind::Target => {
                    self.warn(format!("{}: a target curve has no phase", trace_label(&t)));
                }
                Ok(Some(t)) => {
                    self.view.tf.phase_reference = Some(TraceKey::Stored(t.id));
                    self.toast(format!("phase reference: {}", trace_label(&t)));
                }
                Ok(None) => {
                    if let Some(m) = self.need_tf() {
                        self.view.tf.phase_reference = Some(TraceKey::Live(m.id));
                        self.toast(format!("phase reference: {}", m.config.name));
                    }
                }
            },
            C::Target => self.prompt(PromptKind::ImportFile(ImportRole::Target), String::new()),
            C::Average => self.average(AverageMethod::Power, out),
            C::AverageComplex => self.average(AverageMethod::Complex, out),
            C::AverageCoherence => self.average(AverageMethod::CoherenceWeighted, out),
            C::ToggleIr => {
                let i = PaneKind::Ir.index();
                self.layout.shown[i] = !self.layout.shown[i];
                if !self.layout.shown[i] && self.layout.focus == PaneKind::Ir {
                    self.layout.focus = PaneKind::Transfer;
                }
            }
            C::CoherenceMask => {
                let cur = self.view.tf.coherence.blank_below;
                let i = COHERENCE_MASKS.iter().position(|m| *m == cur).unwrap_or(0);
                let next = COHERENCE_MASKS[(i + 1) % COHERENCE_MASKS.len()];
                self.view.tf.coherence.blank_below = next;
                self.toast(match next {
                    None => "coherence mask off".to_string(),
                    Some(t) => format!("coherence mask: hide γ² < {t:.1}"),
                });
            }
            C::CoherencePlacement => {
                self.view.tf.coherence_placement = match self.view.tf.coherence_placement {
                    CoherencePlacement::Pane => CoherencePlacement::OverlayOnMagnitude,
                    CoherencePlacement::OverlayOnMagnitude => CoherencePlacement::Pane,
                };
            }
            C::PhaseUnwrap => {
                self.view.tf.phase = match self.view.tf.phase {
                    PhaseView::Wrapped => PhaseView::Unwrapped {
                        range: Range::new(-720.0, 180.0),
                    },
                    _ => PhaseView::Wrapped,
                };
            }
            C::GroupDelay => {
                self.view.tf.phase = match self.view.tf.phase {
                    PhaseView::GroupDelay { .. } => PhaseView::Wrapped,
                    _ => PhaseView::GroupDelay {
                        range_ms: Range::new(-2.0, 10.0),
                    },
                };
            }
            C::SpectrumStyle => {
                self.view.spectrum.style = match self.view.spectrum.style {
                    SpectrumStyle::Bars => SpectrumStyle::Line,
                    SpectrumStyle::Line => SpectrumStyle::Bars,
                };
            }
            C::PeakHold => {
                self.view.spectrum.peak_hold = !self.view.spectrum.peak_hold;
                self.peaks.clear();
            }
            C::Spectrograph => {
                let before = self.view.spectrum.mode;
                let mode = before.next();
                self.view.spectrum.mode = mode;
                // The history starts with the view and goes with it: nothing is kept for a
                // hidden spectrograph; from the split to the spectrograph alone it stays.
                if !mode.spectrograph() {
                    self.spectrographs.clear();
                    self.view.spectrum.spectrograph.cursor_s = None;
                } else if !before.spectrograph()
                    && let Some(d) = self.data.clone()
                {
                    self.fold_spectrographs(&d);
                }
                self.focus(PaneKind::Spectrum);
            }
            C::SpectrographSpan => {
                let sg = &mut self.view.spectrum.spectrograph;
                sg.span_s = ac2_scene::view::SpectrographView::next_span(sg.span_s);
                sg.cursor_s = sg.cursor_s.filter(|t| *t <= f64::from(sg.span_s));
                let span = sg.span_s;
                // Slots of another length cannot hold the frames already placed.
                self.spectrographs.clear();
                self.toast(format!("spectrograph: last {span} s"));
            }
            C::IrMode => {
                self.view.ir.mode = match self.view.ir.mode {
                    IrMode::Linear => IrMode::Log,
                    IrMode::Log => IrMode::Etc,
                    IrMode::Etc => IrMode::Linear,
                };
            }
            C::SplLeqView
            | C::SplShowMeter
            | C::SplShowLeq
            | C::SplShowMeterLeq
            | C::SplShowBands => {
                let bands = crate::scenes::has_band_meter(self);
                self.view.spl.mode = match c {
                    C::SplShowMeter => SplMode::Meter,
                    C::SplShowLeq => SplMode::Leq,
                    C::SplShowMeterLeq => SplMode::MeterLeq,
                    C::SplShowBands => SplMode::Bands,
                    _ => self.view.spl.mode.next(bands),
                };
                self.focus(PaneKind::Spl);
            }
            // Either shows the windows (a layout change is about them) and is remembered.
            C::SplLeqStyle | C::SplLeqHistory => {
                let l = &mut self.view.spl.layout;
                if c == C::SplLeqStyle {
                    l.style = match l.style {
                        LeqStyle::Columns => LeqStyle::Tiles,
                        LeqStyle::Tiles => LeqStyle::Columns,
                    };
                } else {
                    l.history = !l.history;
                }
                self.view.spl.mode = self.view.spl.mode.with_leq();
                self.focus(PaneKind::Spl);
                self.prefs.leq = self.view.spl.layout;
                self.prefs_dirty = true;
            }
            C::SplNewLog => match self.pane_meas(PaneKind::Spl).cloned() {
                Some(m) => {
                    let MeasKind::Spl { config } = &m.config.kind else {
                        return;
                    };
                    let zone = self.local_zone;
                    let run = self
                        .leq_run(m.id)
                        .map(|r| ac2_scene::leq::run_text(&r, &config.leq, |t| zone.offset_s(t)));
                    self.overlay = Overlay::NewLog(Box::new(NewLogPrompt {
                        meas: m.id,
                        meter: m.config.name.clone(),
                        confirm: ac2_scene::leq::new_log_confirm(
                            &m.config.name,
                            &config.leq,
                            run.as_ref(),
                        ),
                    }));
                }
                None => self.warn("no SPL meter: make one first (New SPL meter… in Ctrl+K)"),
            },
            C::SplTimeWeighting
            | C::SplWeighting
            | C::SplFast
            | C::SplSlow
            | C::SplImpulse
            | C::SplA
            | C::SplC
            | C::SplZ => self.spl_weightings(c, out),
            C::LeqWindows => match self.pane_meas(PaneKind::Spl) {
                Some(_) => self.open_settings(Page::Leq, out),
                None => self.warn("no SPL meter: make one first (New SPL meter… in Ctrl+K)"),
            },
        }
    }
}
