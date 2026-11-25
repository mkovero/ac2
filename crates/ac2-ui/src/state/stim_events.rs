//! What the daemon says of this app's stimulus lease: armed, playing, a run started,
//! stopped, lost or refused.

use super::*;

impl AppState {
    /// One stimulus event on the rig shown: the phase follows it, and a stop of this app's
    /// that has landed takes the transfers it asked for with it.
    pub(super) fn stim_event(&mut self, s: StimEvent, out: &mut Vec<Request>) {
        match s {
            StimEvent::Armed => {
                self.rig.stimulus.phase = StimPhase::Armed;
                if self.rig.due.armed_with.take() != self.rig.stimulus.settings() {
                    self.resend_stimulus(out);
                }
                let d = self.rig.stimulus.describe();
                if let Some(p) = &self.rig.dynamics_run.plan {
                    self.toast(ac2_scene::stim_item::armed_dynamics_toast(&d, &p.name));
                } else if self.rig.sweep.plan.is_some() {
                    self.toast(ac2_scene::stim_item::armed_sweep_toast(&d));
                } else {
                    self.toast(ac2_scene::stim_item::armed_toast(&d));
                }
            }
            StimEvent::DynamicsStarted(run) => {
                self.rig.stimulus.phase = StimPhase::Firing;
                self.rig.dynamics_run.run = Some(run.id);
                self.rig.dynamics_run.seen = Some(run.status.clone());
                self.toast(ac2_scene::stim_item::dynamics_started(
                    &run.name,
                    run.longest.0,
                ));
            }
            StimEvent::SweepStarted(run) => {
                self.rig.stimulus.phase = StimPhase::Firing;
                self.rig.sweep.run = Some(run.id);
                self.rig.sweep.seen = Some(run.status.clone());
                self.toast(ac2_scene::stim_item::sweep_started(
                    run.repeats,
                    run.sweep_duration.0 + run.post_roll.0,
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
                // The mirror may still show the generator as this client left it: until it
                // agrees, the bar says the stop and the keys wait.
                self.rig.stimulus.stopped_unmirrored = self.mirror_lease_mine();
                let announced = std::mem::take(&mut self.rig.stimulus.stop_announced);
                let transfers = self.stop_loopback_transfers(out);
                if !transfers.is_empty() {
                    self.toast(ac2_scene::stim_rows::stopped_with(&transfers));
                }
                let releasing = std::mem::take(&mut self.rig.sweep.releasing);
                let pending = std::mem::take(&mut self.rig.sweep.arm_after_stop);
                match self.rig.sweep.plan.take() {
                    Some(plan) if pending => self.arm_sweep(plan, false, out),
                    plan => {
                        self.rig.sweep.plan = plan;
                        self.end_sweep_mode();
                        if !releasing && !announced && transfers.is_empty() {
                            self.toast(ac2_scene::stim_rows::stopped_with(&[]));
                        }
                    }
                }
            }
            StimEvent::Lost(msg) => {
                self.rig.stimulus.phase = StimPhase::Idle;
                self.rig.stimulus.stopped_unmirrored = false;
                self.rig.stimulus.stop_announced = false;
                self.rig.stimulus.stop_transfers = false;
                self.rig.sweep.releasing = false;
                self.rig.sweep.arm_after_stop = false;
                self.end_sweep_mode();
                self.fault(format!("stimulus lease lost: {msg}"));
            }
            StimEvent::Failed(msg) => {
                self.rig.sweep.releasing = false;
                self.rig.stimulus.stopped_unmirrored = false;
                self.rig.stimulus.stop_announced = false;
                self.rig.stimulus.stop_transfers = false;
                if std::mem::take(&mut self.rig.sweep.arm_after_stop) {
                    self.end_sweep_mode();
                }
                self.rig.stimulus.phase = match self.rig.stimulus.phase {
                    StimPhase::Arming => StimPhase::Idle,
                    // A fire is only sent from this run's own arm: refused, it is armed as
                    // it was.
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
