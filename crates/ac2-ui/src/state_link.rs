//! Events from the daemon link: connection, mirror and data updates, stimulus events, found delays, spectrograph and peak folding.

use super::*;

impl AppState {
    pub(super) fn conn_event(&mut self, e: ConnEvent, out: &mut Vec<Request>) {
        match e {
            ConnEvent::Connecting { target } => {
                if !matches!(self.conn, ConnState::Failed { .. }) {
                    self.conn = ConnState::Connecting { target };
                }
            }
            ConnEvent::Connected {
                target,
                server,
                client_id,
            } => {
                self.conn = ConnState::Connected {
                    target,
                    server,
                    client_id,
                };
                // A new connection holds no lease.
                self.stimulus.phase = StimPhase::Idle;
                self.stimulus.arm_after_stop = false;
                // Nor has it rebuilt any history: the log may have moved on meanwhile.
                self.leq_logs.clear();
                self.forget_layout_calls();
            }
            ConnEvent::Failed { target, error, .. } => {
                self.conn = ConnState::Failed { target, error };
                // Whatever daemon answers next, its ids are its own: what is shown waits
                // for its name.
                let names = self.meas_names();
                self.park_gone_meas(&names, |_| true);
                self.forget_layout_calls();
                self.pending_select = None;
                self.pending_stack = None;
                self.mirror = None;
                self.data = None;
                self.stimulus.phase = StimPhase::Idle;
                self.stimulus.arm_after_stop = false;
            }
            ConnEvent::Mirror(v) => {
                let names = self.meas_names();
                let incarnation = self.mirror.as_ref().and_then(|m| m.incarnation);
                // Another daemon (restarted, swapped in) numbers its measurements afresh: an
                // id it has may be another measurement than the one shown.
                let reborn = incarnation.is_some() && v.incarnation != incarnation;
                let (was_stopped, was_firing) = self.daemon().map_or((true, false), |s| {
                    (s.session.stopped.is_some(), s.generator.firing)
                });
                self.mirror = Some(v);
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
                self.meas_seen.extend(seen);
                // The daemon disarmed the generator when the audio stopped; an arm queued
                // behind a stop must not follow it into a session that is reopening.
                if self.stimulus.arm_after_stop
                    && self.daemon().is_some_and(|s| s.session.stopped.is_some())
                {
                    self.stimulus.arm_after_stop = false;
                    self.warn("not armed: the audio stopped; the daemon is reopening the session");
                }
                let ids: Vec<MeasId> = self.measurements().iter().map(|m| m.id).collect();
                if let Some(p) = self.pending_select
                    && ids.contains(&p)
                {
                    self.selected = Some(p);
                    self.pending_select = None;
                }
                if let Some(p) = self.pending_stack
                    && ids.contains(&p)
                {
                    self.pending_stack = None;
                    self.stack_created(p);
                }
                let waiting = self.pending_select.is_some() && self.pending_select == self.selected;
                // A measurement this app made, selected before the daemon lists it, stays.
                let keep = if waiting { self.selected } else { None };
                self.park_gone_meas(&names, |id| {
                    Some(id) != keep && (reborn || !ids.contains(&id))
                });
                if let Some(id) = self.lost_selection.as_ref().and_then(|n| {
                    self.measurements()
                        .iter()
                        .find(|m| m.config.name == *n)
                        .map(|m| m.id)
                }) {
                    self.selected = Some(id);
                    self.standin = None;
                    self.lost_selection = None;
                    if std::mem::take(&mut self.lost_own_panes) {
                        self.own_panes = Some(id);
                    }
                }
                if !waiting && self.selected.is_none() {
                    self.selected = self
                        .measurements()
                        .iter()
                        .find(|m| matches!(m.config.kind, MeasKind::Transfer { .. }))
                        .or(self.measurements().first())
                        .map(|m| m.id);
                    self.standin = self.selected;
                }
                self.edits.retain(|k, _| ids.contains(k));
                let metas: BTreeMap<TraceId, TraceMeta> = self
                    .daemon()
                    .map(|st| st.traces.iter().map(|t| (t.id, t.clone())).collect())
                    .unwrap_or_default();
                // Fetched data keeps its columns; its metadata follows the mirror (edits,
                // visibility and slots change by event). The smoothing stays the one the
                // columns were served with until the re-smoothed data arrives.
                self.traces.retain(|k, _| metas.contains_key(k));
                for (id, (data, _)) in &mut self.traces {
                    if let Some(m) = metas.get(id) {
                        follow_meta(data, m);
                    }
                }
                if self.selected_trace.is_some_and(|t| !metas.contains_key(&t)) {
                    self.selected_trace = None;
                }
                if let Some(TraceKey::Stored(r)) = self.view.tf.phase_reference
                    && !metas.contains_key(&r)
                {
                    self.view.tf.phase_reference = None;
                }
                self.follow_spectrum_starts();
                self.follow_output_device();
                self.follow_settings();
                self.follow_sweep(out);
                self.follow_leq_alarms();
                let data = self.data.clone();
                self.follow_leq_logs(data.as_deref(), out);
                if self.open_session_when_empty && self.connected() && self.daemon().is_some() {
                    self.open_session_when_empty = false;
                    if self.open_session().is_none() && self.overlay == Overlay::None {
                        self.open_audio_setup(out);
                    }
                }
            }
            ConnEvent::Devices(r) => {
                if let Ok(d) = &r {
                    self.devices = Some(d.clone());
                }
                let names = self.session_input_names();
                match (&mut self.overlay, r) {
                    (Overlay::Settings(s), Ok(d)) => {
                        s.session.set_backends(d, &self.prefs);
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
                    self.preview_sent_s = f64::NEG_INFINITY;
                } else if let Err(e) = r {
                    self.fault(format!("detect loopback: {e}"));
                }
            }
            ConnEvent::SessionOpened { transfers } => {
                let none_yet = self.daemon().is_some_and(|s| s.measurements.is_empty());
                if none_yet && !transfers.is_empty() && self.overlay == Overlay::None {
                    self.overlay = Overlay::Offer(Box::new(Offer { transfers }));
                }
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
            ConnEvent::MeasCreated(m) => {
                // Before it is selected: focusing a sweep pane selects what that pane shows.
                if self.sweep_pane_on_create.as_ref() == Some(&m.config.name) {
                    self.sweep_pane_on_create = None;
                    self.open_created_sweep(m.id);
                }
                self.selected = Some(m.id);
                self.pending_select = Some(m.id);
                // Made here, by the operator: as if its row were chosen, the panes on screen
                // are its own from now (it has none yet to bring).
                self.own_panes = Some(m.id);
                if self.stack_on_create.as_ref() == Some(&m.config.name) {
                    self.stack_on_create = None;
                    self.pending_stack = Some(m.id);
                }
            }
            ConnEvent::Data(d) => {
                if self.view.spectrum.peak_hold {
                    self.fold_peaks(&d);
                }
                if self.spectrograph_shown() {
                    self.fold_spectrographs(&d);
                }
                // A new log clears the history before its first frame goes in.
                self.follow_leq_logs(Some(&d), out);
                self.fold_leq(&d);
                self.fold_spl(&d);
                self.keep_bar_from(&d);
                self.keep_spec_glide(&d);
                self.data = Some(d);
                self.fit_started_spectra();
            }
            ConnEvent::Trace(mut t, g) => {
                if let Some(m) = self
                    .daemon()
                    .and_then(|s| s.traces.iter().find(|x| x.id == t.meta.id))
                    .cloned()
                {
                    follow_meta(&mut t, &m);
                }
                self.traces.insert(t.meta.id, (t, g));
                self.fit_new_sweep();
            }
            ConnEvent::Reply { what, result } if self.layout_reply(&what, &result, out) => {}
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
                if !self.layout_reply(&what, &Err(error), out) {
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
                self.stimulus.phase = StimPhase::Armed;
                if self.armed_with.take() != self.stimulus.settings() {
                    self.resend_stimulus(out);
                }
                let d = self.stimulus.describe();
                if self.sweep.plan.is_some() {
                    self.toast(format!("armed: {d} · Enter plays the sweep · Esc stops"));
                } else {
                    self.toast(format!("armed: {d} · Enter fires · Esc stops"));
                }
            }
            StimEvent::SweepStarted(run) => {
                self.stimulus.phase = StimPhase::Firing;
                self.sweep.run = Some(run.id);
                self.sweep.seen = Some(run.status.clone());
                self.toast(format!(
                    "sweep playing: {} × {} s · Esc stops (and discards it)",
                    run.repeats,
                    format::fixed(run.sweep_duration.0 + run.post_roll.0, 1)
                ));
            }
            StimEvent::Set { firing } => {
                if self.stimulus.phase != StimPhase::Stopping {
                    self.stimulus.phase = if firing {
                        StimPhase::Firing
                    } else {
                        StimPhase::Armed
                    };
                }
            }
            StimEvent::Stopped => {
                self.stimulus.phase = StimPhase::Idle;
                let announced = std::mem::take(&mut self.stimulus.stop_announced);
                if std::mem::take(&mut self.stimulus.arm_after_stop) && !self.sweep.arm_after_stop {
                    self.arm_after_stopped(out);
                }
                let releasing = std::mem::take(&mut self.sweep.releasing);
                let pending = std::mem::take(&mut self.sweep.arm_after_stop);
                match self.sweep.plan.take() {
                    Some(plan) if pending => self.arm_sweep(plan, false, out),
                    plan => {
                        self.sweep.plan = plan;
                        self.end_sweep_mode();
                        if !releasing && !announced {
                            self.toast("stimulus stopped");
                        }
                    }
                }
            }
            StimEvent::Lost(msg) => {
                self.stimulus.phase = StimPhase::Idle;
                self.stimulus.arm_after_stop = false;
                self.stimulus.stop_announced = false;
                self.sweep.releasing = false;
                self.sweep.arm_after_stop = false;
                self.end_sweep_mode();
                self.fault(format!("stimulus lease lost: {msg}"));
            }
            StimEvent::Failed(msg) => {
                self.sweep.releasing = false;
                self.stimulus.arm_after_stop = false;
                self.stimulus.stop_announced = false;
                if std::mem::take(&mut self.sweep.arm_after_stop) {
                    self.end_sweep_mode();
                }
                self.stimulus.phase = match self.stimulus.phase {
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
    /// its next frame is a gap in the picture.
    pub(super) fn fold_spectrographs(&mut self, d: &DataSnapshot) {
        use ac2_proto::FrameData;
        use ac2_scene::spectrograph::{SpectrographFrame, SpectrographHistory};
        let span = self.view.spectrum.spectrograph.span_s;
        let running: BTreeMap<MeasId, bool> = self
            .measurements()
            .iter()
            .map(|m| (m.id, m.running))
            .collect();
        self.spectrographs.retain(|id, _| running.contains_key(id));
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
            let Some(&live) = running.get(&meas) else {
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
            h.push(&SpectrographFrame {
                seq: tf.frame.stamp.seq,
                at: tf.frame.stamp.capture_wall_ns,
                grid,
                edges: &cols.edges,
                scale,
                spec_level,
                level,
                validity,
            });
        }
    }

    pub(super) fn fold_peaks(&mut self, d: &DataSnapshot) {
        use ac2_proto::FrameData;
        for tf in d.latest.frames.values() {
            let (meas, level, validity) = match &tf.frame.data {
                FrameData::Spec(f) => (f.meas, &f.level, None),
                FrameData::Rta(f) => (f.meas, &f.level, Some(f.validity.as_slice())),
                _ => continue,
            };
            let seq = tf.frame.stamp.seq;
            let at = tf.frame.stamp.capture_wall_ns.0;
            let e = self
                .peaks
                .entry(meas)
                .or_insert_with(|| (0, at, PeakHold::new(0.0)));
            if e.2.values().is_empty() || seq > e.0 {
                let dt = (at.saturating_sub(e.1)) as f32 / 1e9;
                e.2.update(level, validity, dt);
                e.0 = seq;
                e.1 = at;
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

    /// Measurements `gone` from the daemon (`names`: what they were called): each pane that
    /// showed one waits for one of that name to come back, and so does the selection (a
    /// stand-in may take its place meanwhile). A session reopened, salvaged or a daemon
    /// restarted takes them away for a moment and makes them again under the same names.
    fn park_gone_meas(&mut self, names: &BTreeMap<MeasId, String>, gone: impl Fn(MeasId) -> bool) {
        for (pane, v) in &mut self.layout.views {
            if let Some(id) = v.meas.filter(|id| gone(*id)) {
                v.meas = None;
                if let Some(n) = names.get(&id) {
                    self.pending_pane_meas.insert(*pane, n.clone());
                }
            }
        }
        // Hidden and compared ones wait by name as a pane's measurement does, unless one
        // that stays has that name: it is another measurement, and the choice was not its.
        let staying: BTreeSet<String> = self
            .measurements()
            .iter()
            .filter(|m| !gone(m.id))
            .map(|m| m.config.name.clone())
            .collect();
        for (ids, pending) in [
            (&mut self.hidden_meas, &mut self.pending_hidden),
            (&mut self.compared_meas, &mut self.pending_compared),
        ] {
            ids.retain(|id| {
                if !gone(*id) {
                    return true;
                }
                if let Some(n) = names.get(id).filter(|n| !staying.contains(*n)) {
                    pending.insert(n.clone());
                }
                false
            });
        }
        if let Some(id) = self.selected.filter(|id| gone(*id)) {
            // A stand-in going leaves the operator's choice waiting.
            if self.standin != Some(id)
                && let Some(n) = names.get(&id)
            {
                self.lost_selection = Some(n.clone());
                self.lost_own_panes = self.own_panes == Some(id);
            }
            self.selected = None;
            self.standin = None;
        }
        // Its panes stay on screen, nobody's own until a row is chosen.
        if self.own_panes.is_some_and(&gone) {
            self.own_panes = None;
        }
    }

    /// The selection moved off the stand-in (the operator chose, this app made something),
    /// or the operator chose the stand-in's row: the selection is theirs again.
    pub(super) fn follow_standin(&mut self) {
        if self.own_panes_due || self.selected.is_some_and(|s| Some(s) != self.standin) {
            self.standin = None;
            self.lost_selection = None;
            self.lost_own_panes = false;
        }
    }

    /// The selection is one this app made by itself.
    pub(super) fn selection_is_standin(&self) -> bool {
        self.selected.is_some() && self.selected == self.standin
    }
}
