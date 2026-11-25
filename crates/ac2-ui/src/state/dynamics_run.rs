//! A dynamics run from the app: Space on a Dynamics pane arms its lead measurement (asking
//! first, when it has runs, whether to replay one's rests), Enter plays it, Esc stops. The
//! generator is armed as the daemon's `dynamics.run` needs it: the carrier at the loudest
//! burst level, on the measurement's outputs, so the top bar and the ceiling check name
//! the loudest the programme plays.

use crate::state::*;
use ac2_proto::model::{DynamicsConfig, DynamicsStatus, TraceKind, TraceOwner};
use ac2_proto::units::DynamicsId;

/// A dynamics run armed (or arming): Enter plays it.
#[derive(Clone, Debug, PartialEq)]
pub struct DynamicsPlan {
    pub meas: MeasId,
    pub name: String,
    pub config: DynamicsConfig,
    /// The run whose rests are replayed; `None`: adaptive rests.
    pub rests_from: Option<TraceId>,
}

/// Dynamics runs as this client runs them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DynamicsRunUi {
    pub plan: Option<DynamicsPlan>,
    /// The run this client started, followed until it is stored or fails.
    pub run: Option<DynamicsId>,
    /// The last status of that run acted on.
    pub seen: Option<DynamicsStatus>,
}

impl AppState {
    /// Opens the dynamics dialog (new measurement), or says why it cannot open.
    pub(super) fn open_dynamics_dialog(&mut self, keymap: &Keymap, out: &mut Vec<Request>) {
        let Some(o) = self.open_stream().cloned() else {
            self.warn(format!(
                "no audio I/O to play on: {}",
                open_stream_hint(keymap)
            ));
            return;
        };
        if let Some(why) = self.two_input_refusal(true, keymap) {
            self.warn(why);
            return;
        }
        let made = self.daemon().map_or(0, |s| {
            s.measurements
                .iter()
                .filter(|m| matches!(m.config.kind, MeasKind::Dynamics { .. }))
                .count()
        });
        let f = Form::dynamics(
            Some(&o),
            made,
            &self.stream_input_names(),
            &self.stream_output_names(),
        )
        .play_on(Some(&o), &self.rig.stimulus.outputs);
        self.overlay = Overlay::Form(Box::new(f));
        if self.rig.devices.is_none() {
            out.push(Request::Devices);
        }
    }

    /// The runs of dynamics measurement `meas`, newest first.
    fn dynamics_runs_of(&self, meas: MeasId) -> Vec<(TraceId, String)> {
        let ms = self.measurements();
        let mine = TraceOwner::Meas { meas };
        let mut v: Vec<(TraceId, String)> = self
            .daemon()
            .map(|s| {
                s.traces
                    .iter()
                    .filter(|t| {
                        t.kind == TraceKind::Dynamics
                            && ac2_scene::meas_list::group_of(t, &ms) == mine
                    })
                    .map(|t| (t.id, t.edit.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        v.sort_by_key(|(id, _)| std::cmp::Reverse(*id));
        v
    }

    /// Space on a Dynamics pane: its lead measurement armed for a run. One of another rig
    /// is refused: the run plays on the rig shown.
    pub(super) fn space_dynamics(&mut self, force: bool, keymap: &Keymap, out: &mut Vec<Request>) {
        if self.rig.dynamics_run.run.is_some()
            || (self.rig.dynamics_run.plan.is_some() && self.rig.stimulus.phase != StimPhase::Idle)
        {
            return;
        }
        let pane = self.layout.focus;
        if !self.pane_ours(pane) {
            self.warn("this pane shows another rig's measurement: a run plays on the rig shown");
            return;
        }
        let Some(m) = self
            .pane_meas(pane)
            .filter(|m| matches!(m.config.kind, MeasKind::Dynamics { .. }))
            .cloned()
        else {
            self.open_dynamics_dialog(keymap, out);
            return;
        };
        if !self.connected() {
            self.warn("not connected");
            return;
        }
        if self.daemon().is_some_and(|s| s.stream.open.is_none()) {
            self.warn(format!(
                "no audio I/O to play on: {}",
                open_stream_hint(keymap)
            ));
            return;
        }
        let runs = self.dynamics_runs_of(m.id);
        if runs.is_empty() {
            self.arm_dynamics(m.id, None, force, out);
            return;
        }
        let names: Vec<&str> = runs.iter().map(|(_, n)| n.as_str()).collect();
        let (title, choices) = ac2_scene::dynamics::rests_question(&m.config.name, &names);
        self.overlay = Overlay::Choose(Box::new(ChoicePrompt {
            purpose: ChoicePurpose::DynamicsRests {
                meas: m.id,
                force,
                runs: runs.iter().map(|(t, _)| *t).collect(),
            },
            label: title.clone(),
            title,
            lines: Vec::new(),
            choices,
            owners: Vec::new(),
            index: 0,
            hint: "Enter arms · Esc cancels".into(),
        }));
    }

    /// The rests answer: 0 adapts them, `n` replays run `runs[n − 1]`'s.
    pub(super) fn dynamics_rests_pick(
        &mut self,
        meas: MeasId,
        force: bool,
        runs: &[TraceId],
        i: usize,
        out: &mut Vec<Request>,
    ) {
        let rests_from = i.checked_sub(1).and_then(|n| runs.get(n).copied());
        self.arm_dynamics(meas, rests_from, force, out);
    }

    /// Arms the generator for a run of `meas`.
    pub(super) fn arm_dynamics(
        &mut self,
        meas: MeasId,
        rests_from: Option<TraceId>,
        force: bool,
        out: &mut Vec<Request>,
    ) {
        let Some(m) = self.meas(meas).cloned() else {
            return;
        };
        let MeasKind::Dynamics { config } = &m.config.kind else {
            return;
        };
        let Some(top) = config.bursts.iter().map(|b| b.0).reduce(f64::max) else {
            return;
        };
        if let Some(c) = self.ceiling()
            && top > c.0
        {
            self.warn(ac2_scene::stim_rows::burst_above_max(
                &m.config.name,
                top,
                c,
                ac2_scene::stim_rows::BurstFix::RaiseMaxOrEdit,
            ));
            return;
        }
        match self.rig.stimulus.phase {
            StimPhase::Firing | StimPhase::FireRequested => {
                self.warn(format!(
                    "the stimulus is firing: {STOP_ANYWHERE} stops it, then arm the run"
                ));
                return;
            }
            StimPhase::Stopping => {
                self.warn("the stimulus is stopping: Space again once it has stopped");
                return;
            }
            _ => {}
        }
        // A sweep set up before gives way to this run.
        self.rig.sweep.plan = None;
        self.rig.stimulus.signal = ac2_proto::model::Signal::Sine {
            freq: config.carrier,
        };
        self.rig.stimulus.level = Some(Dbfs(top));
        self.rig.stimulus.outputs = config.outputs.clone();
        let from_name = rests_from.and_then(|t| {
            self.dynamics_runs_of(meas)
                .into_iter()
                .find(|(id, _)| *id == t)
                .map(|(_, n)| n)
        });
        self.rig.dynamics_run.plan = Some(DynamicsPlan {
            meas,
            name: m.config.name.clone(),
            config: config.clone(),
            rests_from,
        });
        match self.rig.stimulus.phase {
            StimPhase::Idle => {
                if let Some(settings) = self.rig.stimulus.settings() {
                    self.rig.stimulus.phase = StimPhase::Arming;
                    self.rig.due.armed_with = Some(settings.clone());
                    out.push(Request::StimArm { settings, force });
                }
            }
            _ => self.resend_stimulus(out),
        }
        self.toast(ac2_scene::dynamics::armed_text(
            &m.config.name,
            &format::dbfs(top),
            from_name.as_deref(),
        ));
    }

    /// Enter while armed with a dynamics run: `dynamics.run` of its measurement.
    pub(super) fn fire_dynamics(&mut self, out: &mut Vec<Request>) {
        let Some(p) = &self.rig.dynamics_run.plan else {
            return;
        };
        let (meas, rests_from) = (p.meas, p.rests_from);
        self.rig.stimulus.phase = StimPhase::FireRequested;
        out.push(Request::DynamicsRun { meas, rests_from });
    }

    /// The run's progress for the title of a Dynamics pane showing its measurement: any
    /// client's run, as the mirror says.
    pub fn dynamics_progress(&self, pane: PaneId) -> Option<String> {
        let m = self.pane_meas(pane)?;
        let r = self.daemon()?.dynamics.as_ref()?;
        match r.status {
            DynamicsStatus::Playing { step, steps, phase } if r.meas == m.id => {
                Some(ac2_scene::dynamics::progress_text(step, steps, phase))
            }
            _ => None,
        }
    }

    /// Acts on the run this client started as the mirror reports it: stored, its trace is
    /// selected (its measurement's panes draw it); failed, a U1 banner says why.
    pub(super) fn follow_dynamics(&mut self, out: &mut Vec<Request>) {
        let Some(id) = self.rig.dynamics_run.run else {
            return;
        };
        let Some(r) = self.daemon().and_then(|s| s.dynamics.clone()) else {
            return;
        };
        if r.id != id || self.rig.dynamics_run.seen.as_ref() == Some(&r.status) {
            return;
        }
        self.rig.dynamics_run.seen = Some(r.status.clone());
        if !matches!(r.status, DynamicsStatus::Playing { .. })
            && matches!(
                self.rig.stimulus.phase,
                StimPhase::Firing | StimPhase::FireRequested
            )
        {
            self.rig.stimulus.phase = StimPhase::Idle;
        }
        match r.status {
            DynamicsStatus::Playing { .. } => {}
            DynamicsStatus::Done { trace } => {
                self.rig.dynamics_run.run = None;
                self.rig.selected = Some(r.meas);
                self.rig.choice.chosen = Some(r.meas);
                self.rig
                    .collapsed
                    .remove(&TraceOwner::Meas { meas: r.meas });
                self.rig.selected_trace = Some(trace);
                self.release_after_sweep(out);
                if let Some(p) = self.lead(PaneKind::Dynamics) {
                    self.layout.set_focus(p);
                }
                self.toast(format!("{} stored", r.name));
            }
            DynamicsStatus::Failed { msg, .. } => {
                self.rig.dynamics_run.run = None;
                self.release_after_sweep(out);
                self.fault(format!("dynamics run failed: {msg}"));
            }
        }
    }
}
