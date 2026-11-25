//! Space and Enter as stop keys, and what this app's own stop takes with it.
//!
//! The key that started something stops it: while this app's stimulus plays (or a fire
//! is on its way) Space and Enter stop it as Esc does, and while it is armed Space
//! disarms it. A stop is only ever easier to reach this way: arming and firing stay two
//! separate keys, and while a stop is on its way Space and Enter do nothing, so a key
//! pressed twice (or held, whether or not the platform flags the repeat) that stopped the
//! sound never starts it again.

use super::*;
use ac2_scene::stim_item::StimState;

/// This app's stimulus on the rig shown, as Space, Enter and the bar read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnStim {
    /// Nothing of this app's armed or playing (another client's may be).
    Idle,
    Arming,
    Armed,
    /// The generator's noise or sine, fire requested or playing.
    Playing,
    /// A sweep or dynamics run, fire requested or playing: a second Enter or Space must
    /// not discard what a first one started, so only Esc ends it.
    Run,
    /// This app's stop has gone out, or has landed while the mirror still shows the
    /// generator as this client left it.
    Stopping,
    /// The mirror shows a lease under this client's name that this run of the app did not
    /// arm (an earlier connection's): Space stops it; Enter cannot fire it, since this
    /// connection holds no lease the daemon would take the fire under.
    Elsewhere,
}

impl AppState {
    pub fn own_stim(&self) -> OwnStim {
        let run_planned = self.rig.sweep.plan.is_some() || self.rig.dynamics_run.plan.is_some();
        if self.rig.sweep.run.is_some() || self.rig.dynamics_run.run.is_some() {
            return OwnStim::Run;
        }
        match self.rig.stimulus.phase {
            StimPhase::Arming => OwnStim::Arming,
            StimPhase::Armed => OwnStim::Armed,
            StimPhase::FireRequested | StimPhase::Firing if run_planned => OwnStim::Run,
            StimPhase::FireRequested | StimPhase::Firing => OwnStim::Playing,
            StimPhase::Stopping => OwnStim::Stopping,
            StimPhase::Idle if self.rig.stimulus.stopped_unmirrored => OwnStim::Stopping,
            StimPhase::Idle if self.mirror_lease_mine() => OwnStim::Elsewhere,
            StimPhase::Idle => OwnStim::Idle,
        }
    }

    /// The mirror shows the generator armed or playing under this client's name.
    pub(super) fn mirror_lease_mine(&self) -> bool {
        self.daemon().is_some_and(|s| {
            (s.generator.armed || s.generator.firing)
                && s.generator
                    .owner
                    .as_ref()
                    .is_some_and(|o| Some(o) == self.my_client_id())
        })
    }

    /// What the fixed item says plays: this app's own stimulus as this run of the app set
    /// it while it arms, holds or stops it; any other (another client's, or one under this
    /// name from elsewhere) as the daemon holds it, which is what sounds.
    pub fn stim_plays(&self) -> String {
        let local = self.rig.stimulus.phase != StimPhase::Idle;
        let mirrored = self
            .daemon()
            .filter(|s| !local && (s.generator.armed || s.generator.firing))
            .and_then(|s| s.generator.settings.as_ref());
        let labels = self
            .daemon()
            .map(|s| s.outputs.as_slice())
            .unwrap_or_default();
        let (prefix, level, outputs) = match mirrored {
            Some(g) => (
                ac2_scene::stimulus::level_prefix(Some(&g.signal)),
                Some(g.level),
                g.outputs.as_slice(),
            ),
            None => (
                if self.rig.sweep.plan.is_some() {
                    "sweep ".to_owned()
                } else {
                    ac2_scene::stimulus::level_prefix(self.generator_signal().as_ref())
                },
                self.rig.stimulus.level,
                self.rig.stimulus.outputs.as_slice(),
            ),
        };
        ac2_scene::stim_item::plays(
            &prefix,
            level,
            &ac2_scene::rig::stimulus_outputs(outputs, labels),
        )
    }

    /// The stimulus of the rig shown as the top bar states it; `None` before the daemon's
    /// state is mirrored. The mirrored generator is the truth, but for this app's stop in
    /// flight: the mirror says it plays until the stop lands, and the keys do nothing
    /// meanwhile, so the bar says `STOPPING…` and offers no key.
    pub fn stim_state(&self) -> Option<StimState> {
        let g = &self.daemon()?.generator;
        // The local phase leads where the mirror lags it: an arm this app saw land, a stop
        // it sent or saw land before the mirror says so.
        Some(match ((g.armed, g.firing), self.rig.stimulus.phase) {
            (_, StimPhase::Stopping) => StimState::Stopping,
            (_, StimPhase::Idle) if self.rig.stimulus.stopped_unmirrored => StimState::Stopping,
            ((_, true), _) | (_, StimPhase::Firing) => StimState::Firing,
            (_, StimPhase::FireRequested) => StimState::FirePending,
            ((true, false), _) | (_, StimPhase::Armed) => StimState::Armed,
            (_, StimPhase::Arming) => StimState::Arming,
            _ => StimState::Off,
        })
    }

    /// Space: stops this app's stimulus while it plays, disarms it while it is armed, else
    /// arms what the focused view plays. While an arm, a stop or a run is on its way it does
    /// nothing (Esc still stops): a quick second Space is the first one pressed twice, and
    /// must neither stop what it armed nor arm what it stopped.
    pub(super) fn space_key(&mut self, keymap: &Keymap, out: &mut Vec<Request>) {
        match self.own_stim() {
            OwnStim::Armed | OwnStim::Playing | OwnStim::Elsewhere => self.stop_stimulus_here(out),
            OwnStim::Arming | OwnStim::Run | OwnStim::Stopping => {}
            OwnStim::Idle => self.space(false, keymap, out),
        }
    }

    /// Enter: fires what this run of the app armed, stops this app's noise or sine while it plays; nothing
    /// while an arm, a stop or a run is on its way.
    pub(super) fn enter_key(&mut self, out: &mut Vec<Request>) {
        match self.own_stim() {
            OwnStim::Playing => self.stop_stimulus_here(out),
            OwnStim::Armed if self.rig.dynamics_run.plan.is_some() => self.fire_dynamics(out),
            OwnStim::Armed if self.rig.sweep.plan.is_some() => self.fire_sweep(out),
            OwnStim::Armed => {
                self.sync_generator_signal();
                if let Some(settings) = self.rig.stimulus.settings() {
                    self.rig.stimulus.phase = StimPhase::FireRequested;
                    out.push(Request::StimSet(GeneratorDesired {
                        settings,
                        armed: true,
                        firing: true,
                    }));
                }
            }
            OwnStim::Idle => self.warn(ac2_scene::stim_item::NOT_ARMED),
            OwnStim::Elsewhere => self.warn(ac2_scene::stim_item::ARMED_ELSEWHERE),
            OwnStim::Arming | OwnStim::Run | OwnStim::Stopping => {}
        }
    }

    /// The take-over: arms this view's stimulus in place of another client's. With no
    /// other client holding the generator there is nothing to take over, and this app's own
    /// is stopped or armed with its own keys.
    pub(super) fn take_over(&mut self, keymap: &Keymap, out: &mut Vec<Request>) {
        let other = self.daemon().is_some_and(|s| {
            s.generator
                .owner
                .as_ref()
                .is_some_and(|o| Some(o) != self.my_client_id())
        });
        if other {
            self.space(true, keymap, out);
        }
    }

    /// Stops the stimulus of the rig shown where it is armed or firing, and drops a re-arm
    /// waiting on a stop: Esc, Shift+Esc, Space and Enter, and the palette's stop. Sent
    /// while it plays, with the preference on, the stop takes the transfers measuring
    /// against the loopback with it once it lands.
    pub(super) fn stop_stimulus_here(&mut self, out: &mut Vec<Request>) {
        self.rig.sweep.arm_after_stop = false;
        if self.stimulus_live() {
            // Only this app's own noise or sine, started in this run: another client's
            // stimulus feeds measurements this app did not start it for, and an aborted run
            // was never their signal. A second stop (Esc after Space) is the same stop: it
            // keeps what the first asked for.
            if self.rig.stimulus.phase != StimPhase::Stopping {
                self.rig.stimulus.stop_transfers =
                    self.own_stim() == OwnStim::Playing && self.prefs.stop_transfers_with_stimulus;
            }
            self.rig.stimulus.phase = StimPhase::Stopping;
            out.push(Request::StimStop);
        }
    }

    /// This app's stop has landed: stops each running transfer measurement whose reference
    /// is the loopback input, when the stop asked for it. Their names, for the toast; a
    /// transfer on another reference (a mic, another rig's input) keeps running, since its
    /// reference still carries what it measures.
    pub(super) fn stop_loopback_transfers(&mut self, out: &mut Vec<Request>) -> Vec<String> {
        if !std::mem::take(&mut self.rig.stimulus.stop_transfers) {
            return Vec::new();
        }
        let Some(loopback) = self
            .open_session()
            .and_then(|o| o.config.loopback)
            .map(|l| l.input)
        else {
            return Vec::new();
        };
        let stopping: Vec<(MeasId, String)> = self
            .measurements()
            .into_iter()
            .filter(|m| m.running)
            .filter(|m| {
                matches!(&m.config.kind, MeasKind::Transfer { config }
                    if config.reference_input == loopback)
            })
            .map(|m| (m.id, m.config.name.clone()))
            .collect();
        for (meas, name) in &stopping {
            // The toast names them all at once; each reply says only a refusal.
            out.push(Request::Act {
                cmd: Command::MeasStop { meas: *meas },
                done: None,
                failed: format!("{name} not stopped"),
            });
        }
        stopping.into_iter().map(|(_, n)| n).collect()
    }
}
