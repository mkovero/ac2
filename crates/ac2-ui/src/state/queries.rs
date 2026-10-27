//! Read-only views of the state for the scenes and the app (measurements, traces, captions, meters, hints), and the remembered pane layout.

pub(super) mod inputs;
pub(super) mod layout;
pub(super) mod select;

use super::*;

impl AppState {
    /// The active rig's mirrored daemon state ([`RigState::daemon`]).
    pub fn daemon(&self) -> Option<&State> {
        self.rig.daemon()
    }

    /// The active rig's measurements, by id.
    pub fn measurements(&self) -> Vec<&Measurement> {
        self.rig.measurements()
    }

    pub fn meas(&self, id: MeasId) -> Option<&Measurement> {
        self.rig.meas(id)
    }

    /// `meas.update` of `meas` to `config`, made on the configuration the mirror holds (the
    /// daemon refuses it with `conflict` when another client changed it since). The
    /// measurement stays the session's or the rig's, as it is.
    pub fn meas_update(&self, meas: MeasId, mut config: ac2_proto::model::MeasConfig) -> Command {
        let m = self.meas(meas);
        if let Some(m) = m {
            config.scope = m.config.scope;
        }
        // No such measurement: the daemon answers `not_found` before it looks at the base.
        let base = m.map_or(ac2_proto::units::Rev(0), |m| m.config_rev);
        Command::MeasUpdate { meas, base, config }
    }

    pub fn selected_meas(&self) -> Option<&Measurement> {
        self.meas(self.rig.selected?)
    }

    /// Whether `m`'s live curves are hidden in this app.
    pub fn meas_hidden(&self, m: &Measurement) -> bool {
        self.rig.hidden_meas.contains(&m.id)
    }

    /// The keys that act on "the selected curve" (Delete, A) act on the selected stored
    /// trace: it was selected after the measurement. Else on the selected measurement.
    pub fn keys_on_trace(&self) -> bool {
        self.selected_trace_meta().is_some()
    }

    /// The selection is the selected measurement's live-curve row, not its header (both
    /// act on the same curve; the list marks the one picked).
    pub fn live_row_selected(&self) -> bool {
        self.rig.selected_live.is_some()
            && self.rig.selected_live == self.rig.selected
            && !self.keys_on_trace()
    }

    /// Measurements pane `p` can show, in list order.
    pub fn pane_candidates(&self, p: PaneKind) -> Vec<&Measurement> {
        self.measurements()
            .into_iter()
            .filter(|m| p.shows(&m.config.kind))
            .collect()
    }

    /// What a pane key (start/stop, reset) acts on: the measurement the focused pane shows,
    /// never one of another kind selected elsewhere (an SPL meter picked in the list must not
    /// stop when S is pressed on the transfer pane). Says what to create when there is none.
    pub(super) fn focused_pane_meas(&mut self) -> Option<Measurement> {
        let p = self.layout.focus_kind();
        if p == PaneKind::Distortion {
            let m = self.selected_meas().cloned();
            if m.is_none() {
                self.warn(self.select_meas_first());
            }
            return m;
        }
        let m = self.pane_meas(self.layout.focus).cloned();
        if m.is_none() {
            let make = match p {
                PaneKind::Transfer => format!(
                    "no transfer measurement: {}",
                    self.keys.reach(CommandId::NewTransfer)
                ),
                PaneKind::Spectrum => {
                    format!("no spectrum or RTA: {} makes one", self.new_meas_reach())
                }
                _ => format!("no SPL meter: {}", self.keys.reach(CommandId::NewSpl)),
            };
            self.warn(make);
        }
        // The transfer pane showing a sweep's runs: a sweep plays only from the sweep pane.
        if let Some(s) = m.as_ref().filter(|m| !m.config.kind.is_job()) {
            self.warn(format!(
                "{} is a sweep: run it on the {} pane",
                s.config.name,
                PaneKind::Distortion.title()
            ));
            return None;
        }
        m
    }

    /// Stored trace `id` as mirrored, or why not.
    pub fn trace_meta(&self, id: TraceId) -> Result<TraceMeta, String> {
        self.daemon()
            .and_then(|s| s.traces.iter().find(|t| t.id == id))
            .cloned()
            .ok_or_else(|| "that trace is gone (deleted meanwhile)".to_string())
    }

    /// The stored trace selected in the list, if it still exists.
    pub fn selected_trace_meta(&self) -> Option<&TraceMeta> {
        let id = self.rig.selected_trace?;
        self.daemon()?.traces.iter().find(|t| t.id == id)
    }

    /// What K / Shift+K change now: the selected slot, else the spectrum pane's
    /// measurement when that pane has focus, else the transfer pane's.
    pub fn smooth_target(&self) -> Option<SmoothTarget> {
        if let Some(t) = self.selected_trace_meta() {
            return Some(SmoothTarget::Trace(t.clone()));
        }
        let pane = match self.layout.focus_kind() {
            PaneKind::Spectrum => PaneKind::Spectrum,
            _ => PaneKind::Transfer,
        };
        self.kind_meas(pane).map(|m| SmoothTarget::Meas(m.clone()))
    }

    /// A pane's title caption: the selected stored trace with its smoothing when its curve
    /// is drawn there, else the smoothing of the pane's measurement — `smoothing 1/6 oct`,
    /// `slot 3 (Main L S3): smoothing off`, `Sweep 2: smoothing off`; a selected target
    /// curve is named alone. Nothing for other curves smoothing does not apply to.
    pub fn smoothing_caption(&self, id: PaneId) -> Option<String> {
        let pane = self.layout.kind(id);
        if !matches!(pane, PaneKind::Transfer | PaneKind::Spectrum) {
            return None;
        }
        let t = match self.caption_trace(id) {
            Some(t) => SmoothTarget::Trace(t.clone()),
            None => SmoothTarget::Meas(self.pane_meas(id)?.clone()),
        };
        if let SmoothTarget::Trace(_) = &t
            && t.kind() == Smoothable::No
        {
            return Some(t.label());
        }
        if !matches!(t.kind(), Smoothable::Transfer | Smoothable::Spectrum) {
            return None;
        }
        let c = t.caption();
        Some(match t {
            SmoothTarget::Trace(_) => format!("{}: {c}", t.label()),
            SmoothTarget::Meas(_) => c,
        })
    }

    /// The mic-curve note of a readout on `input` whose frame says the daemon applied a
    /// curve (`applied`) or not: `mic curve: MM1 34804 90°`, `mic curve off`,
    /// `no mic curve stored for MM1 34804` … (`ac2_scene::cal::curve_note`).
    pub fn curve_note(&self, input: u16, applied: bool) -> Option<String> {
        let s = self.daemon()?;
        ac2_scene::cal::curve_note(applied, &ac2_proto::cal::state_input_use(s, input).curve)
    }

    /// The mic-curve part of pane `pane`'s title caption: the selected stored trace's curve
    /// when the pane draws it, else the shown measurement's (from its newest frame), else
    /// the shown sweep's.
    pub fn mic_curve_caption(&self, id: PaneId) -> Option<String> {
        use ac2_proto::topic::Topic;
        let pane = self.layout.kind(id);
        let stored =
            |t: &TraceMeta| ac2_scene::trace::curve_note(t.mic.as_ref(), t.mic_curve.as_deref());
        match pane {
            PaneKind::Transfer | PaneKind::Spectrum => {
                if let Some(t) = self.caption_trace(id) {
                    return stored(t);
                }
                let m = self.pane_meas(id)?;
                let stream = m.config.kind.stream()?;
                let applied = self
                    .rig
                    .data
                    .as_ref()
                    .and_then(|d| d.latest.get(&Topic::Data { meas: m.id, stream }))
                    .is_some_and(|f| match &f.frame.data {
                        ac2_proto::FrameData::Tf(x) => x.meta.mic_curve,
                        ac2_proto::FrameData::Spec(x) => x.meta.mic_curve,
                        ac2_proto::FrameData::Rta(x) => x.meta.mic_curve,
                        _ => false,
                    });
                meas_input(&m.config.kind).and_then(|i| self.curve_note(i, applied))
            }
            PaneKind::Distortion => self.pane_sweep(id).and_then(|(d, _)| stored(&d.meta)),
            PaneKind::Spl | PaneKind::Programme => None,
        }
    }

    /// The selected stored trace, when pane `id` (a transfer or spectrum pane) draws it: of
    /// the pane's group or compared into it, and shown. A title names only what its pane
    /// draws: a curve selected under another measurement, under Imported, or hidden would
    /// read as part of this plot.
    pub fn caption_trace(&self, id: PaneId) -> Option<&TraceMeta> {
        let kind = self.layout.kind(id);
        if !matches!(kind, PaneKind::Transfer | PaneKind::Spectrum) {
            return None;
        }
        let shown = self.pane_meas(id);
        // The IR view draws a stored trace only through its impulse response.
        if kind == PaneKind::Transfer && self.pane_modes(id).transfer.ir().is_some() {
            return self.stored_ir(shown).map(|d| &d.meta);
        }
        self.selected_trace_meta().filter(|t| {
            SmoothTarget::Trace((*t).clone()).pane() == kind
                && (self.on_pane(kind, t, shown) || self.trace_compared_on(kind, t, shown))
        })
    }

    /// The pane's title caption: smoothing and mic curve, `smoothing 1/6 oct · mic curve:
    /// MM1 34804 90°`.
    pub fn pane_caption(&self, id: PaneId) -> Option<String> {
        self.pane_caption_variants(id).into_iter().next()
    }

    /// The pane's title caption from the longest to the shortest, for a narrow title: all
    /// of it, then without the mic curve, then the selected stored trace's name alone (the
    /// one thing the title must keep: which curve the keys act on).
    pub fn pane_caption_variants(&self, id: PaneId) -> Vec<String> {
        let pane = self.layout.kind(id);
        let smoothing = self.smoothing_caption(id);
        let curve = self.mic_curve_caption(id);
        let trace = self.caption_trace(id).map(trace_label);
        let mut v: Vec<String> = Vec::new();
        let parts: Vec<&String> = [&smoothing, &curve].into_iter().flatten().collect();
        if !parts.is_empty() {
            v.push(
                parts
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(" · "),
            );
        }
        v.extend(smoothing);
        // The pane's own measurement hidden: said first and kept longest, or an empty plot
        // would read as a fault.
        let hidden = self
            .pane_meas(id)
            .filter(|m| {
                matches!(pane, PaneKind::Transfer | PaneKind::Spectrum) && self.meas_hidden(m)
            })
            .map(|m| format!("{} hidden", m.config.name));
        // The IR view draws no stopped IR: said as the hidden one is, so a plot holding only
        // its captures (or nothing) does not read as a fault. The response views keep the
        // stopped curve, tagged in the legend.
        let stopped = self
            .pane_meas(id)
            .filter(|m| {
                pane == PaneKind::Transfer
                    && self.pane_modes(id).transfer.ir().is_some()
                    && m.config.kind.publishes_tf()
                    && !m.running
                    && !self.meas_hidden(m)
            })
            .map(|m| format!("{} stopped", m.config.name));
        if let Some(h) = hidden.or(stopped) {
            v = v.iter().map(|s| format!("{h} · {s}")).collect();
            v.push(h);
        }
        v.extend(trace);
        v.dedup();
        v
    }

    /// The display offset of measurement `id`'s live curves, dB: the daemon keeps it with
    /// the measurement, so it follows the curve into every pane it is drawn in.
    pub fn meas_offset(&self, id: MeasId) -> f64 {
        self.meas(id).map_or(0.0, |m| m.offset.0)
    }

    pub fn edit(&self, id: MeasId) -> LiveEdit {
        self.rig.edits.get(&id).copied().unwrap_or_default()
    }

    /// Stored trace metadata as mirrored (edits, slots and visibility are the daemon's).
    pub fn stored_traces(&self) -> Vec<&TraceMeta> {
        let mut v: Vec<&TraceMeta> = self
            .daemon()
            .map(|s| s.traces.iter().collect())
            .unwrap_or_default();
        v.sort_by_key(|t| (t.edit.order, t.id));
        v
    }

    /// Every stored trace in the tree's order (group by group; in a group slotted by slot,
    /// then the rest oldest first): the order V / Shift+V step through and legends follow.
    pub fn trace_list(&self) -> Vec<&TraceMeta> {
        let ms = self.measurements();
        ac2_scene::meas_list::trace_order(&ms, &self.stored_traces())
    }

    /// The colour every curve is drawn in (live, stored, math), in every pane, legend and
    /// tree dot: each measurement's colour family.
    pub fn curve_colours(
        &self,
        theme: &ac2_scene::theme::Theme,
    ) -> ac2_scene::families::CurveColours {
        ac2_scene::families::curve_colours(theme, &self.measurements(), &self.stored_traces())
    }

    /// Stored traces as the tree and the trace list get them.
    pub(super) fn trace_items(
        &self,
        colours: &ac2_scene::families::CurveColours,
    ) -> Vec<ac2_scene::trace_list::TraceItem<'_>> {
        self.stored_traces()
            .into_iter()
            .map(|meta| ac2_scene::trace_list::TraceItem {
                meta,
                has_data: self.rig.traces.contains_key(&meta.id),
                color: colours.trace(meta.id),
                eq: match meta.source {
                    ac2_proto::model::TraceSource::Eq { meas, set, .. } => self
                        .meas(meas)
                        .and_then(|m| m.eq.iter().find(|s| s.id == set)),
                    _ => None,
                },
            })
            .collect()
    }

    /// The measurement tree beside the panes.
    pub fn tree_rows(&self) -> Vec<ac2_scene::meas_list::TreeRow> {
        self.tree_rows_folded(&self.rig.collapsed)
    }

    /// Every measurement and math channel in the tree's order, folded groups included:
    /// folding hides a group's rows, not the measurements in it. The imported group's
    /// header is no measurement and is left out.
    pub fn tree_meas_order(&self) -> Vec<MeasId> {
        use ac2_scene::meas_list::TreeKey;
        self.tree_rows_folded(&std::collections::BTreeSet::new())
            .into_iter()
            .filter_map(|r| match r.key {
                TreeKey::Meas(id) | TreeKey::Math(id) => Some(id),
                _ => None,
            })
            .collect()
    }

    pub(super) fn tree_rows_folded(
        &self,
        collapsed: &std::collections::BTreeSet<ac2_proto::model::TraceOwner>,
    ) -> Vec<ac2_scene::meas_list::TreeRow> {
        let colours = self.curve_colours(&ac2_scene::theme::Theme::by_name(self.theme));
        ac2_scene::meas_list::tree_rows(&ac2_scene::meas_list::TreeInput {
            meas: self.meas_items(&colours),
            traces: self.trace_items(&colours),
            collapsed,
            selected: self.rig.selected,
            selected_trace: self.rig.selected_trace,
            keys_on_trace: self.keys_on_trace(),
            live_selected: self.live_row_selected(),
            sweep: self.daemon().and_then(|s| s.sweep.as_ref()),
            compared: self.compared_rows(),
            rig: self.daemon().map(|s| &s.rig),
        })
    }

    /// Every measurement with this app's display of it.
    pub(super) fn meas_items(
        &self,
        colours: &ac2_scene::families::CurveColours,
    ) -> Vec<ac2_scene::meas_list::MeasItem<'_>> {
        self.measurements()
            .into_iter()
            .map(|m| {
                let e = self.edit(m.id);
                let expression = match &m.config.kind {
                    MeasKind::Math { config } => {
                        Some(ac2_scene::math::expression(&config.expr, |o| {
                            self.operand_name(o)
                        }))
                    }
                    _ => None,
                };
                ac2_scene::meas_list::MeasItem {
                    meas: m,
                    expression,
                    offset_db: m.offset.0,
                    inverted: e.inverted,
                    hidden: self.meas_hidden(m),
                    color: colours.meas(m.id),
                }
            })
            .collect()
    }

    /// The group (measurement or imported) that owns what is selected: the selected trace's
    /// owner, a math channel's owner, else the selected measurement.
    pub fn selected_group(&self) -> Option<ac2_proto::model::TraceOwner> {
        use ac2_proto::model::TraceOwner;
        if let Some(t) = self.selected_trace_meta().filter(|_| self.keys_on_trace()) {
            let ms = self.measurements();
            return Some(ac2_scene::meas_list::group_of(t, &ms));
        }
        let m = self.selected_meas()?;
        Some(match &m.config.kind {
            MeasKind::Math { config } => config.owner,
            _ => TraceOwner::Meas { meas: m.id },
        })
    }

    /// The sidebar's rows of stored traces.
    pub fn trace_rows(&self) -> Vec<ac2_scene::trace_list::TraceRow> {
        let colours = self.curve_colours(&ac2_scene::theme::Theme::by_name(self.theme));
        ac2_scene::trace_list::trace_rows(&self.trace_items(&colours), self.rig.selected_trace)
    }

    /// The measurement list's rows, in list order: the selected one marked, as the one the
    /// keys act on unless a stored trace was selected after it.
    pub fn meas_rows(&self) -> Vec<ac2_scene::meas_list::MeasRow> {
        let active = !self.keys_on_trace();
        let colours = self.curve_colours(&ac2_scene::theme::Theme::by_name(self.theme));
        self.meas_items(&colours)
            .iter()
            .map(|item| ac2_scene::meas_list::meas_row(item, self.rig.selected, active))
            .collect()
    }

    /// The trace in each slot 1…9 (index 0 = slot 1).
    pub fn slots(&self) -> [Option<&TraceMeta>; 9] {
        let mut out = [None; 9];
        for t in self
            .daemon()
            .map(|s| s.traces.as_slice())
            .unwrap_or_default()
        {
            if let Some(n) = t.edit.slot.filter(|n| (1..=9).contains(n)) {
                out[usize::from(n - 1)] = Some(t);
            }
        }
        out
    }

    /// Shown stored traces on the transfer pane, slotted first (by slot), then by order.
    pub(super) fn shown_transfer_traces(&self) -> Vec<&TraceMeta> {
        let mut v: Vec<&TraceMeta> = self
            .stored_traces()
            .into_iter()
            .filter(|t| self.on_pane(PaneKind::Transfer, t, self.kind_meas(PaneKind::Transfer)))
            .collect();
        v.sort_by_key(|t| (t.edit.slot.unwrap_or(u8::MAX), t.edit.order, t.id));
        v
    }

    /// This connection's identity. The mirror's is authoritative: it belongs to the daemon
    /// incarnation the shown state comes from, and a restarted daemon binds a new one (the
    /// id from the connect would make another client's lease look like ours, or ours like
    /// another's).
    pub fn my_client_id(&self) -> Option<&ClientId> {
        let ConnState::Connected { client_id, .. } = &self.rig.conn else {
            return None;
        };
        match &self.rig.mirror {
            Some(m) if m.incarnation.is_some() => m.client_id.as_ref(),
            _ => Some(client_id),
        }
    }

    /// The daemon's open audio I/O.
    pub fn open_session(&self) -> Option<&ac2_proto::model::OpenSession> {
        self.daemon().and_then(|s| s.session.open.as_ref())
    }

    /// No audio I/O and nothing kept (no measurements, no stored curves): a fresh start,
    /// where the Sessions dialog begins one. With anything kept, a session lives on (an
    /// autosave restored, a file loaded) and only its audio is missing.
    pub fn nothing_kept(&self) -> bool {
        self.open_session().is_none()
            && self.measurements().is_empty()
            && self.stored_traces().is_empty()
    }

    /// The top bar with no audio I/O: with nothing kept, the Sessions dialog starts
    /// one; with measurements kept, the audio is what is missing.
    pub fn no_audio_line(&self, keymap: &Keymap) -> String {
        if self.measurements().is_empty() {
            format!(
                "no session · {}, then New session",
                keymap.reach(CommandId::Sessions)
            )
        } else {
            format!(
                "no audio I/O · {} opens it",
                keymap.reach(CommandId::OpenSession)
            )
        }
    }

    /// What pane `pane` says when there is nothing to draw yet. A transfer pane: no audio
    /// session, or a session without measurements. Any graph pane: with no stored curve of
    /// its group drawn, that shown traces of a kind it draws wait under Imported. `None` once
    /// there is something (or no daemon). Over drawn curves it moves to the pane's title
    /// strip, out of their way.
    pub fn empty_hint(&self, pane: PaneId, keymap: &Keymap) -> Option<EmptyHint> {
        if !self.connected() {
            return None;
        }
        let st = self.daemon()?;
        let kind = self.layout.kind(pane);
        let text = if kind == PaneKind::Transfer
            && st.session.open.is_none()
            && st.measurements.is_empty()
        {
            // Nothing to keep: a new session is the way in.
            ac2_scene::sessions::no_session_hint(&keymap.reach(CommandId::Sessions))
        } else if kind == PaneKind::Transfer && st.session.open.is_none() {
            format!("No audio I/O — {}", open_session_hint(keymap))
        } else if kind == PaneKind::Transfer && st.measurements.is_empty() {
            let reach = [CommandId::NewMeasurement, CommandId::Palette]
                .into_iter()
                .find_map(|c| keymap.label(c, Scope::Global))
                .unwrap_or_else(|| "Command palette".to_owned());
            ac2_scene::new_meas::no_measurements(&reach)
        } else if !self.pane_shows_stored(pane)
            && let n @ 1.. = self.imported_traces(kind)
        {
            // An imported file draws nowhere until it has a measurement: say where it went.
            let key = keymap
                .chords(CommandId::MoveTrace, Scope::Global)
                .first()
                .map(|c| c.label());
            let text = ac2_scene::meas_list::imported_hint(n, key.as_deref());
            let place = if self.pane_shows_live(pane) {
                HintPlace::Title
            } else {
                HintPlace::Centre
            };
            return Some(EmptyHint { text, place });
        } else if kind == PaneKind::Spectrum
            // The spectrograph says it under its own history.
            && !self.pane_modes(pane).spectrum.spectrograph()
            && !self.pane_shows_live(pane)
            && !self.pane_shows_stored(pane)
            && let Some(m) = self.pane_meas(pane)
        {
            // Its own measurement has nothing to draw: say why, never draw another's.
            use ac2_scene::missing::{Missing, missing_text};
            let (why, command, scope) = if self.meas_hidden(m) {
                (Missing::Hidden, CommandId::ToggleSelected, Scope::Global)
            } else if !m.running {
                (Missing::Stopped, CommandId::StartStop, Scope::Spectrum)
            } else {
                let what = Missing::NotYet {
                    what: "frames",
                    after: None,
                };
                (what, CommandId::StartStop, Scope::Spectrum)
            };
            let key = keymap
                .first_chord(command, scope)
                .map_or_else(|| "the palette".to_owned(), |c| c.label());
            missing_text(&m.config.name, why, &key)
        } else {
            return None;
        };
        let place = if self.pane_shows_stored(pane) {
            HintPlace::Title
        } else {
            HintPlace::Centre
        };
        Some(EmptyHint { text, place })
    }

    /// Whether pane `pane` draws any stored curve whose data has arrived: a shown capture,
    /// target or sweep of its group, or a compared one; on the sweep pane, a run.
    pub fn pane_shows_stored(&self, pane: PaneId) -> bool {
        let kind = self.layout.kind(pane);
        if kind == PaneKind::Distortion {
            return self.pane_sweep(pane).is_some();
        }
        let shown = self.pane_meas(pane);
        self.rig.traces.values().any(|(t, _)| {
            self.on_pane(kind, &t.meta, shown) || self.trace_compared_on(kind, &t.meta, shown)
        })
    }

    /// The measurement whose group a pane showing `shown` draws: `shown`, or the owner of
    /// the math channel it is. `None` with nothing shown (or a math channel under Imported).
    pub fn group_of_shown(&self, shown: Option<&Measurement>) -> Option<MeasId> {
        let m = shown?;
        ac2_scene::meas_list::meas_group(m, &self.measurements()).meas()
    }

    /// Whether a pane of kind `kind` showing `shown` draws stored trace `t` as its own: a
    /// shown curve of a kind the pane draws, of the pane's group. Every other group's waits
    /// until its measurement is the pane's (or is compared); Imported's until moved under a
    /// measurement, since a file without one belongs to no pane. The sweep pane draws one
    /// run of its group instead ([`Self::pane_sweep`]).
    pub fn on_pane(&self, kind: PaneKind, t: &TraceMeta, shown: Option<&Measurement>) -> bool {
        kind != PaneKind::Distortion
            && kind_shown(t, kind)
            && ac2_scene::meas_list::group_of(t, &self.measurements())
                .meas()
                .is_some_and(|g| Some(g) == self.group_of_shown(shown))
    }

    /// Whether measurement `m` publishes a live curve a pane of kind `kind` draws.
    pub fn live_drawn_in(m: &Measurement, kind: PaneKind) -> bool {
        match kind {
            PaneKind::Transfer => m.config.kind.publishes_tf(),
            PaneKind::Spectrum => spectrum_stream(m).is_some(),
            PaneKind::Spl | PaneKind::Distortion | PaneKind::Programme => false,
        }
    }

    /// Whether a pane of kind `kind` showing `shown` draws measurement `m`'s live curve as
    /// its own: it publishes one the pane draws, is not hidden, and is the pane's
    /// measurement or a math channel of the pane's group. Stopped, it keeps its last curve,
    /// tagged `stopped`: hiding it is the operator's call (A).
    pub fn live_on_pane(
        &self,
        kind: PaneKind,
        m: &Measurement,
        shown: Option<&Measurement>,
    ) -> bool {
        if !Self::live_drawn_in(m, kind) || self.meas_hidden(m) {
            return false;
        }
        if shown.map(|p| p.id) == Some(m.id) {
            return true;
        }
        ac2_scene::meas_list::meas_group(m, &self.measurements())
            .meas()
            .is_some_and(|g| Some(g) == self.group_of_shown(shown))
    }

    /// Whether a pane of kind `kind` showing `shown` draws `m`'s live curve for compare
    /// only: compared into `shown` (C), not hidden, a curve the pane draws, and not of the
    /// pane's group (drawn there anyway, once, unmarked).
    pub fn live_compared_on(
        &self,
        kind: PaneKind,
        m: &Measurement,
        shown: Option<&Measurement>,
    ) -> bool {
        Self::live_drawn_in(m, kind)
            && !self.meas_hidden(m)
            && self
                .compared_into(shown)
                .contains(&ac2_proto::model::Compared::Meas { meas: m.id })
            && !self.live_on_pane(kind, m, shown)
    }

    /// Whether a pane of kind `kind` showing `shown` draws stored trace `t` for compare
    /// only: compared into `shown`, shown, of a kind the pane draws, filed under a measurement, and not
    /// of the pane's group. An imported one is not drawn even compared: it is no
    /// measurement's yet. The sweep pane's compared runs are [`Self::compared_sweeps`].
    pub fn trace_compared_on(
        &self,
        kind: PaneKind,
        t: &TraceMeta,
        shown: Option<&Measurement>,
    ) -> bool {
        kind != PaneKind::Distortion
            && self
                .compared_into(shown)
                .contains(&ac2_proto::model::Compared::Trace { trace: t.id })
            && kind_shown(t, kind)
            && ac2_scene::meas_list::group_of(t, &self.measurements()) != TraceOwner::Imported
            && !self.on_pane(kind, t, shown)
    }

    /// The tree rows compared (C) into the focused pane's measurement, for their mark.
    pub(super) fn compared_rows(&self) -> Vec<ac2_scene::meas_list::TreeKey> {
        use ac2_proto::model::Compared;
        use ac2_scene::meas_list::TreeKey;
        self.compared_into(self.pane_meas(self.layout.focus))
            .iter()
            .filter_map(|c| match *c {
                Compared::Meas { meas } => self.meas(meas).map(|m| match m.config.kind {
                    MeasKind::Math { .. } => TreeKey::Math(meas),
                    _ => TreeKey::Live(meas),
                }),
                Compared::Trace { trace } => Some(TreeKey::Trace(trace)),
            })
            .collect()
    }

    /// Stored traces under Imported of a kind a pane of kind `kind` draws (shown ones; any
    /// sweep run for the sweep pane, which picks runs whether shown or not): no pane draws
    /// them until they are moved under a measurement.
    fn imported_traces(&self, kind: PaneKind) -> usize {
        let ms = self.measurements();
        self.stored_traces()
            .into_iter()
            .filter(|t| {
                (if kind == PaneKind::Distortion {
                    drawn_in(t, kind)
                } else {
                    kind_shown(t, kind)
                }) && ac2_scene::meas_list::group_of(t, &ms) == TraceOwner::Imported
            })
            .count()
    }

    /// Whether pane `pane` draws a live curve now (one with a frame).
    fn pane_shows_live(&self, pane: PaneId) -> bool {
        let Some(d) = self.rig.data.as_ref() else {
            return false;
        };
        let kind = self.layout.kind(pane);
        let shown = self.pane_meas(pane);
        self.measurements().into_iter().any(|m| {
            let Some(stream) = m.config.kind.stream() else {
                return false;
            };
            (self.live_on_pane(kind, m, shown) || self.live_compared_on(kind, m, shown))
                && d.latest.get(&Topic::Data { meas: m.id, stream }).is_some()
        })
    }

    pub fn connected(&self) -> bool {
        matches!(self.rig.conn, ConnState::Connected { .. })
    }

    /// Keys go to the picked Inputs row's meters, else to the focused pane's scope.
    pub fn scope(&self) -> Scope {
        if self.rig.input_pick.is_some() {
            Scope::Inputs
        } else {
            self.layout.focused().scope()
        }
    }

    /// Navigation still moving: the UI keeps repainting until it settles.
    pub fn animating(&self) -> bool {
        !self.nav.settled()
    }

    /// Some Leq or band Leq bars still ease to their newest frame at `now`.
    pub fn bars_easing(&self, now: std::time::Instant) -> bool {
        self.prefs.bar_ease
            && self.rig.bar_from.keys().any(|t| {
                self.rig
                    .data
                    .as_ref()
                    .and_then(|d| d.latest.get(t))
                    .is_some_and(|tf| {
                        ac2_scene::bar_ease::easing(
                            now.saturating_duration_since(tf.received).as_secs_f64(),
                        )
                    })
            })
    }

    /// Generator ceiling from the daemon.
    pub fn ceiling(&self) -> Option<Dbfs> {
        self.daemon().map(|s| s.generator.ceiling)
    }

    /// Something may be emitting or armed: ours in flight, or the mirrored generator.
    /// A window drawn over the panes (behind a backdrop) is open: it owns the keyboard and
    /// the mouse. The delay candidates and a pane's measurement list stay out of the way.
    pub fn window_over_panes(&self) -> bool {
        !matches!(
            self.overlay,
            Overlay::None | Overlay::DelayPick(_) | Overlay::PaneMenu(_)
        )
    }

    pub fn stimulus_live(&self) -> bool {
        self.rig.stimulus.phase != StimPhase::Idle
            || self
                .daemon()
                .is_some_and(|s| s.generator.armed || s.generator.firing)
    }
}
