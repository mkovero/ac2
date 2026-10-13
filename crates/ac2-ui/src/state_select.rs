//! Selection steps: the selected trace, tree rows, and what each pane shows.

use super::*;

impl AppState {
    /// The one trace selection: a sweep selected is also what the sweep pane shows.
    /// A transfer-like trace selected brings its measurement's group to the transfer pane,
    /// which draws only that group: a selected curve the pane leaves out would take the
    /// trace keys while nothing on screen moves.
    pub(super) fn select_trace(&mut self, id: Option<TraceId>) {
        self.set_selected_trace(id);
        let owner = self
            .selected_trace_meta()
            .filter(|t| drawn_in(t, PaneKind::Transfer))
            .and_then(|t| match t.edit.owner {
                TraceOwner::Meas { meas } => Some(meas),
                TraceOwner::Imported => None,
            })
            .filter(|m| {
                self.meas(*m)
                    .is_some_and(|m| PaneKind::Transfer.shows(&m.config.kind))
            });
        // The transfer pane worked in last shows the trace's measurement, so the trace is
        // drawn in its group.
        if let Some(m) = owner
            && let Some(p) = self.layout.lead(PaneKind::Transfer)
            && let Some(v) = self.layout.view_mut(p)
        {
            v.meas = Some(m);
        }
    }

    /// The trace selection alone, every pane keeping what it shows: a sweep selected is
    /// also what the sweep pane shows.
    fn set_selected_trace(&mut self, id: Option<TraceId>) {
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
                let to_it = self.to_it();
                let new = self.keys.palette_entry("New …");
                self.warn(format!(
                    "select a {what} measurement first ({to_it}; {new} makes one)"
                ));
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

    /// G / Shift+G: the focused pane's next or previous view, in its kind's chain.
    pub(super) fn step_view(&mut self, forward: bool) {
        let kind = self.layout.focus_kind();
        let before = self.spectrograph_shown();
        let bands = crate::scenes::has_band_meter(self);
        let m = self.modes_mut();
        match (kind, forward) {
            (PaneKind::Transfer, true) => m.transfer = m.transfer.next(),
            (PaneKind::Transfer, false) => m.transfer = m.transfer.prev(),
            (PaneKind::Spectrum, true) => m.spectrum = m.spectrum.next(),
            (PaneKind::Spectrum, false) => m.spectrum = m.spectrum.prev(),
            (PaneKind::Spl, true) => m.spl = m.spl.next(bands),
            (PaneKind::Spl, false) => m.spl = m.spl.prev(bands),
            (PaneKind::Distortion, true) => m.sweep = m.sweep.next(),
            (PaneKind::Distortion, false) => m.sweep = m.sweep.prev(),
        }
        // The history starts with the view and goes with the last pane showing it: nothing
        // is kept for a hidden spectrograph; from the split to the spectrograph alone it
        // stays.
        if kind == PaneKind::Spectrum {
            if !self.spectrograph_shown() {
                self.spectrographs.clear();
                self.view.spectrum.spectrograph.cursor_s = None;
            } else if !before && let Some(d) = self.data.clone() {
                self.fold_spectrographs(&d);
            }
        }
    }

    /// A click on a legend row of pane `p`: selects exactly the curve it names — a stored
    /// trace, else the measurement (or math channel) whose live curve it is, as its row in
    /// the list does — so the keys of the selection (A hides it) act on it. The pane keeps
    /// what it shows: the curve is on it already, and a compared curve of another
    /// measurement selected must not swap the pane's curves under the pointer.
    pub(super) fn legend_pick(&mut self, p: PaneId, key: ac2_scene::trace::TraceKey) {
        use ac2_scene::trace::TraceKey;
        self.layout.set_focus(p);
        match key {
            TraceKey::Stored(id) => {
                if self.trace_meta(id).is_ok() {
                    self.set_selected_trace(Some(id));
                }
            }
            TraceKey::Live(id) => {
                if let Some(m) = self.meas(id) {
                    // A math channel's curve is its own row; a measurement's is its live row.
                    let live = ac2_scene::meas_list::has_live_curve(&m.config.kind);
                    self.selected = Some(id);
                    self.selected_trace = None;
                    self.selected_live = live.then_some(id);
                }
            }
        }
    }

    /// Selects the measurement pane `p` shows, if any, leaving the pane's choice as it is.
    pub(super) fn select_shown(&mut self, p: PaneId) {
        if let Some(id) = self.pane_meas(p).map(|m| m.id) {
            self.selected = Some(id);
            self.selected_trace = None;
            self.selected_live = None;
        }
    }

    /// A measurement's row in the tree chosen (click, Enter, Tab): selected, and its own
    /// panes on screen when it has them; else the focused pane shows it, turning into the
    /// kind of pane that measurement belongs in. The operator's eyes are on the focused
    /// pane: the measurement goes where they look, and every other pane keeps what it shows.
    pub(super) fn select_meas_row(&mut self, id: MeasId) {
        let Some(kind) = self.meas(id).map(|m| PaneKind::for_kind(&m.config.kind)) else {
            return;
        };
        let f = self.layout.focus;
        self.set_pane_kind(f, kind);
        self.select_on(f, id);
        self.own_panes_due = true;
    }

    /// A child row of the tree chosen (a live curve, a math channel, a stored trace's
    /// measurement): the focused pane shows measurement `id`, turning into a kind that draws
    /// it when it cannot. Only that pane changes: the focus stays and no measurement's own
    /// panes come, so a pane can be given another curve without the others moving.
    pub(super) fn focused_pane_shows(&mut self, id: MeasId) {
        let Some(m) = self.meas(id) else {
            return;
        };
        let f = self.layout.focus;
        if !self.layout.kind(f).shows(&m.config.kind) {
            let kind = PaneKind::for_kind(&m.config.kind);
            self.set_pane_kind(f, kind);
        }
        if let Some(v) = self.layout.view_mut(f) {
            v.meas = Some(id);
        }
    }

    /// A measurement's live-curve row chosen (click, Tab): a child row, shown by the
    /// focused pane; the live curve is what the keys act on, as a click on its legend row
    /// makes it.
    pub(super) fn select_live_row(&mut self, id: MeasId) {
        if self.meas(id).is_none() {
            return;
        }
        self.focused_pane_shows(id);
        self.selected = Some(id);
        self.selected_trace = None;
        self.selected_live = Some(id);
    }

    /// A math channel's row chosen (click, Tab): a child row of its owner, shown by the
    /// focused pane; the channel is what the keys act on.
    pub(super) fn select_math_row(&mut self, id: MeasId) {
        if self.meas(id).is_none() {
            return;
        }
        self.focused_pane_shows(id);
        self.selected = Some(id);
        self.selected_trace = None;
        self.selected_live = None;
    }

    /// A stored trace's row chosen (click, Tab): a child row, its measurement shown by the
    /// focused pane (turned into the kind that draws the trace), the trace selected; the
    /// measurement selection stays.
    pub(super) fn select_trace_row(&mut self, id: TraceId) {
        let Some(t) = self
            .daemon()
            .and_then(|s| s.traces.iter().find(|t| t.id == id))
            .cloned()
        else {
            return;
        };
        let f = self.layout.focus;
        if !drawn_in(&t, self.layout.kind(f)) {
            self.set_pane_kind(f, display::home_pane(t.kind));
        }
        if let TraceOwner::Meas { meas } = ac2_scene::meas_list::group_of(&t, &self.measurements())
            && self
                .meas(meas)
                .is_some_and(|m| self.layout.kind(f).shows(&m.config.kind))
            && let Some(v) = self.layout.view_mut(f)
        {
            v.meas = Some(meas);
        }
        self.select_trace(Some(id));
    }

    /// Tab / Shift+Tab: the next / previous row of the measurement list as it is shown
    /// (a folded group's rows skipped, hidden curves included: they are selectable),
    /// wrapping. A measurement's header and its live curve are two stops, as they are two
    /// rows: the live curve is selected as a click on its row (or legend row) selects it.
    /// A measurement row is chosen as a click on it is; a stored trace row selects the
    /// trace as a click does, the focused pane first showing its measurement.
    pub(super) fn cycle_tree_row(&mut self, d: i32) {
        use ac2_scene::meas_list::TreeKey;
        #[derive(Clone, Copy, PartialEq)]
        enum Stop {
            Meas(MeasId),
            Math(MeasId),
            Live(MeasId),
            Trace(TraceId),
        }
        let stop = |k: TreeKey| match k {
            TreeKey::Meas(id) => Some(Stop::Meas(id)),
            TreeKey::Math(id) => Some(Stop::Math(id)),
            TreeKey::Live(id) => Some(Stop::Live(id)),
            TreeKey::Trace(id) => Some(Stop::Trace(id)),
            TreeKey::Imported => None,
        };
        let stops = |rows: Vec<ac2_scene::meas_list::TreeRow>| {
            let mut v: Vec<Stop> = Vec::new();
            for s in rows.into_iter().filter_map(|r| stop(r.key)) {
                if v.last() != Some(&s) {
                    v.push(s);
                }
            }
            v
        };
        // Every row, folded ones too, places a selection inside a folded group; the step
        // lands on the next row the list shows.
        let all = stops(self.tree_rows_folded(&std::collections::BTreeSet::new()));
        let shown = stops(self.tree_rows());
        if shown.is_empty() {
            self.warn("no measurements to step through");
            return;
        }
        let at = match self.selected_trace {
            Some(t) => Some(Stop::Trace(t)),
            None if self.live_row_selected() => self.selected.map(Stop::Live),
            None => self.selected.map(Stop::Meas),
        }
        .and_then(|s| {
            all.iter().position(|x| {
                *x == s || matches!((s, *x), (Stop::Meas(a), Stop::Math(b)) if a == b)
            })
        });
        let n = all.len() as i32;
        let next = match at {
            Some(i) => (1..=n)
                .map(|k| all[(i as i32 + d * k).rem_euclid(n) as usize])
                .find(|s| shown.contains(s)),
            None if d > 0 => shown.first().copied(),
            None => shown.last().copied(),
        };
        match next {
            Some(Stop::Meas(id)) => self.select_meas_row(id),
            Some(Stop::Math(id)) => self.select_math_row(id),
            Some(Stop::Live(id)) => self.select_live_row(id),
            Some(Stop::Trace(id)) => {
                self.select_trace_row(id);
                self.reveal_trace();
            }
            None => {}
        }
        self.tree_reveal = true;
    }

    /// Pane `p` shows `id`; it gets the focus and `id` is selected. A pane that cannot draw
    /// it turns into the kind of pane it belongs in; one that can keeps its kind (a sweep
    /// picked on the transfer pane has its runs drawn there).
    pub(super) fn pane_show(&mut self, p: PaneId, id: MeasId) {
        if matches!(self.overlay, Overlay::PaneMenu(_)) {
            self.overlay = Overlay::None;
        }
        let Some(m) = self.meas(id) else {
            return;
        };
        if self.layout.view(p).is_none() {
            return;
        }
        if !self.layout.kind(p).shows(&m.config.kind) {
            let kind = PaneKind::for_kind(&m.config.kind);
            self.set_pane_kind(p, kind);
        }
        self.layout.set_focus(p);
        self.select_on(p, id);
    }

    /// Selects `id` as what pane `p` shows: a sweep picked on the transfer pane is that
    /// pane's (its runs drawn there).
    pub(super) fn select_on(&mut self, p: PaneId, id: MeasId) {
        // A pane closed since (a stale menu or click) has nothing to show it on.
        if self.layout.view(p).is_none() {
            return;
        }
        self.selected = Some(id);
        self.selected_trace = None;
        self.selected_live = None;
        if let Some(v) = self.layout.view_mut(p) {
            v.meas = Some(id);
        }
    }
}
