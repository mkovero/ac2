//! The stimulus: level, generator and sweep arm / fire / stop, and the sweep traces it leaves.

use super::*;

impl AppState {
    pub(super) fn set_level_text(
        &mut self,
        text: &str,
        out: &mut Vec<Request>,
    ) -> Result<(), String> {
        let v = parse_number(text, &["dbfs", "db"])?;
        if v > 0.0 {
            return Err("level must be ≤ 0 dBFS".into());
        }
        if let Some(c) = self.ceiling()
            && v > c.0
        {
            return Err(format!("above the daemon's ceiling {}", dbfs(c.0)));
        }
        self.stimulus.level = Some(Dbfs(v));
        if self.stimulus.phase == StimPhase::Idle {
            self.toast(format!("level {} · Space arms", dbfs(v)));
        }
        self.resend_stimulus(out);
        Ok(())
    }

    /// Sends the current settings when armed or firing (level / output change).
    pub(super) fn resend_stimulus(&mut self, out: &mut Vec<Request>) {
        let firing = match self.stimulus.phase {
            StimPhase::Armed => false,
            StimPhase::Firing | StimPhase::FireRequested => true,
            _ => return,
        };
        if let Some(settings) = self.stimulus.settings() {
            out.push(Request::StimSet(GeneratorDesired {
                settings,
                armed: true,
                firing,
            }));
        }
    }

    pub(super) fn step_level(&mut self, db: f64, out: &mut Vec<Request>) {
        let Some(l) = self.stimulus.level else {
            self.warn("no stimulus level yet: type one (L)");
            return;
        };
        let ceiling = self.ceiling().map_or(0.0, |c| c.0);
        let v = (l.0 + db).clamp(LEVEL_FLOOR, ceiling.min(0.0));
        // Steps land on whole decibels so ↑ then ↓ returns exactly.
        let v = (v * 10.0).round() / 10.0;
        if v == l.0 {
            if db > 0.0 {
                self.toast(format!("at the ceiling {}", dbfs(ceiling)));
            }
            return;
        }
        self.stimulus.level = Some(Dbfs(v));
        self.resend_stimulus(out);
    }

    /// The sweep view (the sweep pane focused, maximised or not): there the stimulus keys
    /// re-sweep; on every other view they drive the generator for live measuring.
    pub fn sweep_view(&self) -> bool {
        self.layout.focus == PaneKind::Distortion
    }

    /// What the stimulus keys start next, for the top bar: armed, what Enter fires; idle,
    /// what Space arms on the focused view. `None` while a request is in flight or playing.
    pub fn stimulus_next(&self) -> Option<(NextKey, NextStimulus)> {
        let level = self.stimulus.level;
        match self.stimulus.phase {
            StimPhase::Armed => Some((
                NextKey::Enter,
                match &self.sweep.plan {
                    Some(p) => NextStimulus::Sweep {
                        meas: p.name.clone(),
                        duration_s: p.config.sweep.duration.0,
                        level,
                    },
                    None => self.generator_stimulus(),
                },
            )),
            StimPhase::Idle if self.sweep_view() => Some((
                NextKey::Space,
                self.sweep_meas()
                    .and_then(|m| match &m.config.kind {
                        MeasKind::Sweep { config } => Some(NextStimulus::Sweep {
                            meas: m.config.name.clone(),
                            duration_s: config.sweep.duration.0,
                            level: Some(config.level),
                        }),
                        _ => None,
                    })
                    .unwrap_or(NextStimulus::SweepDialog),
            )),
            StimPhase::Idle => Some((NextKey::Space, self.generator_stimulus())),
            _ => None,
        }
    }

    /// What this app's stimulus does for a reference that carries nothing (NO REFERENCE):
    /// off or armed, the banner names the keys that start it on the focused view; playing,
    /// in flight or another client's, the patch is the suspect.
    pub fn drive(&self) -> Drive {
        let key = |c: CommandId| {
            RESERVED
                .iter()
                .find(|(_, id)| *id == c)
                .map_or_else(String::new, |(k, _)| k.label())
        };
        let g = self.daemon().map(|s| &s.generator);
        if self.sweep.run.is_some() || g.is_some_and(|g| g.firing) {
            return Drive::Playing;
        }
        let mine = g
            .and_then(|g| g.owner.as_ref())
            .is_some_and(|o| Some(o) == self.my_client_id());
        let armed = match self.stimulus.phase {
            StimPhase::Armed => true,
            StimPhase::Stopping => false,
            StimPhase::Idle => match g {
                Some(g) if g.armed && !mine => return Drive::Playing,
                Some(g) => g.armed,
                None => false,
            },
            StimPhase::Arming | StimPhase::FireRequested | StimPhase::Firing => {
                return Drive::Playing;
            }
        };
        // Armed with a sweep, Enter plays the sweep; Space on a live view re-arms the
        // generator, so there the off reminder is the one that holds.
        if armed && self.sweep.plan.is_none() {
            Drive::Armed {
                fire: key(CommandId::StimulusFire),
            }
        } else if self.sweep_view() {
            Drive::SweepView
        } else {
            Drive::Idle {
                arm: key(CommandId::StimulusArm),
                fire: key(CommandId::StimulusFire),
            }
        }
    }

    /// The generator as the live views play it: never a sweep signal.
    pub(super) fn generator_stimulus(&self) -> NextStimulus {
        NextStimulus::Generator {
            signal: match self.stimulus.signal {
                Signal::Ess { .. } => Signal::Pink,
                s => s,
            },
            level: self.stimulus.level,
            outputs: self.stimulus.outputs.clone(),
            labels: self.daemon().map(|s| s.outputs.clone()).unwrap_or_default(),
        }
    }

    /// Space: arms what the focused view plays (the sweep view a re-sweep, the others the
    /// generator). Armed with the other kind and still silent, Space re-sets the armed
    /// stimulus to this view's, so what Enter fires is always what a Space on this view
    /// chose; a playing stimulus is never changed by it.
    pub(super) fn space(&mut self, force: bool, keymap: &Keymap, out: &mut Vec<Request>) {
        if self.sweep_view() {
            self.space_sweep(force, keymap, out);
        } else {
            self.space_generator(force, keymap, out);
        }
    }

    pub(super) fn space_sweep(&mut self, force: bool, keymap: &Keymap, out: &mut Vec<Request>) {
        // Already set up with a sweep (arming, armed, playing, or queued behind a stop).
        if self.sweep.run.is_some()
            || (self.sweep.plan.is_some()
                && (self.stimulus.phase != StimPhase::Idle || self.sweep.arm_after_stop))
        {
            return;
        }
        let Some(plan) = self.sweep_meas().and_then(|m| match &m.config.kind {
            MeasKind::Sweep { config } => Some(SweepPlan {
                meas: m.id,
                name: m.config.name.clone(),
                config: config.clone(),
            }),
            _ => None,
        }) else {
            self.open_sweep_dialog(keymap, out);
            return;
        };
        if !self.connected() {
            self.warn("not connected");
            return;
        }
        if self.daemon().is_some_and(|s| s.session.open.is_none()) {
            self.warn(format!(
                "no audio session to sweep: {}",
                open_session_hint(keymap)
            ));
            return;
        }
        // The level was typed for the measurement; the ceiling may have come down since.
        if let Some(c) = self.ceiling()
            && plan.config.level.0 > c.0
        {
            self.warn(format!(
                "{}'s level {} is above the daemon's ceiling {}: edit it (palette: edit the \
                 selected measurement)",
                plan.name,
                dbfs(plan.config.level.0),
                dbfs(c.0)
            ));
            return;
        }
        let phase = self.stimulus.phase;
        // A noise queued behind the stop gives way to the sweep this view arms.
        self.stimulus.arm_after_stop = false;
        self.arm_sweep(plan, force, out);
        match phase {
            StimPhase::Stopping => {
                self.toast("stopping… arms the re-sweep once the stop is done (Esc cancels)");
            }
            StimPhase::Armed => self.toast(format!(
                "armed: a run of {} · Enter plays the sweep · Esc stops",
                self.sweep.plan.as_ref().map_or("", |p| p.name.as_str())
            )),
            _ => {}
        }
    }

    pub(super) fn space_generator(&mut self, force: bool, keymap: &Keymap, out: &mut Vec<Request>) {
        if self.sweep.run.is_some() {
            return;
        }
        if self.sweep.plan.is_some() {
            match self.stimulus.phase {
                StimPhase::Armed => {
                    self.end_sweep_mode();
                    self.resend_stimulus(out);
                    self.toast(format!(
                        "armed: {} · Enter fires · Esc stops",
                        self.stimulus.describe()
                    ));
                    return;
                }
                StimPhase::Stopping | StimPhase::Idle => {
                    self.sweep.arm_after_stop = false;
                    self.end_sweep_mode();
                }
                _ => return,
            }
        }
        self.arm(force, keymap, out);
    }

    /// The sweep measurement Space on the sweep view runs: the selected measurement when
    /// it is one, else the owner of the selected (or shown) run, else the pane's.
    pub fn sweep_meas(&self) -> Option<&Measurement> {
        let is = |m: &&Measurement| matches!(m.config.kind, MeasKind::Sweep { .. });
        let owner_of = |t: &TraceMeta| t.edit.owner.meas().and_then(|id| self.meas(id));
        self.selected_meas()
            .filter(is)
            .or_else(|| self.selected_trace_meta().and_then(owner_of).filter(is))
            .or_else(|| {
                self.shown_sweep()
                    .and_then(|(t, _)| owner_of(&t.meta))
                    .filter(is)
            })
            .or_else(|| self.pane_meas(PaneKind::Distortion))
    }

    pub(super) fn arm(&mut self, force: bool, keymap: &Keymap, out: &mut Vec<Request>) {
        if !self.connected() {
            self.warn("not connected");
            return;
        }
        if self.stimulus.level.is_none() {
            self.prompt(PromptKind::StimulusLevel, String::new());
            self.toast("type a level first; nothing is armed without one");
            return;
        }
        if self.daemon().is_some_and(|s| s.session.open.is_none()) {
            self.warn(format!(
                "no audio session to play into: {}",
                open_session_hint(keymap)
            ));
            return;
        }
        if self.stimulus.phase == StimPhase::Stopping && !force {
            if !self.stimulus.arm_after_stop {
                self.stimulus.arm_after_stop = true;
                self.toast("stopping… arms once the stop is done (Esc cancels)");
            }
            return;
        }
        if !matches!(self.stimulus.phase, StimPhase::Idle) && !force {
            return;
        }
        if let Some(settings) = self.stimulus.settings() {
            self.stimulus.phase = StimPhase::Arming;
            self.armed_with = Some(settings.clone());
            out.push(Request::StimArm { settings, force });
        }
    }

    /// The arm Space asked for while a stop was in flight, now that the stop has landed:
    /// the same checks as a fresh Space, against the state the stop left.
    pub(super) fn arm_after_stopped(&mut self, out: &mut Vec<Request>) {
        if !self.connected() || self.stimulus.phase != StimPhase::Idle {
            return;
        }
        if self.daemon().is_some_and(|s| s.session.open.is_none()) {
            self.warn("not armed: no audio session to play into");
            return;
        }
        if self.daemon().is_some_and(|s| s.session.stopped.is_some()) {
            self.warn("not armed: the audio stopped; the daemon is reopening the session");
            return;
        }
        if let Some(settings) = self.stimulus.settings() {
            self.stimulus.phase = StimPhase::Arming;
            self.armed_with = Some(settings.clone());
            out.push(Request::StimArm {
                settings,
                force: false,
            });
        }
    }

    /// The sweep dialog over the open session's inputs and outputs, by name.
    pub(super) fn open_sweep_dialog(&mut self, keymap: &Keymap, out: &mut Vec<Request>) {
        let Some(o) = self.open_session().cloned() else {
            self.warn(format!(
                "no audio session to sweep: {}",
                open_session_hint(keymap)
            ));
            return;
        };
        let inputs = self.session_input_names();
        let outputs = self.session_output_names();
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
        let sweeps = self.daemon().map_or(0, |s| {
            s.measurements
                .iter()
                .filter(|m| matches!(m.config.kind, MeasKind::Sweep { .. }))
                .count()
        });
        let f = Form::sweep(
            Some(&o),
            sweeps,
            &inputs,
            &outputs,
            &mics,
            self.stimulus.level,
        );
        self.overlay = Overlay::Form(Box::new(f));
        if self.devices.is_none() {
            out.push(Request::Devices);
        }
    }

    /// A run of the sweep measurement becomes the stimulus: armed with its settings (or
    /// re-sent when already armed); Enter then plays it.
    pub(super) fn arm_sweep(&mut self, plan: SweepPlan, force: bool, out: &mut Vec<Request>) {
        let r = &plan.config;
        self.stimulus.signal = Signal::Ess { sweep: r.sweep };
        self.stimulus.level = Some(r.level);
        self.stimulus.outputs = r.outputs.clone();
        self.sweep.plan = Some(plan);
        match self.stimulus.phase {
            StimPhase::Idle => {
                if let Some(settings) = self.stimulus.settings() {
                    self.stimulus.phase = StimPhase::Arming;
                    self.armed_with = Some(settings.clone());
                    out.push(Request::StimArm { settings, force });
                }
            }
            StimPhase::Firing | StimPhase::FireRequested => {
                self.warn(format!(
                    "the stimulus is firing: {STOP_ANYWHERE} stops it, then arm the sweep"
                ));
                self.sweep.plan = None;
            }
            // The stop before it is still on its way (stop, then the lease given back): the
            // sweep arms once it has landed, not into the lease that stop is releasing.
            StimPhase::Stopping => self.sweep.arm_after_stop = true,
            _ => self.resend_stimulus(out),
        }
    }

    /// Enter while armed with a sweep: `sweep.run` of its measurement. A level or outputs
    /// changed while armed become the measurement's settings first: the run plays what the
    /// top bar named, and the next run the same.
    pub(super) fn fire_sweep(&mut self, out: &mut Vec<Request>) {
        let Some(plan) = &self.sweep.plan else {
            return;
        };
        let mut c = plan.config.clone();
        if let Some(l) = self.stimulus.level {
            c.level = l;
        }
        c.outputs = self.stimulus.outputs.clone();
        if let Signal::Ess { sweep } = self.stimulus.signal {
            c.sweep = sweep;
        }
        let update = (c != plan.config).then(|| {
            Box::new(ac2_proto::model::MeasConfig {
                name: plan.name.clone(),
                kind: MeasKind::Sweep { config: c },
            })
        });
        let (meas, label) = (plan.meas, plan.name.clone());
        self.stimulus.phase = StimPhase::FireRequested;
        out.push(Request::Sweep {
            meas,
            label,
            update,
        });
    }

    /// A stop ends the sweep set-up: the next arm is noise again.
    pub(super) fn end_sweep_mode(&mut self) {
        if self.sweep.plan.take().is_some() {
            self.stimulus.signal = Signal::Pink;
        }
    }

    /// Acts on the run this client started as the mirror reports it: stored → the pane
    /// shows it; failed → says why. The daemon disarms once the sweep has played, so the
    /// stimulus is off from then on; when the run ends the lease is given back and sweep
    /// mode ends — the next sweep is armed from the dialog again.
    pub(super) fn follow_sweep(&mut self, out: &mut Vec<Request>) {
        let Some(id) = self.sweep.run else {
            return;
        };
        let Some(r) = self.daemon().and_then(|s| s.sweep.clone()) else {
            return;
        };
        if r.id != id || self.sweep.seen.as_ref() == Some(&r.status) {
            return;
        }
        self.sweep.seen = Some(r.status.clone());
        if !matches!(r.status, SweepStatus::Playing { .. })
            && matches!(
                self.stimulus.phase,
                StimPhase::Firing | StimPhase::FireRequested
            )
        {
            self.stimulus.phase = StimPhase::Idle;
        }
        match r.status {
            SweepStatus::Playing { .. } => {}
            SweepStatus::Analysing => {
                self.toast("sweep recorded: analysing");
            }
            SweepStatus::Done { trace } => {
                self.sweep.run = None;
                // The new result is selected under its measurement: listed highlighted,
                // named in the captions, and what the trace keys change.
                self.selected = Some(r.meas);
                self.collapsed
                    .remove(&ac2_proto::model::TraceOwner::Meas { meas: r.meas });
                self.selected_trace = Some(trace);
                self.sweep.shown = Some(trace);
                self.sweep.fit = Some(trace);
                self.fit_new_sweep();
                self.release_after_sweep(out);
                self.layout.shown[PaneKind::Distortion.index()] = true;
                self.layout.focus = PaneKind::Distortion;
                let of = self
                    .meas(r.meas)
                    .map_or_else(String::new, |m| format!(" of {}", m.config.name));
                self.toast(format!(
                    "{}{of} stored: U dB / %, Shift+I impulse response, Space runs it again",
                    r.name
                ));
            }
            SweepStatus::Failed { msg, .. } => {
                self.sweep.run = None;
                self.release_after_sweep(out);
                self.fault(format!("sweep failed: {msg}"));
            }
        }
    }

    /// A finished run ends sweep mode and gives the lease back, unless the operator has
    /// armed again meanwhile. Only a lease this client holds: a stop without one is the
    /// universal stop and would silence whoever took the stimulus over.
    pub(super) fn release_after_sweep(&mut self, out: &mut Vec<Request>) {
        if self.stimulus.phase != StimPhase::Idle {
            return;
        }
        self.end_sweep_mode();
        let mine = self
            .daemon()
            .and_then(|s| s.generator.owner.as_ref())
            .is_some_and(|o| Some(o) == self.my_client_id());
        if mine {
            self.stimulus.phase = StimPhase::Stopping;
            self.sweep.releasing = true;
            out.push(Request::StimStop);
        }
    }

    /// Stopping (or deleting) `stopping` stops the stimulus too when it is the last transfer
    /// measurement running and this app holds the lease, armed or playing: the noise is there to excite
    /// transfer functions, and with none left measuring it only makes the room loud. SPL,
    /// spectrum and RTA read whatever plays and need no stimulus of their own, so they keep
    /// nothing playing; a sweep is its own measurement and is never cut by this. The stop is
    /// the one Esc sends (faded out, disarmed, the lease released); another client's stimulus
    /// is never stopped this way. Says whether it stopped.
    pub(super) fn stop_stimulus_with(
        &mut self,
        stopping: &Measurement,
        out: &mut Vec<Request>,
    ) -> bool {
        let transfer = |m: &Measurement| matches!(m.config.kind, MeasKind::Transfer { .. });
        if !transfer(stopping)
            || !stopping.running
            || self.sweep.run.is_some()
            || self.sweep.plan.is_some()
        {
            return false;
        }
        let other_running = self
            .measurements()
            .iter()
            .any(|m| m.id != stopping.id && m.running && transfer(m));
        if other_running || !self.holds_stimulus() {
            return false;
        }
        self.stimulus.arm_after_stop = false;
        self.stimulus.phase = StimPhase::Stopping;
        self.stimulus.stop_announced = true;
        out.push(Request::StimStop);
        true
    }

    /// This app holds the stimulus lease, armed or playing: its own arm landed, or the
    /// mirror names this client as the owner of a live generator.
    pub(super) fn holds_stimulus(&self) -> bool {
        match self.stimulus.phase {
            StimPhase::Armed | StimPhase::FireRequested | StimPhase::Firing => true,
            StimPhase::Stopping => false,
            StimPhase::Idle | StimPhase::Arming => self.daemon().is_some_and(|s| {
                (s.generator.armed || s.generator.firing)
                    && s.generator
                        .owner
                        .as_ref()
                        .is_some_and(|o| Some(o) == self.my_client_id())
            }),
        }
    }

    /// Sweep traces with their data, oldest first.
    pub fn sweep_traces(&self) -> Vec<(&TraceData, &GridDef)> {
        self.traces
            .values()
            .filter(|(t, _)| t.meta.kind == TraceKind::Sweep && t.sweep.is_some())
            .map(|(t, g)| (t.as_ref(), g.as_ref()))
            .collect()
    }

    /// The sweep trace the distortion pane shows: the selected trace when it is a sweep,
    /// else the newest run of the selected sweep measurement, else the sweep selected last,
    /// else the newest.
    pub fn shown_sweep(&self) -> Option<(&TraceData, &GridDef)> {
        let all = self.sweep_traces();
        let find = |id: TraceId| all.iter().find(|(t, _)| t.meta.id == id).copied();
        let newest_of = |m: &Measurement| {
            let owner = ac2_proto::model::TraceOwner::Meas { meas: m.id };
            all.iter()
                .filter(|(t, _)| t.meta.edit.owner == owner)
                .max_by_key(|(t, _)| t.meta.id)
                .copied()
        };
        self.selected_trace
            .and_then(find)
            .or_else(|| {
                self.selected_meas()
                    .filter(|m| matches!(m.config.kind, MeasKind::Sweep { .. }))
                    .and_then(newest_of)
            })
            .or_else(|| self.sweep.shown.and_then(find))
            .or_else(|| all.last().copied())
    }

    /// N / Shift+N on the sweep pane: the next / previous stored sweep, selected (the
    /// transfer pane and the trace keys follow it).
    pub(super) fn cycle_sweep(&mut self, d: i32) {
        let ids: Vec<TraceId> = self.sweep_traces().iter().map(|(t, _)| t.meta.id).collect();
        if ids.is_empty() {
            self.warn("no sweep results yet: Shift+S sets one up");
            return;
        }
        let cur = self.shown_sweep().map(|(t, _)| t.meta.id);
        let i = cur
            .and_then(|c| ids.iter().position(|x| *x == c))
            .unwrap_or(0) as i32;
        let id = ids[(i + d).rem_euclid(ids.len() as i32) as usize];
        self.select_trace(Some(id));
        if let Some((t, _)) = self.shown_sweep() {
            let name = trace_label(&t.meta);
            self.toast(format!("{name} selected"));
        }
    }
}
