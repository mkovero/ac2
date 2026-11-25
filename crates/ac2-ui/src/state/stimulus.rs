//! The stimulus: level, generator and sweep arm / fire / stop, and the sweep traces it leaves.

use super::*;

impl AppState {
    pub(super) fn set_level_text(
        &mut self,
        text: &str,
        out: &mut Vec<Request>,
    ) -> Result<(), String> {
        let v = ac2_scene::unit_text::in_field::<ac2_scene::unit_text::LevelDbfs>(text, "dbfs")?
            .0
            .0;
        if v > 0.0 {
            return Err("level must be ≤ 0 dBFS".into());
        }
        if let Some(c) = self.ceiling()
            && v > c.0
        {
            return Err(ac2_scene::stim_rows::above_max(c));
        }
        self.rig.stimulus.level = Some(Dbfs(v));
        if self.rig.stimulus.phase == StimPhase::Idle {
            self.toast(format!("level {} · Space arms", format::dbfs(v)));
        }
        self.resend_stimulus(out);
        Ok(())
    }

    /// The open session's sample rate, when one is open.
    fn session_rate(&self) -> Option<u32> {
        self.daemon()
            .and_then(|s| s.session.open.as_ref())
            .map(|o| o.sample_rate_hz)
    }

    /// The FFT whose bins a sine goes onto, with the sample rate: the shortest FFT a
    /// running spectrum measurement of this rig reads a sine on, whose bins are bins of
    /// every longer one too. `None` with no spectrum running or no audio I/O open.
    pub fn sine_bins(&self) -> Option<(usize, f64)> {
        let fs = f64::from(self.session_rate()?);
        let all = self.measurements();
        let running = all
            .iter()
            .filter(|m| m.running)
            .filter_map(|m| match &m.config.kind {
                MeasKind::Spectrum { config } => Some(config.resolution),
                _ => None,
            });
        ac2_proto::model::SpecResolution::shared_sine_fft_len(running, fs).map(|n| (n, fs))
    }

    /// The generator's signal as it is or would be sent: the choice, a sine set to be on
    /// a bin moved there. `None` until a signal is chosen.
    pub fn generator_signal(&self) -> Option<Signal> {
        self.rig
            .stimulus
            .choice()
            .map(|c| c.played(self.sine_bins()))
    }

    /// Makes the stimulus' signal the generator's as it resolves now, unless a sweep is
    /// set up (which keeps its own). `true` when that changed it.
    pub(super) fn sync_generator_signal(&mut self) -> bool {
        if self.rig.sweep.plan.is_some() || self.rig.dynamics_run.plan.is_some() {
            return false;
        }
        let Some(signal) = self.generator_signal() else {
            return false;
        };
        let changed = self.rig.stimulus.signal != signal;
        self.rig.stimulus.signal = signal;
        changed
    }

    /// The operator's choice of signal, from the dialog or the settings rows: kept, said
    /// with the frequency really played, and sent at once while armed or playing with the
    /// generator (a sweep set up keeps its own signal; the choice is what the next Space
    /// on a live view arms).
    pub(super) fn choose_signal(&mut self, choice: SignalChoice, out: &mut Vec<Request>) {
        self.rig.stimulus.generator = Some(choice.kind());
        if let SignalChoice::Sine { freq, on_bin } = choice {
            self.rig.stimulus.sine = SineSetting { freq, on_bin };
        }
        let said = choice.chosen_text(self.sine_bins());
        let idle = self.rig.stimulus.phase == StimPhase::Idle;
        match self.rig.stimulus.level {
            None => self.toast(format!(
                "{said} · type a level; nothing is armed without one"
            )),
            Some(_) if idle => self.toast(format!("{said} · Space arms")),
            Some(_) => self.toast(said),
        }
        if self.rig.sweep.plan.is_none() {
            self.resend_stimulus(out);
        }
    }

    /// Sends the current settings when armed or firing (level / output change).
    pub(super) fn resend_stimulus(&mut self, out: &mut Vec<Request>) {
        let firing = match self.rig.stimulus.phase {
            StimPhase::Armed => false,
            StimPhase::Firing | StimPhase::FireRequested => true,
            _ => return,
        };
        self.sync_generator_signal();
        if let Some(settings) = self.rig.stimulus.settings() {
            out.push(Request::StimSet(GeneratorDesired {
                settings,
                armed: true,
                firing,
            }));
        }
    }

    pub(super) fn step_level(&mut self, db: f64, out: &mut Vec<Request>) {
        let Some(l) = self.rig.stimulus.level else {
            let l = self.keys.reach(CommandId::StimulusLevel);
            self.warn(format!("no stimulus level yet: type one ({l})"));
            return;
        };
        let ceiling = self.ceiling().map_or(0.0, |c| c.0);
        let v = (l.0 + db).clamp(LEVEL_FLOOR, ceiling.min(0.0));
        // Steps land on whole decibels so ↑ then ↓ returns exactly.
        let v = (v * 10.0).round() / 10.0;
        if v == l.0 {
            if db > 0.0 {
                self.toast(ac2_scene::stim_rows::at_max(ceiling));
            }
            return;
        }
        self.rig.stimulus.level = Some(Dbfs(v));
        self.resend_stimulus(out);
    }

    /// The sweep view (the sweep pane focused, maximised or not, or a sweep walk's pane):
    /// there the stimulus keys re-sweep; on every other view they drive the generator for
    /// live measuring.
    pub fn sweep_view(&self) -> bool {
        self.layout.focus_kind() == PaneKind::Distortion || self.focused_sweep_walk().is_some()
    }

    /// What the stimulus keys start next, for the top bar: armed, what Enter fires; idle,
    /// what Space arms on the focused view. `None` whenever neither starts anything: a
    /// request in flight, playing, a stimulus under this name from elsewhere, another
    /// client's.
    pub fn stimulus_next(&self) -> Option<(NextKey, NextStimulus)> {
        match self.own_stim() {
            OwnStim::Armed => {}
            OwnStim::Idle if !self.other_holds() => {}
            _ => return None,
        }
        let level = self.rig.stimulus.level;
        match self.rig.stimulus.phase {
            StimPhase::Armed => Some((
                NextKey::Enter,
                match &self.rig.sweep.plan {
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
            // Without a level the hint names the key that types one, whatever the signal;
            // with one, an unchosen signal is what Space asks for next.
            StimPhase::Idle if level.is_some() && self.rig.stimulus.generator.is_none() => {
                Some((NextKey::Space, NextStimulus::SignalDialog))
            }
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
        // The keys the banner names are the ones Space and Enter act on now: only this
        // app's own stimulus, off or armed, has a key that starts it.
        let armed = match self.own_stim() {
            OwnStim::Armed => true,
            OwnStim::Idle
                if self
                    .daemon()
                    .is_some_and(|s| s.generator.armed || s.generator.firing) =>
            {
                return Drive::Playing;
            }
            OwnStim::Idle => false,
            OwnStim::Arming
            | OwnStim::Playing
            | OwnStim::Run
            | OwnStim::Stopping
            | OwnStim::Elsewhere => return Drive::Playing,
        };
        // Armed with a sweep, Enter plays the sweep; Space on a live view re-arms the
        // generator, so there the off reminder is the one that holds.
        if armed && self.rig.sweep.plan.is_none() {
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

    /// The generator as the live views play it: the operator's choice, which is never a
    /// sweep signal (armed before a choice was made, by a sweep: what is armed).
    pub(super) fn generator_stimulus(&self) -> NextStimulus {
        NextStimulus::Generator {
            signal: self.generator_signal().unwrap_or(self.rig.stimulus.signal),
            level: self.rig.stimulus.level,
            outputs: self.rig.stimulus.outputs.clone(),
            labels: self.daemon().map(|s| s.outputs.clone()).unwrap_or_default(),
        }
    }

    /// Arms what the focused view plays (the sweep view a re-sweep, the others the
    /// generator): Space with nothing of this app's armed, or the take-over (`force`) of
    /// another client's stimulus. A stimulus taken over armed with the other kind and still
    /// silent is re-set to this view's, so what Enter fires is what this view plays; a
    /// playing stimulus is never changed by it.
    pub(super) fn space(&mut self, force: bool, keymap: &Keymap, out: &mut Vec<Request>) {
        if self.layout.focus_kind() == PaneKind::Dynamics {
            self.space_dynamics(force, keymap, out);
        } else if self.sweep_view() {
            self.space_sweep(force, keymap, out);
        } else {
            self.space_generator(force, keymap, out);
        }
    }

    pub(super) fn space_sweep(&mut self, force: bool, keymap: &Keymap, out: &mut Vec<Request>) {
        // Already set up with a sweep (arming, armed, playing, or queued behind a stop).
        if self.rig.sweep.run.is_some()
            || (self.rig.sweep.plan.is_some()
                && (self.rig.stimulus.phase != StimPhase::Idle || self.rig.sweep.arm_after_stop))
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
                "no audio I/O to sweep: {}",
                open_session_hint(keymap)
            ));
            return;
        }
        // The level was typed for the measurement; the ceiling may have come down since.
        if let Some(c) = self.ceiling()
            && plan.config.level.0 > c.0
        {
            self.warn(ac2_scene::stim_rows::sweep_above_max(
                &plan.name,
                plan.config.level,
                c,
            ));
            return;
        }
        let phase = self.rig.stimulus.phase;
        self.arm_sweep(plan, force, out);
        match phase {
            StimPhase::Stopping => {
                self.toast(ac2_scene::stim_item::RESWEEP_AFTER_STOP);
            }
            StimPhase::Armed => self.toast(ac2_scene::stim_item::armed_sweep_toast(
                &ac2_scene::stim_item::sweep_run_of(
                    self.rig.sweep.plan.as_ref().map_or("", |p| p.name.as_str()),
                ),
            )),
            _ => {}
        }
    }

    pub(super) fn space_generator(&mut self, force: bool, keymap: &Keymap, out: &mut Vec<Request>) {
        if self.rig.sweep.run.is_some() {
            return;
        }
        // Nothing of the generator is armed before the operator said what it plays, as
        // nothing is without a level: the dialog opens on pink, so Enter alone takes it.
        if self.rig.stimulus.generator.is_none() {
            if !self.connected() {
                self.warn("not connected");
                return;
            }
            self.overlay = Overlay::Form(Box::new(crate::signal_dialog::form(
                self.rig.stimulus.sine,
                self.sine_bins(),
                self.session_rate(),
            )));
            self.toast(ac2_scene::stim_item::CHOOSE_SIGNAL_FIRST);
            return;
        }
        if self.rig.sweep.plan.is_some() {
            match self.rig.stimulus.phase {
                StimPhase::Armed => {
                    self.end_sweep_mode();
                    self.resend_stimulus(out);
                    self.toast(ac2_scene::stim_item::armed_toast(
                        &self.rig.stimulus.describe(),
                    ));
                    return;
                }
                StimPhase::Stopping | StimPhase::Idle => {
                    self.rig.sweep.arm_after_stop = false;
                    self.end_sweep_mode();
                }
                _ => return,
            }
        }
        self.arm(force, keymap, out);
    }

    /// The sweep measurement Space on the sweep view runs: the focused sweep walk's or sweep
    /// pane's (a pane key acts on what its pane shows), else the selected measurement when it is one, else
    /// the owner of the selected (or shown) run, else the sweep pane's.
    pub fn sweep_meas(&self) -> Option<&Measurement> {
        let is = |m: &&Measurement| matches!(m.config.kind, MeasKind::Sweep { .. });
        let owner_of = |t: &TraceMeta| t.edit.owner.meas().and_then(|id| self.meas(id));
        let f = self.layout.focus;
        let walk = self
            .focused_sweep_walk()
            .and_then(|w| w.mics.first())
            .and_then(|m| self.meas(m.meas));
        walk.or_else(|| {
            (self.layout.kind(f) == PaneKind::Distortion)
                .then(|| self.pane_meas(f))
                .flatten()
        })
        .filter(is)
        .or_else(|| self.selected_meas().filter(is))
        .or_else(|| self.selected_trace_meta().and_then(owner_of).filter(is))
        .or_else(|| {
            self.kind_sweep()
                .and_then(|(t, _)| owner_of(&t.meta))
                .filter(is)
        })
        .or_else(|| self.kind_meas(PaneKind::Distortion))
    }

    pub(super) fn arm(&mut self, force: bool, keymap: &Keymap, out: &mut Vec<Request>) {
        if !self.connected() {
            self.warn("not connected");
            return;
        }
        if self.rig.stimulus.level.is_none() {
            self.prompt(PromptKind::StimulusLevel, String::new());
            self.toast(ac2_scene::stim_item::LEVEL_FIRST);
            return;
        }
        if self.daemon().is_some_and(|s| s.session.open.is_none()) {
            self.warn(format!(
                "no audio I/O to play into: {}",
                open_session_hint(keymap)
            ));
            return;
        }
        if !matches!(self.rig.stimulus.phase, StimPhase::Idle) && !force {
            return;
        }
        self.sync_generator_signal();
        if let Some(settings) = self.rig.stimulus.settings() {
            self.rig.stimulus.phase = StimPhase::Arming;
            self.rig.due.armed_with = Some(settings.clone());
            out.push(Request::StimArm { settings, force });
        }
    }

    /// The sweep dialog over the open session's inputs and outputs, by name.
    pub(super) fn open_sweep_dialog(&mut self, keymap: &Keymap, out: &mut Vec<Request>) {
        let Some(o) = self.open_session().cloned() else {
            self.warn(format!(
                "no audio I/O to sweep: {}",
                open_session_hint(keymap)
            ));
            return;
        };
        if let Some(why) = self.two_input_refusal(true, keymap) {
            self.warn(why);
            return;
        }
        let inputs = self.session_input_names();
        let outputs = self.session_output_names();
        let mics: Vec<u16> = self
            .daemon()
            .map(|s| {
                s.inputs
                    .iter()
                    .filter(|i| i.mic().is_some())
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
            self.rig.stimulus.level,
        );
        self.overlay = Overlay::Form(Box::new(f));
        if self.rig.devices.is_none() {
            out.push(Request::Devices);
        }
    }

    /// A run of the sweep measurement becomes the stimulus: armed with its settings (or
    /// re-sent when already armed); Enter then plays it.
    pub(super) fn arm_sweep(&mut self, plan: SweepPlan, force: bool, out: &mut Vec<Request>) {
        let r = &plan.config;
        self.rig.stimulus.signal = Signal::Ess { sweep: r.sweep };
        self.rig.stimulus.level = Some(r.level);
        self.rig.stimulus.outputs = r.outputs.clone();
        self.rig.sweep.plan = Some(plan);
        match self.rig.stimulus.phase {
            StimPhase::Idle => {
                if let Some(settings) = self.rig.stimulus.settings() {
                    self.rig.stimulus.phase = StimPhase::Arming;
                    self.rig.due.armed_with = Some(settings.clone());
                    out.push(Request::StimArm { settings, force });
                }
            }
            StimPhase::Firing | StimPhase::FireRequested => {
                self.warn(format!(
                    "the stimulus is firing: {STOP_ANYWHERE} stops it, then arm the sweep"
                ));
                self.rig.sweep.plan = None;
            }
            // The stop before it is still on its way (stop, then the lease given back): the
            // sweep arms once it has landed, not into the lease that stop is releasing.
            StimPhase::Stopping => self.rig.sweep.arm_after_stop = true,
            _ => self.resend_stimulus(out),
        }
    }

    /// Enter while armed with a sweep: `sweep.run` of its measurement. A level or outputs
    /// changed while armed become the measurement's settings first: the run plays what the
    /// top bar named, and the next run the same.
    pub(super) fn fire_sweep(&mut self, out: &mut Vec<Request>) {
        let Some(plan) = &self.rig.sweep.plan else {
            return;
        };
        let mut c = plan.config.clone();
        if let Some(l) = self.rig.stimulus.level {
            c.level = l;
        }
        c.outputs = self.rig.stimulus.outputs.clone();
        if let Signal::Ess { sweep } = self.rig.stimulus.signal {
            c.sweep = sweep;
        }
        let base = self
            .meas(plan.meas)
            .map_or(ac2_proto::units::Rev(0), |m| m.config_rev);
        let update = (c != plan.config).then(|| {
            (
                base,
                Box::new(ac2_proto::model::MeasConfig {
                    name: plan.name.clone(),
                    kind: MeasKind::Sweep { config: c },
                    scope: ac2_proto::model::MeasScope::Session,
                }),
            )
        });
        let (meas, label) = (plan.meas, plan.name.clone());
        self.rig.stimulus.phase = StimPhase::FireRequested;
        out.push(Request::Sweep {
            meas,
            label,
            update,
        });
    }

    /// A stop ends the sweep set-up: the next arm is the generator's chosen signal again.
    pub(super) fn end_sweep_mode(&mut self) {
        let dynamics = self.rig.dynamics_run.plan.take().is_some();
        if self.rig.sweep.plan.take().is_some() || dynamics {
            // Before a choice was made nothing of the generator can be armed, so pink
            // here is only what the idle state holds; the first arm asks.
            self.rig.stimulus.signal = Signal::Pink;
            self.sync_generator_signal();
        }
    }

    /// Acts on the run this client started as the mirror reports it: stored → the pane
    /// shows it; failed → says why. The daemon disarms once the sweep has played, so the
    /// stimulus is off from then on; when the run ends the lease is given back and sweep
    /// mode ends — the next sweep is armed from the dialog again.
    pub(super) fn follow_sweep(&mut self, out: &mut Vec<Request>) {
        let Some(id) = self.rig.sweep.run else {
            return;
        };
        let Some(r) = self.daemon().and_then(|s| s.sweep.clone()) else {
            return;
        };
        if r.id != id || self.rig.sweep.seen.as_ref() == Some(&r.status) {
            return;
        }
        self.rig.sweep.seen = Some(r.status.clone());
        if !matches!(r.status, SweepStatus::Playing { .. })
            && matches!(
                self.rig.stimulus.phase,
                StimPhase::Firing | StimPhase::FireRequested
            )
        {
            self.rig.stimulus.phase = StimPhase::Idle;
        }
        match r.status {
            SweepStatus::Playing { .. } => {}
            SweepStatus::Analysing => {
                self.toast(ac2_scene::stim_item::SWEEP_ANALYSING);
            }
            SweepStatus::Done { traces } => {
                self.rig.sweep.run = None;
                // The first input's trace is shown; the others are listed beside it.
                let Some(&trace) = traces.first() else {
                    self.release_after_sweep(out);
                    return;
                };
                // The new result is selected under its measurement: listed highlighted,
                // named in the captions, and what the trace keys change.
                self.rig.selected = Some(r.meas);
                self.rig.choice.chosen = Some(r.meas);
                self.rig
                    .collapsed
                    .remove(&ac2_proto::model::TraceOwner::Meas { meas: r.meas });
                self.rig.selected_trace = Some(trace);
                self.rig.sweep.shown = Some(trace);
                self.rig.sweep.fit = Some(trace);
                self.fit_new_sweep();
                self.release_after_sweep(out);
                let of = self
                    .meas(r.meas)
                    .map_or_else(String::new, |m| format!(" of {}", m.config.name));
                let stored =
                    ac2_scene::meas_list::sweep_stored(&format!("{}{of}", r.name), traces.len());
                // A sweep pane on screen shows the result; the layout is otherwise the
                // operator's, and the toast says where the result is.
                match self.lead(PaneKind::Distortion) {
                    Some(p) => {
                        self.layout.set_focus(p);
                        let ir = self
                            .keys
                            .label(CommandId::SweepIr, Scope::Distortion)
                            .map_or_else(String::new, |k| format!("{k} impulse response, "));
                        let again = Chord::key(eframe::egui::Key::Space).label();
                        self.toast(format!(
                            "{stored}: {ir}{again} runs it again; dB / % in Settings › Display"
                        ));
                    }
                    None => self.toast(format!(
                        "{stored}: {} to see it in the focused pane",
                        self.to_it()
                    )),
                }
            }
            SweepStatus::Failed { msg, .. } => {
                self.rig.sweep.run = None;
                self.release_after_sweep(out);
                self.fault(ac2_scene::stim_item::sweep_failed(&msg));
            }
        }
    }

    /// A finished run ends sweep mode and gives the lease back, unless the operator has
    /// armed again meanwhile. Only a lease this client holds: a stop without one is the
    /// universal stop and would silence whoever took the stimulus over.
    pub(super) fn release_after_sweep(&mut self, out: &mut Vec<Request>) {
        if self.rig.stimulus.phase != StimPhase::Idle {
            return;
        }
        self.end_sweep_mode();
        let mine = self
            .daemon()
            .and_then(|s| s.generator.owner.as_ref())
            .is_some_and(|o| Some(o) == self.my_client_id());
        if mine {
            self.rig.stimulus.phase = StimPhase::Stopping;
            self.rig.stimulus.stop_transfers = false;
            self.rig.sweep.releasing = true;
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
            || self.rig.sweep.run.is_some()
            || self.rig.sweep.plan.is_some()
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
        self.rig.stimulus.phase = StimPhase::Stopping;
        self.rig.stimulus.stop_announced = true;
        // The transfer that asked for this stop is the last one running: nothing is left
        // for the stop to take with it, and it must not come back to stop transfers.
        self.rig.stimulus.stop_transfers = false;
        out.push(Request::StimStop);
        true
    }

    /// This connection holds the stimulus lease, armed or playing: its own arm landed.
    pub(super) fn holds_stimulus(&self) -> bool {
        match self.rig.stimulus.phase {
            StimPhase::Armed | StimPhase::FireRequested | StimPhase::Firing => true,
            // No lease this connection holds: an arm not yet granted, a stop on its way, or
            // one the mirror shows under this name from an earlier connection.
            StimPhase::Stopping | StimPhase::Idle | StimPhase::Arming => false,
        }
    }

    /// Sweep traces with their data, oldest first.
    pub fn sweep_traces(&self) -> Vec<(&TraceData, &GridDef)> {
        self.rig
            .traces
            .values()
            .filter(|(t, _)| t.meta.kind == TraceKind::Sweep && t.sweep.is_some())
            .map(|(t, g)| (t.as_ref(), g.as_ref()))
            .collect()
    }

    /// The runs of the group a sweep pane showing `shown` draws (its sweep measurement's),
    /// oldest first.
    fn group_sweeps(&self, shown: Option<&Measurement>) -> Vec<(&TraceData, &GridDef)> {
        let Some(group) = self.group_of_shown(shown) else {
            return Vec::new();
        };
        let ms = self.measurements();
        let mine = ac2_proto::model::TraceOwner::Meas { meas: group };
        self.sweep_traces()
            .into_iter()
            .filter(|(t, _)| ac2_scene::meas_list::group_of(&t.meta, &ms) == mine)
            .collect()
    }

    /// The run sweep pane `pane` shows, of its sweep measurement's group only: the selected
    /// trace when it is one of them, else the run of theirs selected last, else their
    /// newest. Never another measurement's run: that one is drawn only compared.
    pub fn pane_sweep(&self, pane: PaneId) -> Option<(&TraceData, &GridDef)> {
        self.sweep_shown_by(self.pane_meas(pane))
    }

    /// The run a sweep pane showing `shown` draws ([`Self::pane_sweep`]).
    fn sweep_shown_by(&self, shown: Option<&Measurement>) -> Option<(&TraceData, &GridDef)> {
        let runs = self.group_sweeps(shown);
        let find = |id: TraceId| runs.iter().find(|(t, _)| t.meta.id == id).copied();
        self.rig
            .selected_trace
            .and_then(find)
            .or_else(|| self.rig.sweep.shown.and_then(find))
            .or_else(|| runs.iter().max_by_key(|(t, _)| t.meta.id).copied())
    }

    /// The run "the sweep pane" shows, for what is about the kind rather than one pane (the
    /// sweep IR keys, Space's sweep): the focused sweep pane's, else the one focused last;
    /// with none on screen, what one would show.
    pub fn kind_sweep(&self) -> Option<(&TraceData, &GridDef)> {
        self.sweep_shown_by(self.kind_meas(PaneKind::Distortion))
    }

    /// The runs sweep pane `pane` draws behind its own, nearest first: each run entry, and
    /// the run each ticked sweep measurement's own sweep pane would draw. An imported run is
    /// no measurement's: it is not drawn until moved under one.
    pub fn compared_sweeps(&self, pane: PaneId) -> Vec<(&TraceData, &GridDef)> {
        if self.layout.kind(pane) != PaneKind::Distortion {
            return Vec::new();
        }
        let own = self.pane_sweep(pane).map(|(t, _)| t.meta.id);
        let ms = self.measurements();
        let mut v: Vec<(&TraceData, &GridDef)> = Vec::new();
        for e in self.compared_into(pane) {
            let run = match e {
                ac2_proto::model::layout::EntryRef::Trace { trace } => self
                    .sweep_traces()
                    .into_iter()
                    .find(|(t, _)| t.meta.id == trace),
                ac2_proto::model::layout::EntryRef::Group { meas } => {
                    self.sweep_shown_by(self.meas(meas))
                }
                ac2_proto::model::layout::EntryRef::Live { .. } => None,
            };
            if let Some(r) = run
                && Some(r.0.meta.id) != own
                && ac2_scene::meas_list::group_of(&r.0.meta, &ms)
                    != ac2_proto::model::TraceOwner::Imported
                && !v.iter().any(|(t, _)| t.meta.id == r.0.meta.id)
            {
                v.push(r);
            }
        }
        v
    }

    /// V / Shift+V on the sweep pane: the next / previous stored sweep run, selected as a
    /// click on its row selects it (the sweep pane draws the selected run; the trace keys
    /// follow it; no other pane changes).
    pub(super) fn cycle_sweep(&mut self, d: i32) {
        let pane = self.layout.focus;
        let ids: Vec<TraceId> = self
            .group_sweeps(self.pane_meas(pane))
            .iter()
            .map(|(t, _)| t.meta.id)
            .collect();
        if ids.is_empty() {
            self.warn(ac2_scene::room::no_sweep_results(&self.new_sweep_key()));
            return;
        }
        let cur = self.pane_sweep(pane).map(|(t, _)| t.meta.id);
        let i = cur
            .and_then(|c| ids.iter().position(|x| *x == c))
            .unwrap_or(0) as i32;
        let id = ids[(i + d).rem_euclid(ids.len() as i32) as usize];
        self.select_trace(id);
        if let Some((t, _)) = self.pane_sweep(pane) {
            let name = trace_label(&t.meta);
            self.toast(format!("{name} selected"));
        }
    }
}
