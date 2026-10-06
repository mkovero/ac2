//! The state store: one mirrored [`State`], one `rev` counter, serial commits, and the
//! bounded replay buffer behind `state.since` (Q5).
//!
//! Every commit is a [`Change`] carrying an entity's full new value; applying it here and on
//! a client is the same assignment, so a client that applies the events after a snapshot's
//! rev in order holds exactly this state.

use std::collections::VecDeque;
use std::time::Instant;

use ac2_proto::event::{Change, Event, Patch, StateSnapshot};
use ac2_proto::model::{
    Autosave, AutosaveState, Generator, Session, State, TimingState, TimingStatus,
};
use ac2_proto::units::{DaemonIncarnation, Dbfs, Rev, SessionEpoch};

use crate::config::ReplayLimits;

/// Serial state store.
#[derive(Debug)]
pub(crate) struct Store {
    state: State,
    rev: Rev,
    replay: VecDeque<(Instant, Event)>,
    limits: ReplayLimits,
}

impl Store {
    /// Fresh state of a new incarnation: no session, disarmed generator with no owner.
    pub(crate) fn new(ceiling: Dbfs, ceiling_bound: Dbfs, limits: ReplayLimits) -> Self {
        Self {
            state: State {
                session: Session {
                    epoch: SessionEpoch(0),
                    open: None,
                    stopped: None,
                },
                measurements: Vec::new(),
                traces: Vec::new(),
                generator: Generator {
                    owner: None,
                    armed: false,
                    firing: false,
                    settings: None,
                    ceiling,
                    ceiling_bound,
                    last_action: None,
                },
                calibrations: Vec::new(),
                mics: Vec::new(),
                inputs: Vec::new(),
                outputs: Vec::new(),
                spl_logs: Vec::new(),
                timing: TimingStatus {
                    epoch: 0,
                    state: TimingState::NoStimulus,
                    last_lock: None,
                    drift: None,
                    internal_reference: false,
                },
                sweep: None,
                autosave: Autosave {
                    state: AutosaveState::Off,
                    saved_at: None,
                },
                recording: None,
            },
            rev: Rev(0),
            replay: VecDeque::new(),
            limits,
        }
    }

    /// The initial state carries the calibration store's contents (rev 0, no events).
    pub(crate) fn with_calibrations(mut self, c: crate::calstore::Contents) -> Self {
        self.state.calibrations = c.calibrations;
        self.state.mics = c.mics;
        self.state.inputs = c.inputs;
        self
    }

    /// The initial state carries the rig settings' output labels (rev 0, no events).
    pub(crate) fn with_outputs(mut self, outputs: Vec<ac2_proto::model::OutputSetup>) -> Self {
        self.state.outputs = outputs;
        self
    }

    /// The initial autosave status (rev 0, no events).
    pub(crate) fn with_autosave(mut self, autosave: Autosave) -> Self {
        self.state.autosave = autosave;
        self
    }

    pub(crate) fn rev(&self) -> Rev {
        self.rev
    }

    pub(crate) fn state(&self) -> &State {
        &self.state
    }

    /// Applies `change`, bumps `rev` and records the event for replay.
    pub(crate) fn commit(&mut self, change: Change, now: Instant) -> Event {
        apply(&mut self.state, &change);
        self.rev = Rev(self.rev.0 + 1);
        let ev = Event {
            rev: self.rev,
            change,
        };
        self.replay.push_back((now, ev.clone()));
        self.evict(now);
        ev
    }

    fn evict(&mut self, now: Instant) {
        while self.replay.len() > self.limits.events {
            self.replay.pop_front();
        }
        while self
            .replay
            .front()
            .is_some_and(|(t, _)| now.saturating_duration_since(*t) > self.limits.age)
        {
            self.replay.pop_front();
        }
    }

    /// Events after `rev`, or `Err(oldest replayable rev)` when the gap has been evicted (or
    /// `rev` is not from this incarnation's history).
    pub(crate) fn since(&mut self, rev: Rev, now: Instant) -> Result<Vec<Event>, Rev> {
        self.evict(now);
        let oldest = self
            .replay
            .front()
            .map_or(Rev(self.rev.0 + 1), |(_, e)| e.rev);
        if rev > self.rev || rev.0 + 1 < oldest.0 {
            return Err(oldest);
        }
        Ok(self
            .replay
            .iter()
            .filter(|(_, e)| e.rev > rev)
            .map(|(_, e)| e.clone())
            .collect())
    }

    pub(crate) fn snapshot(&self, incarnation: DaemonIncarnation) -> StateSnapshot {
        StateSnapshot {
            state: self.state.clone(),
            rev: self.rev,
            daemon_incarnation: incarnation,
            session_epoch: self.state.session.epoch,
        }
    }
}

/// Replaces the entity with the same key, or appends it (ids are allocated increasing, so
/// keyed lists stay in creation order).
fn upsert<T: Clone, K: PartialEq>(v: &mut Vec<T>, patch: &Patch<T, K>, key: impl Fn(&T) -> K) {
    match patch {
        Patch::Set(x) => {
            let k = key(x);
            match v.iter().position(|e| key(e) == k) {
                Some(i) => v[i] = x.clone(),
                None => v.push(x.clone()),
            }
        }
        Patch::Deleted(k) => v.retain(|e| key(e) != *k),
    }
}

/// Applies one change: plain assignment of the entity's new value.
pub(crate) fn apply(s: &mut State, c: &Change) {
    match c {
        Change::Session(x) => s.session = x.clone(),
        Change::Measurement(p) => upsert(&mut s.measurements, p, |m| m.id),
        Change::Trace(p) => upsert(&mut s.traces, p, |t| t.id),
        Change::Generator(g) => s.generator = g.clone(),
        Change::Calibration(p) => upsert(&mut s.calibrations, p, |c| c.key.clone()),
        Change::Mic(p) => upsert(&mut s.mics, p, |m| m.name.clone()),
        Change::Inputs(i) => s.inputs = i.clone(),
        Change::Outputs(o) => s.outputs = o.clone(),
        Change::SplLog(p) => upsert(&mut s.spl_logs, p, |l| l.meas),
        Change::Timing(t) => s.timing = *t,
        Change::Sweep(r) => s.sweep = Some(r.clone()),
        Change::Autosave(a) => s.autosave = a.clone(),
        Change::Recording(r) => s.recording = Some(r.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn store(events: usize) -> Store {
        Store::new(
            Dbfs(-10.0),
            Dbfs(-10.0),
            ReplayLimits {
                events,
                age: Duration::from_secs(60),
            },
        )
    }

    fn bump(s: &mut Store, now: Instant) -> Event {
        let mut g = s.state().generator.clone();
        g.armed = !g.armed;
        s.commit(Change::Generator(g), now)
    }

    #[test]
    fn since_replays_and_expires() {
        let t0 = Instant::now();
        let mut s = store(4);
        for _ in 0..3 {
            bump(&mut s, t0);
        }
        assert_eq!(s.since(Rev(0), t0).expect("replayable").len(), 3);
        assert_eq!(s.since(Rev(3), t0).expect("replayable").len(), 0);
        for _ in 0..3 {
            bump(&mut s, t0);
        }
        // Six commits, four kept: revs 3..=6; since(2) still works, since(1) does not.
        assert_eq!(s.since(Rev(2), t0).expect("replayable").len(), 4);
        assert_eq!(s.since(Rev(1), t0), Err(Rev(3)));
        assert_eq!(s.since(Rev(99), t0), Err(Rev(3)));
        // Age eviction.
        let later = t0 + Duration::from_secs(61);
        assert_eq!(s.since(Rev(5), later), Err(Rev(7)));
        assert_eq!(s.since(Rev(6), later).expect("current").len(), 0);
    }

    #[test]
    fn events_reproduce_state() {
        let t0 = Instant::now();
        let mut s = store(100);
        let base = s.state().clone();
        let mut evs = Vec::new();
        for _ in 0..5 {
            evs.push(bump(&mut s, t0));
        }
        let mut mirror = base;
        for e in &evs {
            apply(&mut mirror, &e.change);
        }
        assert_eq!(&mirror, s.state());
    }
}
