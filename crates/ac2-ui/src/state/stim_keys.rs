//! Space and Enter as stop keys, and what this app's own stop takes with it.
//!
//! The key that started something stops it: while this app's stimulus plays (or a fire
//! is on its way) Space and Enter stop it as Esc does, and while it is armed Space
//! disarms it. A stop is only ever easier to reach this way; arming and firing stay two
//! separate keys, and auto-repeat never reaches these keys ([`crate::app`]), so a held
//! Space that stopped never arms again.

use super::*;

/// This app's stimulus on the rig shown, as Space and Enter read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OwnStim {
    /// Nothing of this app's armed or playing (another client's may be).
    Idle,
    Arming,
    Armed,
    /// Fire requested or playing.
    Playing,
    Stopping,
}

impl AppState {
    pub(super) fn own_stim(&self) -> OwnStim {
        match self.rig.stimulus.phase {
            StimPhase::Arming => OwnStim::Arming,
            StimPhase::Armed => OwnStim::Armed,
            StimPhase::FireRequested | StimPhase::Firing => OwnStim::Playing,
            StimPhase::Stopping => OwnStim::Stopping,
            // A lease this client holds that the local phase has not caught up with (the
            // mirror after a reconnect): the mirror says what it is.
            StimPhase::Idle => match self.daemon().map(|s| &s.generator) {
                Some(g)
                    if g.owner
                        .as_ref()
                        .is_some_and(|o| Some(o) == self.my_client_id()) =>
                {
                    if g.firing {
                        OwnStim::Playing
                    } else if g.armed {
                        OwnStim::Armed
                    } else {
                        OwnStim::Idle
                    }
                }
                _ => OwnStim::Idle,
            },
        }
    }

    /// Space: stops this app's stimulus while it plays, disarms it while it is armed, else
    /// arms what the focused view plays. While an arm is on its way it does nothing (Esc
    /// still stops): a quick second Space is the arm's own key pressed twice, not a stop.
    pub(super) fn space_key(&mut self, keymap: &Keymap, out: &mut Vec<Request>) {
        match self.own_stim() {
            OwnStim::Armed | OwnStim::Playing => self.stop_stimulus_here(out),
            OwnStim::Arming => {}
            OwnStim::Idle | OwnStim::Stopping => self.space(false, keymap, out),
        }
    }

    /// Enter: stops this app's stimulus while it plays or a fire is on its way; `false`
    /// when it leaves Enter its other meaning (firing what is armed).
    pub(super) fn enter_stops(&mut self, out: &mut Vec<Request>) -> bool {
        if self.own_stim() == OwnStim::Playing {
            self.stop_stimulus_here(out);
            true
        } else {
            false
        }
    }

    /// Stops the stimulus of the rig shown where it is armed or firing, and drops a re-arm
    /// waiting on a stop: Esc, Shift+Esc, Space and Enter, and the palette's stop. Sent
    /// while it plays, with the preference on, the stop takes the transfers measuring
    /// against the loopback with it once it lands.
    pub(super) fn stop_stimulus_here(&mut self, out: &mut Vec<Request>) {
        self.rig.stimulus.arm_after_stop = false;
        self.rig.sweep.arm_after_stop = false;
        if self.stimulus_live() {
            let playing = matches!(
                self.rig.stimulus.phase,
                StimPhase::FireRequested | StimPhase::Firing
            ) || self.daemon().is_some_and(|s| s.generator.firing);
            self.rig.stimulus.stop_transfers = playing && self.prefs.stop_transfers_with_stimulus;
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
                done: String::new(),
                failed: format!("{name} not stopped"),
            });
        }
        stopping.into_iter().map(|(_, n)| n).collect()
    }
}
