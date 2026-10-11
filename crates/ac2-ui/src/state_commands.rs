//! Key commands ([`CommandId`]): the app, stimulus, pane, trace and view steps; the rest go
//! on to `commands_meas` and `commands_tf`.

use super::*;

impl AppState {
    /// The list of pane `p`, the shown measurement highlighted.
    pub(super) fn pane_menu(&mut self, p: PaneId) -> Overlay {
        let rows = self.pane_menu_rows(p);
        let shown = self.pane_meas(p).map(|m| PaneMenuRow::Meas(m.id));
        let index = rows
            .iter()
            .position(|(r, _)| Some(*r) == shown)
            .unwrap_or(0);
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
        let mut what = format!(
            "{} {}",
            trace_label(t),
            if edit.visible { "shown" } else { "hidden" }
        );
        // The transfer pane draws one group: a trace shown outside it says where it is.
        let shown = TraceMeta {
            edit: edit.clone(),
            ..t.clone()
        };
        if edit.visible
            && drawn_in(&shown, PaneKind::Transfer)
            && !self.on_transfer_pane(&shown, self.kind_meas(PaneKind::Transfer))
            && !self.compared_traces.contains(&shown.id)
        {
            let ms = self.measurements();
            what.push_str(&match ac2_scene::meas_list::group_of(&shown, &ms) {
                TraceOwner::Meas { meas } => format!(
                    " · on the transfer pane with {}",
                    self.meas(meas)
                        .map_or("its measurement", |m| &m.config.name)
                ),
                TraceOwner::Imported => {
                    " · under Imported: compare it (C) or Move to measurement… to see it".to_owned()
                }
            });
        }
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

    /// Steps the selected measurement's colour family (`meas.colour`) to the next one no
    /// other measurement has.
    pub(super) fn step_colour(&mut self, forward: bool, out: &mut Vec<Request>) {
        let Some(m) = self.selected_meas().cloned() else {
            self.warn("select a measurement first (N)");
            return;
        };
        if matches!(m.config.kind, MeasKind::Math { .. }) {
            self.warn(format!(
                "{} is a math channel: it takes its measurement's colours",
                m.config.name
            ));
            return;
        }
        let Some(colour) = ac2_scene::families::step_colour(&self.measurements(), m.id, forward)
        else {
            self.warn("every other colour is another measurement's");
            return;
        };
        self.call(
            out,
            Command::MeasColour {
                meas: m.id,
                colour: Some(colour),
            },
            format!(
                "{}: {}",
                m.config.name,
                ac2_scene::format::colour_family(colour)
            ),
        );
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
            C::Quit => self.request_quit(),
            C::Fullscreen => self.cycle_fullscreen(),
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

            C::WarningToasts => self.toggle_warning_toasts(),

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
                self.open_settings(Page::AudioIo, out);
                if let Overlay::Settings(s) = &mut self.overlay {
                    s.focus_channels(FocusWant::StimulusOutputs);
                }
            }
            C::Settings => {
                let page = self.settings_page_last;
                self.open_settings(page, out);
            }

            C::FocusPane1 => self.focus_nth(1),
            C::FocusPane2 => self.focus_nth(2),
            C::FocusPane3 => self.focus_nth(3),
            C::FocusPane4 => self.focus_nth(4),
            C::FocusPane5 => self.focus_nth(5),
            C::FocusPane6 => self.focus_nth(6),
            C::FocusPane7 => self.focus_nth(7),
            C::FocusPane8 => self.focus_nth(8),
            C::FocusPane9 => self.focus_nth(9),
            C::SplitPane => self.split_pane(),
            C::ClosePane => self.close_pane(),
            C::TurnSplit => self.turn_split(),
            C::TransferStack => self.transfer_stack(),
            C::GrowPane => self.resize_pane(true),
            C::ShrinkPane => self.resize_pane(false),
            C::SweepNew => self.open_sweep_dialog(keymap, out),
            C::DistortionUnit => self.toggle_distortion_unit(),
            C::NextView => self.step_view(true),
            C::PrevView => self.step_view(false),
            C::SweepIr => {
                let m = self.modes_mut();
                m.sweep = match m.sweep {
                    SweepMode::Ir(_) => SweepMode::Response,
                    _ => SweepMode::Ir(IrMode::Linear),
                };
            }
            C::MaximizePane => self.cycle_layout(),
            C::NextInTree => self.cycle_tree_row(1),
            C::PrevInTree => self.cycle_tree_row(-1),
            C::PaneMeasurement => self.overlay = self.pane_menu(self.layout.focus),
            // The sweep pane draws one run at a time: V steps through the runs, never onto a
            // curve the pane cannot draw.
            C::NextTrace if self.layout.focus_kind() == PaneKind::Distortion => {
                self.cycle_sweep(1);
            }
            C::PrevTrace if self.layout.focus_kind() == PaneKind::Distortion => {
                self.cycle_sweep(-1);
            }
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
            C::PlotChrome => {
                let title = self.layout.focused().title();
                let c = &mut self.modes_mut().chrome;
                *c = c.next();
                let name = c.name();
                self.toast(format!("{title}: {name}"));
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
            C::Compare => self.toggle_compare(),
            C::ClearCompare => self.clear_compare(),
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
            c @ (C::ImportTrace
            | C::Sessions
            | C::SessionNew
            | C::SessionSave
            | C::SessionSaveAs
            | C::SessionLoad
            | C::SessionExport
            | C::Reconnect
            | C::OpenSession
            | C::Record
            | C::ReplayRecording
            | C::CloseSession
            | C::NewTransfer
            | C::NewSpectrum
            | C::NewRta
            | C::NewSpl
            | C::NewMath
            | C::NextColour
            | C::PrevColour
            | C::EditMeas
            | C::HideGroup
            | C::MoveTrace
            | C::ToggleGroup
            | C::InputMics
            | C::TraceMicCurve
            | C::CalDelete
            | C::InputSetup
            | C::Calibrations
            | C::MicCurveInput
            | C::FinderAuto
            | C::FinderFull
            | C::FinderMid
            | C::FinderSub
            | C::FinderCustom
            | C::FinderObservation
            | C::MicCurve
            | C::ResetAverage
            | C::StartStop) => self.command_meas(c, keymap, out),
            c @ (C::InsertDelay
            | C::InsertStrongest
            | C::TypeDelay
            | C::DelayDown
            | C::DelayUp
            | C::DelayDownFine
            | C::DelayUpFine
            | C::TrackDelay
            | C::Invert
            | C::Offset
            | C::OffsetUp
            | C::OffsetDown
            | C::OffsetUpCoarse
            | C::OffsetDownCoarse
            | C::OffsetClear
            | C::NudgeEarlier
            | C::NudgeLater
            | C::PhaseReference
            | C::Target
            | C::Average
            | C::AverageComplex
            | C::AverageCoherence
            | C::CoherenceMask
            | C::CoherencePlacement
            | C::LegendTopLeft
            | C::LegendTopRight
            | C::LegendBottomLeft
            | C::LegendBottomRight
            | C::LegendHide
            | C::PhaseUnwrap
            | C::GroupDelay
            | C::SpectrumStyle
            | C::PeakHold
            | C::SpectrographSpan
            | C::SplShowMeter
            | C::SplShowLeq
            | C::SplShowMeterLeq
            | C::SplShowBands
            | C::SplLeqStyle
            | C::SplLeqHistory
            | C::SplNewLog
            | C::SplTimeWeighting
            | C::SplWeighting
            | C::SplFast
            | C::SplSlow
            | C::SplImpulse
            | C::SplA
            | C::SplC
            | C::SplZ
            | C::LeqWindows) => self.command_tf(c, out),
        }
    }
}
