//! Read-only views of the state for the scenes and the app (measurements, traces, captions, meters, hints), and the remembered pane layout.

use super::*;

impl AppState {
    /// The mirrored daemon state.
    pub fn daemon(&self) -> Option<&State> {
        self.mirror.as_ref().and_then(|m| m.state.as_deref())
    }

    pub fn measurements(&self) -> Vec<&Measurement> {
        let mut v: Vec<&Measurement> = self
            .daemon()
            .map(|s| s.measurements.iter().collect())
            .unwrap_or_default();
        v.sort_by_key(|m| m.id);
        v
    }

    pub fn meas(&self, id: MeasId) -> Option<&Measurement> {
        self.daemon()?.measurements.iter().find(|m| m.id == id)
    }

    pub fn selected_meas(&self) -> Option<&Measurement> {
        self.meas(self.selected?)
    }

    /// Whether `m`'s live curves are hidden in this app.
    pub fn meas_hidden(&self, m: &Measurement) -> bool {
        self.hidden_meas.contains(&m.config.name)
    }

    /// The keys that act on "the selected curve" (Delete, A) act on the selected stored
    /// trace: it was selected after the measurement. Else on the selected measurement.
    pub fn keys_on_trace(&self) -> bool {
        self.selected_trace_meta().is_some()
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
                self.warn("select a measurement first (N)");
            }
            return m;
        }
        let m = self.pane_meas(self.layout.focus).cloned();
        if m.is_none() {
            self.warn(match p {
                PaneKind::Transfer => "no transfer measurement: Ctrl+K → New transfer measurement…",
                PaneKind::Spectrum => "no spectrum or RTA: Ctrl+K → New spectrum / New RTA",
                _ => "no SPL meter: Ctrl+K → New SPL meter",
            });
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
        let id = self.selected_trace?;
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
        let t = match self.selected_trace_meta() {
            Some(t) if SmoothTarget::Trace(t.clone()).pane() == pane => {
                SmoothTarget::Trace(t.clone())
            }
            _ => SmoothTarget::Meas(self.pane_meas(id)?.clone()),
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
                if let Some(t) = self.selected_trace_meta()
                    && SmoothTarget::Trace(t.clone()).pane() == pane
                {
                    return stored(t);
                }
                let m = self.pane_meas(id)?;
                let stream = m.config.kind.stream()?;
                let applied = self
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
            PaneKind::Distortion => self.shown_sweep().and_then(|(d, _)| stored(&d.meta)),
            PaneKind::Spl => None,
        }
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
        let trace = self
            .selected_trace_meta()
            .filter(|t| {
                matches!(pane, PaneKind::Transfer | PaneKind::Spectrum)
                    && SmoothTarget::Trace((*t).clone()).pane() == pane
            })
            .map(trace_label);
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
        // A stopped transfer measurement draws no live curve: said as the hidden one is, so
        // a plot holding only its captures (or nothing) does not read as a fault. The IR
        // view says it in its own plot.
        let stopped = self
            .pane_meas(id)
            .filter(|m| {
                pane == PaneKind::Transfer
                    && self.pane_modes(id).transfer != TransferView::Ir
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

    pub fn edit(&self, id: MeasId) -> LiveEdit {
        self.edits.get(&id).copied().unwrap_or_default()
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
                has_data: self.traces.contains_key(&meta.id),
                color: colours.trace(meta.id),
            })
            .collect()
    }

    /// The measurement tree beside the panes.
    pub fn tree_rows(&self) -> Vec<ac2_scene::meas_list::TreeRow> {
        self.tree_rows_folded(&self.collapsed)
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

    fn tree_rows_folded(
        &self,
        collapsed: &std::collections::BTreeSet<ac2_proto::model::TraceOwner>,
    ) -> Vec<ac2_scene::meas_list::TreeRow> {
        let colours = self.curve_colours(&ac2_scene::theme::Theme::by_name(self.theme));
        ac2_scene::meas_list::tree_rows(&ac2_scene::meas_list::TreeInput {
            meas: self.meas_items(&colours),
            traces: self.trace_items(&colours),
            collapsed,
            selected: self.selected,
            selected_trace: self.selected_trace,
            keys_on_trace: self.keys_on_trace(),
            sweep: self.daemon().and_then(|s| s.sweep.as_ref()),
            compared: self.compared_rows(),
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
                    offset_db: e.offset_db,
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
        ac2_scene::trace_list::trace_rows(&self.trace_items(&colours), self.selected_trace)
    }

    /// The measurement list's rows, in list order: the selected one marked, as the one the
    /// keys act on unless a stored trace was selected after it.
    pub fn meas_rows(&self) -> Vec<ac2_scene::meas_list::MeasRow> {
        let active = !self.keys_on_trace();
        let colours = self.curve_colours(&ac2_scene::theme::Theme::by_name(self.theme));
        self.meas_items(&colours)
            .iter()
            .map(|item| ac2_scene::meas_list::meas_row(item, self.selected, active))
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
            .filter(|t| self.on_transfer_pane(t, self.kind_meas(PaneKind::Transfer)))
            .collect();
        v.sort_by_key(|t| (t.edit.slot.unwrap_or(u8::MAX), t.edit.order, t.id));
        v
    }

    /// This connection's identity. The mirror's is authoritative: it belongs to the daemon
    /// incarnation the shown state comes from, and a restarted daemon binds a new one (the
    /// id from the connect would make another client's lease look like ours, or ours like
    /// another's).
    pub fn my_client_id(&self) -> Option<&ClientId> {
        let ConnState::Connected { client_id, .. } = &self.conn else {
            return None;
        };
        match &self.mirror {
            Some(m) if m.incarnation.is_some() => m.client_id.as_ref(),
            _ => Some(client_id),
        }
    }

    /// The daemon's open audio session.
    pub fn open_session(&self) -> Option<&ac2_proto::model::OpenSession> {
        self.daemon().and_then(|s| s.session.open.as_ref())
    }

    /// What the transfer pane says when there is nothing to measure yet: no audio session,
    /// or a session without measurements; or, with no stored curve of the pane's group
    /// drawn, that shown traces wait under Imported. `None` once there is something (or no
    /// daemon). Over drawn curves it moves to the pane's title strip, out of their way.
    pub fn empty_hint(&self, keymap: &Keymap) -> Option<EmptyHint> {
        if !self.connected() {
            return None;
        }
        let st = self.daemon()?;
        let text = if st.session.open.is_none() {
            format!("No audio session — {}", open_session_hint(keymap))
        } else if st.measurements.is_empty() {
            let palette = keymap
                .chords(CommandId::Palette, Scope::Global)
                .first()
                .map_or_else(|| "Command palette".to_owned(), |c| c.label());
            format!(
                "No measurements — {palette} → New transfer measurement… (or New spectrum, RTA, SPL meter)"
            )
        } else if !self.transfer_shows_stored()
            && let n @ 1.. = self.imported_transfer_traces()
        {
            // An imported file draws nowhere until it has a measurement: say where it went.
            let key = keymap
                .chords(CommandId::MoveTrace, Scope::Global)
                .first()
                .map(|c| c.label());
            let text = ac2_scene::meas_list::imported_hint(n, key.as_deref());
            let place = if self.transfer_shows_live() {
                HintPlace::Title
            } else {
                HintPlace::Centre
            };
            return Some(EmptyHint { text, place });
        } else {
            return None;
        };
        let place = if self.transfer_shows_stored() {
            HintPlace::Title
        } else {
            HintPlace::Centre
        };
        Some(EmptyHint { text, place })
    }

    /// Whether the transfer pane draws any stored curve (a shown capture, target or sweep
    /// of its group, or a compared one, whose data has arrived).
    pub fn transfer_shows_stored(&self) -> bool {
        let tf = self.kind_meas(PaneKind::Transfer);
        self.traces.values().any(|(t, _)| {
            self.on_transfer_pane(&t.meta, tf) || self.trace_compared_on_transfer(&t.meta, tf)
        })
    }

    /// The measurement whose group a transfer pane showing `shown` draws.
    pub fn group_of_shown(&self, shown: Option<&Measurement>) -> Option<MeasId> {
        let m = shown?;
        match ac2_scene::meas_list::meas_group(m, &self.measurements()) {
            TraceOwner::Meas { meas } => Some(meas),
            TraceOwner::Imported => None,
        }
    }

    /// Whether a transfer pane showing `shown` draws stored trace `t`: a shown
    /// transfer-like curve of the pane's group. Every other group's, Imported's too, waits
    /// until its measurement is the pane's (or the trace is moved under it). With no
    /// measurement to show at all, the pane's group is Imported: files looked at without a
    /// rig are drawn.
    pub fn on_transfer_pane(&self, t: &TraceMeta, shown: Option<&Measurement>) -> bool {
        transfer_kind_shown(t)
            && match ac2_scene::meas_list::group_of(t, &self.measurements()) {
                TraceOwner::Meas { meas } => Some(meas) == self.group_of_shown(shown),
                TraceOwner::Imported => shown.is_none(),
            }
    }

    /// Whether a transfer pane showing `shown` draws measurement `m`'s live curve: it
    /// publishes one, runs, is not hidden, and is the pane's measurement or a math channel
    /// of the pane's group. A stopped measurement's last frame describes a system no longer
    /// measured: drawn as if live it would be read as the current response, so only its
    /// stored traces stay.
    pub fn live_on_transfer_pane(&self, m: &Measurement, shown: Option<&Measurement>) -> bool {
        if !m.config.kind.publishes_tf() || !m.running || self.meas_hidden(m) {
            return false;
        }
        if shown.map(|p| p.id) == Some(m.id) {
            return true;
        }
        match ac2_scene::meas_list::meas_group(m, &self.measurements()) {
            TraceOwner::Meas { meas } => Some(meas) == self.group_of_shown(shown),
            TraceOwner::Imported => false,
        }
    }

    /// Whether measurement `m` (a live curve or math channel) is compared (C).
    pub fn meas_compared(&self, m: &Measurement) -> bool {
        self.compared_meas.contains(&m.config.name)
    }

    /// Whether the transfer pane draws `m`'s live curve for compare only: compared, running,
    /// not hidden, and not of the pane's group (drawn there anyway, once, unmarked).
    pub fn live_compared_on_transfer(&self, m: &Measurement, shown: Option<&Measurement>) -> bool {
        m.config.kind.publishes_tf()
            && m.running
            && !self.meas_hidden(m)
            && self.meas_compared(m)
            && !self.live_on_transfer_pane(m, shown)
    }

    /// Whether the transfer pane draws stored trace `t` for compare only: compared, shown,
    /// a transfer-like curve, and not of the pane's group.
    pub fn trace_compared_on_transfer(&self, t: &TraceMeta, shown: Option<&Measurement>) -> bool {
        self.compared_traces.contains(&t.id)
            && transfer_kind_shown(t)
            && !self.on_transfer_pane(t, shown)
    }

    /// The tree rows compared (C), for their mark.
    pub(super) fn compared_rows(&self) -> Vec<ac2_scene::meas_list::TreeKey> {
        use ac2_scene::meas_list::TreeKey;
        let mut v: Vec<TreeKey> = self
            .measurements()
            .into_iter()
            .filter(|m| self.meas_compared(m))
            .map(|m| match m.config.kind {
                MeasKind::Math { .. } => TreeKey::Math(m.id),
                _ => TreeKey::Live(m.id),
            })
            .collect();
        v.extend(self.compared_traces.iter().map(|id| TreeKey::Trace(*id)));
        v
    }

    /// Shown transfer-like stored traces under Imported the transfer pane leaves out: they
    /// show once moved under a measurement (or compared).
    fn imported_transfer_traces(&self) -> usize {
        let ms = self.measurements();
        self.stored_traces()
            .into_iter()
            .filter(|t| {
                transfer_kind_shown(t)
                    && ac2_scene::meas_list::group_of(t, &ms) == TraceOwner::Imported
                    && !self.compared_traces.contains(&t.id)
            })
            .count()
    }

    /// Whether the transfer pane draws a live curve now (one with a frame).
    fn transfer_shows_live(&self) -> bool {
        let Some(d) = self.data.as_ref() else {
            return false;
        };
        let tf = self.kind_meas(PaneKind::Transfer);
        self.measurements().into_iter().any(|m| {
            (self.live_on_transfer_pane(m, tf) || self.live_compared_on_transfer(m, tf))
                && d.latest
                    .get(&Topic::Data {
                        meas: m.id,
                        stream: Stream::Tf,
                    })
                    .is_some()
        })
    }

    pub fn connected(&self) -> bool {
        matches!(self.conn, ConnState::Connected { .. })
    }

    /// Keys go to the focused pane's scope.
    pub fn scope(&self) -> Scope {
        self.layout.focused().scope()
    }

    /// Navigation still moving: the UI keeps repainting until it settles.
    pub fn animating(&self) -> bool {
        !self.nav.settled()
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
        self.stimulus.phase != StimPhase::Idle
            || self
                .daemon()
                .is_some_and(|s| s.generator.armed || s.generator.firing)
    }

    // ----- reducer -----------------------------------------------------------------------

    /// Once the daemon's state is known, each pane shows the measurement it showed when the
    /// app last ran, if one of that name (and kind) still exists; else the pane's usual
    /// choice, quietly. With no layout remembered, the one pane shows the kind of the first
    /// measurement (a transfer pane while there is none).
    pub(super) fn restore_pane_meas(&mut self) {
        // A layout the operator changed is theirs from then on.
        let fresh = Layout::default();
        if self.layout.root != fresh.root || self.layout.views != fresh.views {
            self.pane_auto = false;
        }
        if self.daemon().is_none() {
            return;
        }
        // The first pane shows what the daemon measures: the kind of its first measurement.
        if self.pane_auto
            && let Some(kind) = self
                .measurements()
                .first()
                .map(|m| PaneKind::for_kind(&m.config.kind))
        {
            self.pane_auto = false;
            let f = self.layout.focus;
            self.set_pane_kind(f, kind);
        }
        if self.pending_pane_meas.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.pending_pane_meas);
        for (pane, name) in pending {
            let kind = self.layout.kind(pane);
            let id = self
                .measurements()
                .iter()
                .find(|m| m.config.name == name && kind.shows(&m.config.kind))
                .map(|m| m.id);
            if let Some(id) = id {
                if let Some(v) = self.layout.view_mut(pane) {
                    v.meas = Some(id);
                }
                if self.layout.focus == pane {
                    self.select_on(pane, id);
                }
            }
        }
    }

    /// The layout as it is now, as the preferences keep it.
    pub fn layout_prefs(&self) -> crate::prefs::LayoutPrefs {
        let remembered = |id: PaneId| {
            self.prefs
                .layout
                .panes
                .as_ref()
                .and_then(|p| p.views.iter().find(|v| v.id == id))
                .and_then(|v| v.measurement.clone())
        };
        // Unknown until the daemon's state is (and while a remembered one waits for it):
        // keep what the preferences say. No layout of the operator's yet: none is kept.
        let panes = (!self.pane_auto).then(|| crate::prefs::PanesPrefs {
            root: self.layout.root.clone(),
            focus: self.layout.focus,
            views: self
                .layout
                .root
                .leaves()
                .into_iter()
                .filter_map(|id| {
                    let v = self.layout.view(id)?;
                    let measurement = match (self.pending_pane_meas.get(&id), self.daemon()) {
                        (Some(n), _) => Some(n.clone()),
                        (None, None) => remembered(id),
                        // A pane following the selection keeps following it: what it
                        // resolves to now is not its choice.
                        (None, Some(_)) if v.meas.is_none() => None,
                        (None, Some(_)) => self.pane_meas(id).map(|m| m.config.name.clone()),
                    };
                    Some(crate::prefs::PanePrefs {
                        id,
                        kind: v.kind,
                        measurement,
                        modes: v.modes,
                    })
                })
                .collect(),
        });
        // Names of measurements that exist (all of them until the daemon's state is known):
        // a deleted one's name must not hide a new one of that name in a later run.
        let existing = |set: &BTreeSet<String>| match self.daemon() {
            None => set.clone(),
            Some(_) => self
                .measurements()
                .iter()
                .map(|m| m.config.name.clone())
                .filter(|n| set.contains(n))
                .collect(),
        };
        let hidden = existing(&self.hidden_meas);
        let compared = existing(&self.compared_meas);
        crate::prefs::LayoutPrefs {
            panes,
            maximized: self.layout.maximized,
            fullscreen: self.fullscreen,
            distortion_unit: self.view.distortion.unit,
            hidden,
            compared,
        }
    }

    /// Saves the layout, the level axes and the legend with the preferences when they changed.
    pub(super) fn remember_layout(&mut self) {
        let now = self.layout_prefs();
        if now != self.prefs.layout {
            self.prefs.layout = now;
            self.prefs_dirty = true;
        }
        let levels = crate::prefs::LevelPrefs::of(&self.view);
        if levels != self.prefs.levels {
            self.prefs.levels = levels;
            self.prefs_dirty = true;
        }
        let legend = crate::prefs::LegendPrefs::of(&self.view.tf.legend);
        if legend != self.prefs.legend {
            self.prefs.legend = legend;
            self.prefs_dirty = true;
        }
    }

    /// What needs metering: the session's input meters while a session is open (the
    /// sidebar shows them all the time) or a dialog meters inputs, and a device preview (the
    /// session dialog on a device the session does not capture).
    pub(super) fn meter_wants(
        &self,
    ) -> (
        bool,
        Option<(ac2_proto::model::BackendKind, ac2_proto::model::DeviceId)>,
    ) {
        let session = self.open_session().is_some();
        match &self.overlay {
            _ if self.overlay.session().is_some() => (
                true,
                self.overlay.session().and_then(|d| d.preview_target()),
            ),
            Overlay::Form(_) => (true, None),
            _ => (session, None),
        }
    }

    /// Follows what the always-on parts of the window need from the mirror: the device
    /// list once per session (the inputs' channel names), and when the running sweep's
    /// step began.
    pub(super) fn sync_session_watch(&mut self, out: &mut Vec<Request>) {
        let epoch = self
            .daemon()
            .filter(|s| s.session.open.is_some())
            .map(|s| s.session.epoch);
        if let Some(e) = epoch
            && self.devices.is_none()
            && self.devices_for != Some(e)
            && self.connected()
        {
            self.devices_for = Some(e);
            if !matches!(self.overlay, Overlay::Settings(_) | Overlay::Form(_)) {
                out.push(Request::Devices);
            }
        }
        let run = self
            .daemon()
            .and_then(|s| s.sweep.as_ref())
            .filter(|r| r.active())
            .map(|r| (r.id, r.status.clone()));
        match run {
            None => self.sweep.step_seen = None,
            Some((id, status)) => {
                let same = self
                    .sweep
                    .step_seen
                    .as_ref()
                    .is_some_and(|(i, s, _)| *i == id && *s == status);
                if !same {
                    self.sweep.step_seen = Some((id, status, self.now_s));
                }
            }
        }
    }

    /// Takes the preferences read at startup, and the view choices they hold: the layout as
    /// last left (the measurements each pane showed come back once the daemon's state is
    /// known). Nothing here arms or plays anything.
    pub fn set_prefs(&mut self, prefs: UiPrefs) {
        self.view.spl.layout = prefs.leq;
        let l = &prefs.layout;
        self.pending_pane_meas.clear();
        match &l.panes {
            Some(p) => {
                let views = p
                    .views
                    .iter()
                    .map(|v| {
                        (
                            v.id,
                            View {
                                kind: v.kind,
                                meas: None,
                                modes: v.modes,
                            },
                        )
                    })
                    .collect();
                self.layout = Layout::of(p.root.clone(), views, p.focus);
                for v in &p.views {
                    if let Some(n) = &v.measurement
                        && self.layout.views.contains_key(&v.id)
                    {
                        self.pending_pane_meas.insert(v.id, n.clone());
                    }
                }
                self.pane_auto = false;
            }
            None => {
                self.layout = Layout::default();
                self.pane_auto = true;
            }
        }
        self.layout.maximized = l.maximized;
        self.fullscreen = l.fullscreen;
        self.view.distortion.unit = l.distortion_unit;
        // Before any frame: a spectrum that starts still fits its axis on its first one.
        prefs.levels.apply(&mut self.view);
        prefs.legend.apply(&mut self.view.tf.legend);
        if let Some(s) = prefs.spectrograph_span_s {
            self.view.spectrum.spectrograph.span_s = s;
        }
        self.view.unresolved = prefs.resolution_marker;
        self.hidden_meas = l.hidden.clone();
        self.compared_meas = l.compared.clone();
        self.prefs = prefs;
    }

    /// W: the split layout → the focused pane alone → the focused pane full screen (the
    /// window too) → the split layout again.
    pub(super) fn cycle_layout(&mut self) {
        if !self.layout.maximized {
            self.layout.maximized = true;
        } else if !self.fullscreen {
            self.fullscreen = true;
        } else {
            self.layout.maximized = false;
            self.fullscreen = false;
        }
    }

    /// The stage view: the focused pane alone with the window full screen, so the screen
    /// holds only the pane's picture (no top bar, list or pane title; the SPL meter and its
    /// Leq windows read across a room). Full screen is the operator's explicit choice to see
    /// only the pane: the stimulus arming, playing or stopping and a sweep running change
    /// nothing on it (no top bar, strip or badge comes back, no pane resizes), and Esc and
    /// Shift+Esc stop as everywhere. Outside full screen the top bar shows the stimulus.
    pub fn stage_view(&self) -> bool {
        self.fullscreen && self.layout.maximized
    }

    /// Whether the panes show key hints now: on in the preferences, and never in the stage
    /// view (the audience sees the windows alone).
    pub fn key_hints_shown(&self) -> bool {
        self.prefs.key_hints && !self.stage_view()
    }

    /// The hint line of `pane`: its most used keys as bound in `keymap`, written in `style`,
    /// then the help key. Only the focused pane has one, and only while hints are shown.
    /// Commands that do nothing in the pane's present view are left out (the sweep pane's
    /// dB / % while it shows the IR, its IR mode while it shows distortion).
    pub fn key_hint_line(
        &self,
        keymap: &Keymap,
        pane: PaneId,
        style: crate::keys::LabelStyle,
    ) -> Option<Vec<crate::hints::KeyHint>> {
        if !self.key_hints_shown() || self.layout.focus != pane {
            return None;
        }
        Some(self.pane_hints(keymap, pane, style))
    }

    /// Every hint of `pane` (the title's tooltip lists them whether or not the line shows).
    pub fn pane_hints(
        &self,
        keymap: &Keymap,
        id: PaneId,
        style: crate::keys::LabelStyle,
    ) -> Vec<crate::hints::KeyHint> {
        let pane = self.layout.kind(id);
        let mode = self
            .layout
            .view(id)
            .map_or(SweepMode::Response, |v| v.modes.sweep);
        let scope = self.layout.view(id).map_or(pane.scope(), |v| v.scope());
        let mut line = crate::hints::line(keymap, scope, style, |c| {
            pane == PaneKind::Distortion && c == CommandId::IrMode && mode != SweepMode::Ir
        });
        // G reaches the band view only when a meter has a band meter (`SplMode::next`).
        if !crate::scenes::has_band_meter(self) {
            for h in line
                .iter_mut()
                .filter(|h| h.command == CommandId::SplLeqView)
            {
                h.name = crate::keys::SPL_VIEWS_NO_BANDS;
            }
        }
        line
    }

    /// The multi-step operation running on the daemon (a set of sweeps), as the progress
    /// strip shows it.
    pub fn operation(&self) -> Option<ac2_scene::progress::Progress> {
        let run = self.daemon()?.sweep.as_ref()?;
        let since = self
            .sweep
            .step_seen
            .as_ref()
            .filter(|(i, s, _)| *i == run.id && *s == run.status)
            .map_or(0.0, |(_, _, t)| self.now_s - t);
        ac2_scene::progress::sweep(run, since)
    }

    /// Every input the open session captures, labelled by name and role, with its meter
    /// and what the running sweep (else the selected measurement) uses it as.
    pub fn session_inputs(&self) -> Vec<ac2_scene::meter::InputRow> {
        use ac2_scene::meter::{InputRole, InputRow, InputUse, MeterReading, input_label};
        let Some(o) = self.open_session() else {
            return Vec::new();
        };
        let meters = if self.overlay.session().is_some() {
            // The dialog's meters may be another device's preview.
            BTreeMap::new()
        } else {
            self.input_meters()
        };
        let device_names = self
            .devices
            .iter()
            .flatten()
            .filter(|b| b.kind == o.backend)
            .flat_map(|b| &b.devices)
            .find(|d| d.id == o.input_device)
            .and_then(|d| d.input.as_ref())
            .and_then(|i| i.channel_names.clone());
        let ms = self.measurements();
        let loopback = o.config.loopback.map(|l| l.input);
        let run = self
            .daemon()
            .and_then(|s| s.sweep.as_ref())
            .filter(|r| r.active());
        let uses: Vec<(u16, InputUse)> = match (run, self.selected_meas()) {
            (Some(r), _) => vec![
                (r.reference_input, InputUse::Reference),
                (r.measurement_input, InputUse::Measurement),
            ],
            (None, Some(m)) => match &m.config.kind {
                MeasKind::Transfer { config } => vec![
                    (config.reference_input, InputUse::Reference),
                    (config.measurement_input, InputUse::Measurement),
                ],
                MeasKind::Spectrum { config } => vec![(config.input, InputUse::Measurement)],
                MeasKind::Rta { config } => vec![(config.input, InputUse::Measurement)],
                MeasKind::Spl { config } => vec![(config.input, InputUse::Measurement)],
                MeasKind::Sweep { config } => vec![
                    (config.reference_input, InputUse::Reference),
                    (config.measurement_input, InputUse::Measurement),
                ],
                // Every live operand's inputs.
                MeasKind::Math { config } => ms
                    .iter()
                    .filter(|x| config.expr.names(Operand::Meas { meas: x.id }))
                    .flat_map(|x| match &x.config.kind {
                        MeasKind::Transfer { config } => vec![
                            (config.reference_input, InputUse::Reference),
                            (config.measurement_input, InputUse::Measurement),
                        ],
                        MeasKind::Spectrum { config } => {
                            vec![(config.input, InputUse::Measurement)]
                        }
                        MeasKind::Rta { config } => vec![(config.input, InputUse::Measurement)],
                        _ => Vec::new(),
                    })
                    .collect(),
            },
            (None, None) => Vec::new(),
        };
        o.config
            .input_channels
            .iter()
            .map(|&c| {
                let mic = self.input_setup(c).mic;
                let dev = device_names
                    .as_ref()
                    .and_then(|n| n.get(usize::from(c)).cloned());
                let name = ac2_scene::meter::input_name(c, mic.as_deref(), dev.as_deref());
                let is_ref = loopback == Some(c)
                    || ms.iter().any(|m| {
                        matches!(&m.config.kind, MeasKind::Transfer { config }
                            if config.reference_input == c)
                    });
                let is_mic = mic.is_some()
                    || ms.iter().any(|m| {
                        matches!(&m.config.kind, MeasKind::Transfer { config }
                            if config.measurement_input == c)
                    });
                let role = if is_ref {
                    Some(InputRole::Reference)
                } else if is_mic {
                    Some(InputRole::Mic)
                } else {
                    None
                };
                let curve = self.daemon().and_then(|s| {
                    ac2_scene::cal::curve_short(&ac2_proto::cal::state_input_use(s, c).curve)
                });
                InputRow {
                    channel: c,
                    label: input_label(c, &name, curve.as_deref(), role),
                    used: uses.iter().find(|(i, _)| *i == c).map(|(_, u)| *u),
                    reading: meters.get(&c).cloned().unwrap_or_else(MeterReading::none),
                }
            })
            .collect()
    }

    /// Subscribes, opens, renews and closes what [`Self::meter_wants`] changed to.
    pub(super) fn sync_meters(
        &mut self,
        before: (
            bool,
            Option<(ac2_proto::model::BackendKind, ac2_proto::model::DeviceId)>,
        ),
        tick: bool,
        out: &mut Vec<Request>,
    ) {
        let after = self.meter_wants();
        if before.0 != after.0 {
            out.push(Request::Meters(after.0));
        }
        let renew = tick && self.now_s - self.preview_sent_s >= PREVIEW_RENEW_S;
        match (&before.1, &after.1) {
            (_, Some((backend, device))) if before.1 != after.1 || renew => {
                out.push(Request::Preview {
                    backend: *backend,
                    device: device.clone(),
                });
                self.preview_sent_s = self.now_s;
            }
            (Some(_), None) => out.push(Request::PreviewStop),
            _ => {}
        }
    }

    /// Input meters of the session dialog's device (its preview, or the session's own
    /// meters when the session captures it) or, for the measurement dialogs, of the
    /// session: device input → reading. Stale frames read as nothing.
    pub fn input_meters(&self) -> BTreeMap<u16, ac2_scene::meter::MeterReading> {
        use ac2_proto::FrameData;
        use ac2_proto::topic::Topic;
        let mut out = BTreeMap::new();
        let Some(d) = &self.data else {
            return out;
        };
        let from_preview = self.overlay.session().and_then(|s| s.preview_target());
        let topic = if from_preview.is_some() {
            Topic::PreviewLevels
        } else {
            Topic::SessionLevels
        };
        let Some(tf) = d.latest.get(&topic).filter(|f| !f.stale) else {
            return out;
        };
        let (channels, peak, rms, clip) = match &tf.frame.data {
            FrameData::SessionLevels(f) => (&f.meta.channels, &f.peak, &f.rms, &f.clip),
            FrameData::PreviewLevels(f) => {
                if from_preview.as_ref() != Some(&(f.meta.backend, f.meta.device.clone())) {
                    return out;
                }
                (&f.meta.channels, &f.peak, &f.rms, &f.clip)
            }
            _ => return out,
        };
        for (i, c) in channels.iter().enumerate() {
            let (Some(p), Some(r), Some(k)) = (peak.get(i), rms.get(i), clip.get(i)) else {
                continue;
            };
            out.insert(
                *c,
                ac2_scene::meter::MeterReading::new(
                    *p,
                    *r,
                    *k != ac2_proto::frame::ClipFlags::NONE,
                ),
            );
        }
        out
    }

    /// The names of the open session's outputs: the device's channel name, else `Output N`.
    pub fn session_output_names(&self) -> Vec<(u16, String)> {
        let Some(o) = self.open_session() else {
            return Vec::new();
        };
        let device_names = self
            .devices
            .iter()
            .flatten()
            .filter(|b| b.kind == o.backend)
            .flat_map(|b| &b.devices)
            .find(|d| d.id == o.output_device)
            .and_then(|d| d.output.as_ref())
            .and_then(|i| i.channel_names.clone());
        (0..o.config.output_channels)
            .map(|c| {
                let name = device_names
                    .as_ref()
                    .and_then(|n| n.get(usize::from(c)).cloned())
                    .unwrap_or_else(|| format!("Output {}", c + 1));
                (c, name)
            })
            .collect()
    }

    /// The names of the open session's inputs, as the dialogs show them: mic name, else the
    /// device's channel name, else `Input N`.
    pub fn session_input_names(&self) -> Vec<(u16, String)> {
        self.session_input_labels()
            .into_iter()
            .map(|(c, name)| (c, ac2_scene::meter::channel_choice(c, &name)))
            .collect()
    }

    /// The open session's inputs by name alone (mic, device channel name or `Input N`).
    pub fn session_input_labels(&self) -> Vec<(u16, String)> {
        let Some(o) = self.open_session() else {
            return Vec::new();
        };
        let device_names = self
            .devices
            .iter()
            .flatten()
            .filter(|b| b.kind == o.backend)
            .flat_map(|b| &b.devices)
            .find(|d| d.id == o.input_device)
            .and_then(|d| d.input.as_ref())
            .and_then(|i| i.channel_names.clone());
        o.config
            .input_channels
            .iter()
            .map(|&c| {
                let mic = self.input_setup(c).mic;
                let dev = device_names
                    .as_ref()
                    .and_then(|n| n.get(usize::from(c)).cloned());
                (
                    c,
                    ac2_scene::meter::input_name(c, mic.as_deref(), dev.as_deref()),
                )
            })
            .collect()
    }
}
