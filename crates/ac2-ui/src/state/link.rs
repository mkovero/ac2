//! Events from the daemon link: connection, mirror and data updates, stimulus events, found delays, spectrograph and peak folding.

use super::*;

/// What waits for the mirror to list it: this client's creations, and choices remembered by
/// measurement name from the app's last run.
#[derive(Clone, Debug, Default)]
pub(crate) struct Pending {
    /// A measurement this client just created: selected once the mirror lists it.
    pub(super) select: Option<MeasId>,
    /// That measurement, created: stacked once the mirror lists it.
    pub(super) stack: Option<MeasId>,
    /// That measurement, opened on a screen of its own: the screen stored as its own panes
    /// once the mirror lists it, so its row brings them back before any edit.
    pub(super) own_layout: Option<MeasId>,
    /// The measurement each pane showed when the app last ran, by name, until the daemon's
    /// state is known.
    pub(super) pane_meas: BTreeMap<PaneId, String>,
    /// The name of what a pane showed until its rig reported it deleted: the pane says so
    /// until it is given something else.
    pub(super) deleted: BTreeMap<PaneId, String>,
    /// Entries behind a pane's lead read from `ui.toml`, with the names they had: once the
    /// daemon's state is known, each takes the id of what has its name now (a daemon
    /// restarted numbers afresh), else keeps its id.
    pub(super) with_names: BTreeMap<PaneId, Vec<(Pick, String)>>,
    /// Hidden measurements by name, waiting for the daemon to list one of that name: read
    /// from `ui.toml`, or a hidden one the daemon took away (a session reopening, a daemon
    /// restarting renumbers them).
    pub(super) hidden: BTreeSet<String>,
}

impl AppState {
    pub(super) fn conn_event(&mut self, e: ConnEvent, out: &mut Vec<Request>) {
        match e {
            ConnEvent::Connecting { target } => {
                if !matches!(self.rig.conn, ConnState::Failed { .. }) {
                    self.rig.conn = ConnState::Connecting { target };
                }
            }
            ConnEvent::Connected {
                target,
                server,
                client_id,
                rig_id,
                name,
            } => {
                if self.is_duplicate(rig_id) {
                    self.rig.duplicate = true;
                    return;
                }
                // A window opened before the hello acts on this rig, now named by it.
                if self.window_rig == Some(self.rig.id) {
                    self.window_rig = Some(rig_id);
                }
                self.rig.connected(ConnState::Connected {
                    target,
                    server,
                    client_id,
                    rig_id,
                    name,
                });
                self.remember_rig();
                if let Overlay::Settings(s) = &mut self.overlay {
                    s.session.backends = None;
                    out.push(Request::Devices);
                }
            }
            ConnEvent::Failed { target, error, .. } => {
                // Whatever daemon answers next, its ids are its own: what is shown waits
                // for its name.
                let names = self.meas_names();
                // The daemon reached next may be another, numbering afresh: names only.
                self.park_gone_meas(&names, false, &BTreeSet::new(), |_| true);
                self.rig.failed(target, error);
            }
            ConnEvent::Mirror(v) => {
                let names = self.meas_names();
                let incarnation = self.rig.mirror.as_ref().and_then(|m| m.incarnation);
                // Another daemon (restarted, swapped in) numbers its measurements afresh: an
                // id it has may be another measurement than the one shown.
                let reborn = incarnation.is_some() && v.incarnation != incarnation;
                let (was_stopped, was_firing) = self.daemon().map_or((true, false), |s| {
                    (s.session.stopped.is_some(), s.generator.firing)
                });
                let traces_before: BTreeSet<TraceId> = self
                    .daemon()
                    .map(|s| s.traces.iter().map(|t| t.id).collect())
                    .unwrap_or_default();
                self.rig.mirror = Some(v);
                let gone_traces: BTreeSet<TraceId> = match self.daemon() {
                    Some(s) if !reborn => {
                        let now: BTreeSet<TraceId> = s.traces.iter().map(|t| t.id).collect();
                        traces_before.difference(&now).copied().collect()
                    }
                    _ => BTreeSet::new(),
                };
                // A driver reset or clock change: the backend cut the output at once and the
                // daemon disarmed the generator; say so when it happens, not only in the bar.
                if !was_stopped
                    && let Some(ac2_proto::model::StopCause::DriverReset { reason }) = self
                        .daemon()
                        .and_then(|s| s.session.stopped.as_ref())
                        .map(|s| s.cause)
                {
                    let line = ac2_scene::audio::driver_reset_line(reason, was_firing);
                    if was_firing {
                        self.fault(line);
                    } else {
                        self.warn(line);
                    }
                }
                let seen: Vec<String> = self
                    .measurements()
                    .iter()
                    .map(|m| m.config.name.clone())
                    .collect();
                self.rig.meas_seen.extend(seen);
                // The daemon disarmed the generator when the audio stopped; an arm queued
                // behind a stop must not follow it into a session that is reopening.
                if self.rig.stimulus.arm_after_stop
                    && self.daemon().is_some_and(|s| s.session.stopped.is_some())
                {
                    self.rig.stimulus.arm_after_stop = false;
                    self.warn(
                        "not armed: the audio stopped; the daemon is reopening the audio I/O",
                    );
                }
                let ids: Vec<MeasId> = self.measurements().iter().map(|m| m.id).collect();
                if let Some(p) = self.rig.pending.select
                    && ids.contains(&p)
                {
                    self.rig.selected = Some(p);
                    self.rig.chosen = Some(p);
                    self.rig.pending.select = None;
                }
                if let Some(p) = self.rig.pending.stack
                    && ids.contains(&p)
                {
                    self.rig.pending.stack = None;
                    self.stack_created(p);
                }
                if let Some(p) = self.rig.pending.own_layout
                    && ids.contains(&p)
                {
                    self.rig.pending.own_layout = None;
                    if self.rig.own_panes == Some(p) {
                        self.store_layout(p, out);
                    }
                }
                let waiting = self.rig.pending.select.is_some()
                    && self.rig.pending.select == self.rig.selected;
                // A measurement this app made, selected before the daemon lists it, stays.
                let keep = if waiting { self.rig.selected } else { None };
                self.park_gone_meas(&names, !reborn, &gone_traces, |id| {
                    Some(id) != keep && (reborn || !ids.contains(&id))
                });
                if let Some(id) = self.rig.lost_selection.as_ref().and_then(|n| {
                    self.measurements()
                        .iter()
                        .find(|m| m.config.name == *n)
                        .map(|m| m.id)
                }) {
                    self.rig.selected = Some(id);
                    self.rig.chosen = Some(id);
                    self.rig.standin = None;
                    self.rig.lost_selection = None;
                    if std::mem::take(&mut self.rig.lost_own_panes) {
                        self.rig.own_panes = Some(id);
                    }
                }
                if !waiting && self.rig.selected.is_none() {
                    self.rig.selected = self
                        .measurements()
                        .iter()
                        .find(|m| matches!(m.config.kind, MeasKind::Transfer { .. }))
                        .or(self.measurements().first())
                        .map(|m| m.id);
                    self.rig.chosen = self.rig.selected;
                    self.rig.standin = self.rig.selected;
                }
                self.rig.keep_listed(reborn, keep);
                if let Some(TraceKey::Stored(r)) = self.view.tf.phase_reference
                    && !self
                        .daemon()
                        .is_some_and(|st| st.traces.iter().any(|t| t.id == r))
                {
                    self.view.tf.phase_reference = None;
                }
                self.rig.follow_spectrum_starts();
                self.follow_output_device();
                self.follow_settings();
                self.follow_sweep(out);
                self.follow_leq_alarms();
                let data = self.rig.data.clone();
                self.follow_leq_logs(data.as_deref(), out);
                if self.rig.open_session_when_empty && self.connected() && self.daemon().is_some() {
                    self.rig.open_session_when_empty = false;
                    // A session kept from before (autosave, a loaded file) is no fresh
                    // start: its hint names the audio page, nothing opens over it.
                    if self.nothing_kept() && self.overlay == Overlay::None {
                        self.open_sessions(crate::sessions::Page::Main, out);
                    }
                }
                self.choose_first();
            }
            ConnEvent::Devices(r) => {
                if let Ok(d) = &r {
                    self.rig.devices = Some(d.clone());
                }
                let names = self.session_input_names();
                match (&mut self.overlay, r) {
                    (Overlay::Settings(s), Ok(d)) => {
                        s.session.set_backends(d, self.prefs.of_rig(self.rig.id));
                        s.follow_focus_want();
                    }
                    (Overlay::Settings(s), Err(e)) => {
                        s.session.error = Some(format!("cannot list devices: {e}"));
                    }
                    (Overlay::Form(f), Ok(_)) => {
                        // Channel names arrived: relabel the inputs.
                        relabel(f, &names);
                    }
                    // The dialog was closed meanwhile.
                    _ => {}
                }
            }
            ConnEvent::Preview {
                backend,
                device,
                result,
            } => {
                // The answer for a device the dialog has since left says nothing about the
                // one it shows now.
                if let Some(s) = self.overlay.session_mut()
                    && s.preview_target() == Some((backend, device))
                {
                    s.preview_error = result.err().map(|e| format!("meters unavailable: {e}"));
                }
            }
            ConnEvent::LoopbackDetected(r) => {
                if let Overlay::Settings(s) = &mut self.overlay {
                    s.session.detect_result(r);
                    // The detection closed the preview on the daemon: reopen it now.
                    self.rig.preview_sent_s = f64::NEG_INFINITY;
                } else if let Err(e) = r {
                    self.fault(format!("detect loopback: {e}"));
                }
            }
            ConnEvent::SessionOpened => {
                self.rig.choose_when_open = true;
                self.choose_first();
            }
            ConnEvent::LeqBackfill { meas, ask, result } => {
                self.leq_backfilled(meas, ask, result);
            }
            ConnEvent::LeqSpan { meas, ask, result } => {
                self.leq_span_fetched(meas, ask, result);
            }
            ConnEvent::BandLog { ask, result } => self.band_log_answered(ask, result),
            ConnEvent::BandTransfer(result) => self.band_transfer_answered(result),
            ConnEvent::Sessions(o) => self.session_outcome(*o, out),
            ConnEvent::SessionProgress(t) => self.session_progress(t),
            ConnEvent::SessionList(r) => self.session_list(r),
            ConnEvent::WalkCreated(w) => self.walk_created(*w),
            ConnEvent::MeasCreated(m) => {
                // A new measurement opens on a screen of its own ([`Self::open_new`]): the
                // panes on screen stay the layout of the one they belong to (a split still
                // being dragged is stored there first). Before it is selected: focusing a
                // sweep pane selects what that pane shows.
                let mine = |n: &Option<String>| n.as_ref() == Some(&m.config.name);
                let split = self
                    .rig
                    .split_on_create
                    .take_if(|(name, _)| *name == m.config.name)
                    .map(|(_, view)| view);
                let stack = mine(&self.rig.stack_on_create);
                if mine(&self.rig.pane_on_create) || split.is_some() || stack {
                    self.rig.pane_on_create = None;
                    self.flush_layout_drag(out);
                    let kind = match split {
                        Some(_) => PaneKind::Transfer,
                        None => PaneKind::for_kind(&m.config.kind),
                    };
                    self.open_new(m.id, kind, split.unwrap_or_default(), stack);
                    self.rig.pending.own_layout = Some(m.id);
                }
                // Its header row is the selection, unfolded with its live curve under it,
                // and the list scrolls to it.
                self.rig.selected = Some(m.id);
                self.rig.chosen = Some(m.id);
                self.rig.selected_trace = None;
                self.rig.selected_live = None;
                self.rig.collapsed.remove(&TraceOwner::Meas { meas: m.id });
                self.tree_reveal = true;
                self.rig.pending.select = Some(m.id);
                // Made here, by the operator: as if its row were chosen, the screen it opened
                // on is its own.
                self.rig.own_panes = Some(m.id);
                if self.rig.stack_on_create.as_ref() == Some(&m.config.name) {
                    self.rig.stack_on_create = None;
                    self.rig.pending.stack = Some(m.id);
                }
            }
            ConnEvent::Data(d) => {
                let fold = self.folding();
                for meas in self.rig.fold_data(d, &fold, out) {
                    self.forget_leq_strips(meas);
                }
                self.fit_first_shows();
                self.feed_follow();
            }
            ConnEvent::Trace(t, g) => {
                self.rig.keep_trace(t, g);
                self.fit_new_sweep();
            }
            ConnEvent::Reply { what, result } if self.layout_reply(&what, &result, out) => {}
            ConnEvent::Reply { what, result } if self.eq_reply(&what, &result, out) => {}
            ConnEvent::Reply { what, result } => {
                if let Overlay::Settings(s) = &mut self.overlay {
                    s.cal.reply(&what, &result);
                }
                match result {
                    Ok(()) => self.toast(what),
                    Err(e) => self.fault(format!("{what}: {e}")),
                }
            }
            ConnEvent::Conflict { what, error } => {
                if !self.layout_reply(&what, &Err(error.clone()), out)
                    && !self.eq_reply(&what, &Err(error), out)
                {
                    self.conflict(&what, out);
                }
            }
            ConnEvent::Warnings(w) => self.reply_warnings(&w),
            ConnEvent::DelayFound {
                meas,
                pick,
                finding,
            } => self.delay_found(meas, pick, *finding, out),
            ConnEvent::Captured { slot, trace } => {
                self.toast(format!("slot {slot}: {} captured", trace.edit.name));
            }
            ConnEvent::Stimulus(s) => self.stim_event(s, out),
            ConnEvent::Server { what, result } => {
                if let Overlay::Settings(s) = &mut self.overlay {
                    match (&result, &what) {
                        // A failed change leaves the info shown as it was.
                        (Err(_), Some(_)) => {}
                        _ => s.connection.server = Some(result.clone()),
                    }
                }
                match (what, result) {
                    (Some(w), Ok(_)) => self.toast(w),
                    (Some(w), Err(e)) => self.fault(format!("{w}: {e}")),
                    (None, _) => {}
                }
            }
        }
    }

    /// X inserts the first arrival, Shift+X the strongest — when the finder accepted. An
    /// ambiguous first arrival is the operator's to pick (decision 1c): the candidate list
    /// opens. A refusal inserts nothing and says why (the banner keeps saying it).
    pub(super) fn delay_found(
        &mut self,
        meas: MeasId,
        pick: DelayPick,
        finding: DelayFinding,
        out: &mut Vec<Request>,
    ) {
        let name = self.meas(meas).map_or_else(
            || format!("measurement {}", meas.0),
            |m| m.config.name.clone(),
        );
        match (&finding.outcome, pick) {
            (DelayOutcome::NoEstimate { reasons }, _) => self.warn(format!(
                "{name}: no delay estimate ({})",
                ac2_scene::finding::no_estimate_reasons(reasons)
            )),
            (DelayOutcome::Ambiguous { .. }, DelayPick::FirstArrival) => {
                self.overlay = Overlay::DelayPick(Box::new(DelayChoice {
                    meas,
                    name,
                    finding,
                    selected: 0,
                }));
            }
            _ => {
                let what = match pick {
                    DelayPick::Strongest => "strongest arrival inserted",
                    _ => "first arrival inserted",
                };
                self.call(
                    out,
                    Command::DelayInsert { meas, pick },
                    format!("{name}: {what}"),
                );
            }
        }
    }

    pub(super) fn stim_event(&mut self, s: StimEvent, out: &mut Vec<Request>) {
        match s {
            StimEvent::Armed => {
                self.rig.stimulus.phase = StimPhase::Armed;
                if self.rig.armed_with.take() != self.rig.stimulus.settings() {
                    self.resend_stimulus(out);
                }
                let d = self.rig.stimulus.describe();
                if self.rig.sweep.plan.is_some() {
                    self.toast(format!("armed: {d} · Enter plays the sweep · Esc stops"));
                } else {
                    self.toast(format!("armed: {d} · Enter fires · Esc stops"));
                }
            }
            StimEvent::SweepStarted(run) => {
                self.rig.stimulus.phase = StimPhase::Firing;
                self.rig.sweep.run = Some(run.id);
                self.rig.sweep.seen = Some(run.status.clone());
                self.toast(format!(
                    "sweep playing: {} × {} s · Esc stops (and discards it)",
                    run.repeats,
                    format::fixed(run.sweep_duration.0 + run.post_roll.0, 1)
                ));
            }
            StimEvent::Set { firing } => {
                if self.rig.stimulus.phase != StimPhase::Stopping {
                    self.rig.stimulus.phase = if firing {
                        StimPhase::Firing
                    } else {
                        StimPhase::Armed
                    };
                }
            }
            StimEvent::Stopped => {
                self.rig.stimulus.phase = StimPhase::Idle;
                let announced = std::mem::take(&mut self.rig.stimulus.stop_announced);
                if std::mem::take(&mut self.rig.stimulus.arm_after_stop)
                    && !self.rig.sweep.arm_after_stop
                {
                    self.arm_after_stopped(out);
                }
                let releasing = std::mem::take(&mut self.rig.sweep.releasing);
                let pending = std::mem::take(&mut self.rig.sweep.arm_after_stop);
                match self.rig.sweep.plan.take() {
                    Some(plan) if pending => self.arm_sweep(plan, false, out),
                    plan => {
                        self.rig.sweep.plan = plan;
                        self.end_sweep_mode();
                        if !releasing && !announced {
                            self.toast("stimulus stopped");
                        }
                    }
                }
            }
            StimEvent::Lost(msg) => {
                self.rig.stimulus.phase = StimPhase::Idle;
                self.rig.stimulus.arm_after_stop = false;
                self.rig.stimulus.stop_announced = false;
                self.rig.sweep.releasing = false;
                self.rig.sweep.arm_after_stop = false;
                self.end_sweep_mode();
                self.fault(format!("stimulus lease lost: {msg}"));
            }
            StimEvent::Failed(msg) => {
                self.rig.sweep.releasing = false;
                self.rig.stimulus.arm_after_stop = false;
                self.rig.stimulus.stop_announced = false;
                if std::mem::take(&mut self.rig.sweep.arm_after_stop) {
                    self.end_sweep_mode();
                }
                self.rig.stimulus.phase = match self.rig.stimulus.phase {
                    StimPhase::Arming => StimPhase::Idle,
                    StimPhase::FireRequested => StimPhase::Armed,
                    // A failed stop leaves the truth to the mirror; the daemon's lease
                    // expiry fades the output out if this client cannot reach it.
                    StimPhase::Stopping => StimPhase::Idle,
                    p => p,
                };
                self.fault(format!("stimulus: {msg}"));
            }
        }
    }
}

impl AppState {
    /// The name of each measurement the daemon's state holds now.
    fn meas_names(&self) -> BTreeMap<MeasId, String> {
        self.measurements()
            .iter()
            .map(|m| (m.id, m.config.name.clone()))
            .collect()
    }

    /// Measurements `gone` from the daemon (`names`: what they were called). `same_ids`: the
    /// daemon keeps numbering as before, so a gone id is one it deleted: it leaves every
    /// pane of this rig, which says what was deleted, and a lead waits for one of its name
    /// (a session reopened makes them again under new ids). Otherwise (a restart, another
    /// daemon) each pane that showed one waits for one of that name, and the entries behind
    /// a lead wait by id. A trace entry the daemon deleted (`gone_traces`) leaves too. The
    /// selection waits by name too (a stand-in may take its place).
    fn park_gone_meas(
        &mut self,
        names: &BTreeMap<MeasId, String>,
        same_ids: bool,
        gone_traces: &BTreeSet<TraceId>,
        gone: impl Fn(MeasId) -> bool,
    ) {
        let (rig, tab) = (self.rig.id, self.active());
        for (pane, v) in &mut self.layout.views {
            // Another rig's pane names that rig's measurements.
            let ours = match v.rig {
                PaneRig::Active => tab == rig,
                PaneRig::Rig(r) => r == rig,
            };
            if !ours {
                continue;
            }
            let lead = v.meas.filter(|id| gone(*id));
            if let Some(id) = lead {
                v.meas = None;
                if let Some(n) = names.get(&id) {
                    self.rig.pending.pane_meas.insert(*pane, n.clone());
                    if same_ids {
                        self.rig.pending.deleted.insert(*pane, n.clone());
                    }
                }
            }
            if same_ids {
                v.with.retain(|p| {
                    p.rig.is_some()
                        || match p.what {
                            EntryRef::Group { meas } | EntryRef::Live { meas } => !gone(meas),
                            EntryRef::Trace { trace } => !gone_traces.contains(&trace),
                        }
                });
                if lead.is_some()
                    && let Some(first) = v.with.as_slice().first().copied()
                    && first.rig.is_none()
                    && let EntryRef::Group { meas } = first.what
                {
                    // The next entry leads.
                    v.with.take_first();
                    v.meas = Some(meas);
                    self.rig.pending.pane_meas.remove(pane);
                    self.rig.pending.deleted.remove(pane);
                }
            }
        }
        // Hidden ones wait by name as a pane's measurement does, unless one
        // that stays has that name: it is another measurement, and the choice was not its.
        let staying: BTreeSet<String> = self
            .measurements()
            .iter()
            .filter(|m| !gone(m.id))
            .map(|m| m.config.name.clone())
            .collect();
        let pending = &mut self.rig.pending.hidden;
        self.rig.hidden_meas.retain(|id| {
            if !gone(*id) {
                return true;
            }
            if let Some(n) = names.get(id).filter(|n| !staying.contains(*n)) {
                pending.insert(n.clone());
            }
            false
        });
        if let Some(id) = self.rig.selected.filter(|id| gone(*id)) {
            // A stand-in going leaves the operator's choice waiting.
            if self.rig.standin != Some(id)
                && let Some(n) = names.get(&id)
            {
                self.rig.lost_selection = Some(n.clone());
                self.rig.lost_own_panes = self.rig.own_panes == Some(id);
            }
            self.rig.selected = None;
            self.rig.standin = None;
        }
        if self.rig.chosen.is_some_and(&gone) {
            self.rig.chosen = None;
        }
        // Its panes stay on screen, nobody's own until a row is chosen.
        if self.rig.own_panes.is_some_and(&gone) {
            self.rig.own_panes = None;
        }
    }

    /// The selection moved off the stand-in (the operator chose, this app made something),
    /// or the operator chose the stand-in's row: the selection is theirs again.
    pub(super) fn follow_standin(&mut self) {
        if self.rig.own_panes_due
            || self
                .rig
                .selected
                .is_some_and(|s| Some(s) != self.rig.standin)
        {
            self.rig.standin = None;
            self.rig.lost_selection = None;
            self.rig.lost_own_panes = false;
        }
    }

    /// The selection is one this app made by itself.
    pub(super) fn selection_is_standin(&self) -> bool {
        self.rig.selected.is_some() && self.rig.selected == self.rig.standin
    }
}

impl RigState {
    /// Note the spectrum / RTA measurements that started since the last state: each fits
    /// the level axis on its first frame.
    pub(super) fn follow_spectrum_starts(&mut self) {
        let running: BTreeSet<MeasId> = self
            .measurements()
            .iter()
            .filter(|m| m.running && spectrum_stream(m).is_some())
            .map(|m| m.id)
            .collect();
        for &id in running.difference(&self.spectrum_running) {
            let shown = self.spectrum_frame(id).map(|f| f.frame.clone());
            self.spectrum_fit.insert(id, shown);
        }
        self.spectrum_fit.retain(|id, _| running.contains(id));
        self.spectrum_running = running;
    }

    /// The newest frame of spectrum / RTA measurement `id`.
    pub(super) fn spectrum_frame(&self, id: MeasId) -> Option<&ac2_client::TopicFrame> {
        let m = self.meas(id)?;
        let stream = spectrum_stream(m)?;
        self.data
            .as_ref()?
            .latest
            .get(&Topic::Data { meas: id, stream })
    }

    /// Folds the newest spectrum / RTA frames into the spectrograph histories. A stream
    /// that is STALE, or whose measurement is not running, marks a break: the time until
    /// its next frame is a gap in the picture. With `ease`, each history's 3D rows glide to
    /// where a frame moves them ([`RowGlide`]).
    pub(super) fn fold_spectrographs(&mut self, d: &DataSnapshot, ease: bool) {
        use ac2_proto::FrameData;
        use ac2_scene::spectrograph::{SpectrographFrame, SpectrographHistory};
        // Each measurement's own span; a history of another span cannot hold its slots.
        let running: BTreeMap<MeasId, (bool, u32)> = self
            .measurements()
            .iter()
            .map(|m| (m.id, (m.running, self.display_of(m).spectrograph_span_s)))
            .collect();
        self.spectrographs
            .retain(|id, h| running.get(id).is_some_and(|(_, s)| h.span_s() == *s));
        if ease {
            let kept = &self.spectrographs;
            self.row_glide.retain(|id, _| kept.contains_key(id));
            self.row_pacing.retain(|id, _| kept.contains_key(id));
        } else {
            self.row_glide.clear();
            self.row_pacing.clear();
        }
        for tf in d.latest.frames.values() {
            let (meas, scale, spec_level, level, validity) = match &tf.frame.data {
                FrameData::Spec(f) => (f.meas, f.meta.scale, Some(f.meta.level), &f.level, None),
                FrameData::Rta(f) => (
                    f.meas,
                    f.meta.scale,
                    None,
                    &f.level,
                    Some(f.validity.as_slice()),
                ),
                _ => continue,
            };
            let Some(&(live, span)) = running.get(&meas) else {
                continue;
            };
            let Some((grid, def)) = tf
                .frame
                .stamp
                .grid_id
                .and_then(|g| d.grids.get(&g).map(|def| (g, def)))
            else {
                continue;
            };
            let h = self
                .spectrographs
                .entry(meas)
                .or_insert_with(|| SpectrographHistory::new(span));
            if tf.stale || !live {
                h.mark_break();
                continue;
            }
            let cols = ac2_scene::grid::columns(def);
            let before = super::row_glide::mark(h);
            let moved = h.push(&SpectrographFrame {
                seq: tf.frame.stamp.seq,
                at: tf.frame.stamp.capture_wall_ns,
                grid,
                edges: &cols.edges,
                scale,
                spec_level,
                level,
                validity,
            });
            if ease && moved {
                let wall = tf.frame.stamp.capture_wall_ns.0;
                self.keep_row_glide(meas, before, d.drained, wall);
            }
        }
    }

    pub(super) fn fold_peaks(&mut self, d: &DataSnapshot) {
        use ac2_proto::FrameData;
        // Held only for the measurements whose peak hold is on (P), from when it went on.
        let held: BTreeSet<MeasId> = self
            .measurements()
            .iter()
            .filter(|m| self.display_of(m).peak.holds())
            .map(|m| m.id)
            .collect();
        self.peaks.retain(|id, _| held.contains(id));
        if held.is_empty() {
            return;
        }
        for tf in d.latest.frames.values() {
            let (meas, level, validity) = match &tf.frame.data {
                FrameData::Spec(f) => (f.meas, &f.level, None),
                FrameData::Rta(f) => (f.meas, &f.level, Some(f.validity.as_slice())),
                _ => continue,
            };
            if !held.contains(&meas) {
                continue;
            }
            let stamp = &tf.frame.stamp;
            let at = stamp.capture_wall_ns.0;
            let e = self
                .peaks
                .entry(meas)
                .or_insert_with(|| (None, at, PeakHold::new(0.0)));
            match FoldedAt::next(e.0, stamp) {
                NextFrame::Seen => continue,
                NextFrame::Newer => {}
                // Another daemon's measurement under this id may be another one, and its
                // clock says nothing of the time since the held frame.
                NextFrame::Afresh => {
                    e.1 = at;
                    e.2 = PeakHold::new(0.0);
                }
            }
            let dt = (at.saturating_sub(e.1)) as f32 / 1e9;
            e.2.update(level, validity, dt);
            e.0 = Some(FoldedAt::of(stamp));
            e.1 = at;
        }
    }

    /// Chooses the tracked peaks again from each newest frame of the measurements whose peak
    /// mode tracks (P), from when it went on, and of those a pane picked (`picked`).
    pub(super) fn fold_peak_tracks(&mut self, d: &DataSnapshot, picked: &BTreeSet<MeasId>) {
        use ac2_proto::FrameData;
        use ac2_scene::spectrum::peaks::{PeakInput, find_peaks};
        let tracked: BTreeSet<MeasId> = self
            .measurements()
            .iter()
            .filter(|m| self.display_of(m).peak.tracks() || picked.contains(&m.id))
            .map(|m| m.id)
            .collect();
        self.peak_tracks.retain(|id, _| tracked.contains(id));
        if tracked.is_empty() {
            return;
        }
        for tf in d.latest.frames.values() {
            let (meas, level, validity, unresolved, narrow) = match &tf.frame.data {
                FrameData::Spec(f) => (
                    f.meas,
                    &f.level,
                    None,
                    f.meta.bands.as_deref().map(|b| &b.unresolved),
                    true,
                ),
                FrameData::Rta(f) => (f.meas, &f.level, Some(f.validity.as_slice()), None, false),
                _ => continue,
            };
            if !tracked.contains(&meas) {
                continue;
            }
            let Some(grid) = tf.frame.stamp.grid_id.and_then(|g| d.grids.get(&g)) else {
                continue;
            };
            let e = self.peak_tracks.entry(meas).or_default();
            match FoldedAt::next(e.0, &tf.frame.stamp) {
                NextFrame::Seen => continue,
                NextFrame::Newer => {}
                // Another daemon's measurement under this id may be another one.
                NextFrame::Afresh => e.1.reset(),
            }
            let cols = ac2_scene::grid::columns(grid);
            e.1.update(&find_peaks(&PeakInput {
                freqs: &cols.freqs,
                level,
                validity,
                unresolved,
                narrow,
            }));
            e.0 = Some(FoldedAt::of(&tf.frame.stamp));
        }
    }
}

impl RigState {
    /// The link reached the daemon (`conn` is [`ConnState::Connected`]): the rig takes its
    /// id and name, and drops what the last connection held.
    pub(super) fn connected(&mut self, conn: ConnState) {
        if let ConnState::Connected { rig_id, name, .. } = &conn {
            self.id = *rig_id;
            self.name.clone_from(name);
        }
        self.conn = conn;
        // A new connection holds no lease.
        self.stimulus.phase = StimPhase::Idle;
        self.stimulus.arm_after_stop = false;
        // Nor has it rebuilt any history: the log may have moved on meanwhile.
        self.leq.logs.clear();
        self.forget_layout_calls();
        // The devices listed were the last daemon's: another host has other ports,
        // and opening with them would name channels this daemon may not have.
        self.devices = None;
        self.devices_for = None;
    }

    /// The link lost the daemon: nothing it mirrored or was asked holds any more.
    pub(super) fn failed(&mut self, target: String, error: String) {
        self.conn = ConnState::Failed { target, error };
        self.forget_layout_calls();
        self.pending.select = None;
        self.pending.stack = None;
        self.mirror = None;
        self.data = None;
        self.stimulus.phase = StimPhase::Idle;
        self.stimulus.arm_after_stop = false;
    }

    /// Drops what is kept by an id the mirror no longer lists: live edits, folds, fetched
    /// traces, the selected trace. `reborn`: another daemon numbers afresh, so no fold
    /// carries over; `keep`: a measurement made here, not listed yet.
    pub(super) fn keep_listed(&mut self, reborn: bool, keep: Option<MeasId>) {
        let ids: Vec<MeasId> = self.measurements().iter().map(|m| m.id).collect();
        self.edits.retain(|k, _| ids.contains(k));
        // A fold belongs to the measurement folded: one listed later under its id
        // (a session loaded, a daemon numbering afresh) starts unfolded.
        self.collapsed.retain(|g| match g {
            TraceOwner::Meas { meas } => !reborn && (ids.contains(meas) || Some(*meas) == keep),
            TraceOwner::Imported => true,
        });
        let metas: BTreeMap<TraceId, TraceMeta> = self
            .daemon()
            .map(|st| st.traces.iter().map(|t| (t.id, t.clone())).collect())
            .unwrap_or_default();
        // Fetched data keeps its columns; its metadata follows the mirror (edits,
        // visibility and slots change by event). The smoothing stays the one the
        // columns were served with until the re-smoothed data arrives.
        self.traces.retain(|k, _| metas.contains_key(k));
        self.trace_peaks.retain(|k, _| metas.contains_key(k));
        for (id, (data, _)) in &mut self.traces {
            if let Some(m) = metas.get(id) {
                follow_meta(data, m);
            }
        }
        if self.selected_trace.is_some_and(|t| !metas.contains_key(&t)) {
            self.selected_trace = None;
        }
    }

    /// A stored trace's data fetched: kept, with the metadata the mirror holds for it.
    pub(super) fn keep_trace(&mut self, mut t: Arc<TraceData>, g: Arc<GridDef>) {
        if let Some(m) = self
            .daemon()
            .and_then(|s| s.traces.iter().find(|x| x.id == t.meta.id))
            .cloned()
        {
            follow_meta(&mut t, &m);
        }
        // New data, new peaks: found again once a pane's pick asks.
        self.trace_peaks.remove(&t.meta.id);
        self.traces.insert(t.meta.id, (t, g));
    }
}
