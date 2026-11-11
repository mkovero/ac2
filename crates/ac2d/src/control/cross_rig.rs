//! Transfer functions between rigs on the control thread (`docs/design/remote-inputs.md`
//! §3.3–§3.4): each one's clock gate follows the relation with its remote rig, and what its
//! job reports (a stop, a relock, a slip, the drift and late cues) becomes its entry in the
//! `remote` entity.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use ac2_proto::event::{Change, Patch};
use ac2_proto::model::{
    CrossRigTransfer, CrossTfState, DelayOutcome, DelaySlip, MeasKind, Measurement,
    NotLockedReason, PeerRelation, Relock, RelockOutcome, RemoteInput, RemoteSide, StopWhy,
    remote_slot,
};
use ac2_proto::units::{MeasId, RigId, Seconds, SessionEpoch, WallNs};
use ac2_proto::{ErrorCode, ProtoError};

use super::{Control, DelaySource};
use crate::conv;
use crate::jobs::JobCmd;
use crate::jobs::cross_rig::{ClockGate, CrossEvent};
use crate::util::{perr, wall_ns};

/// How long a settled relock and a slip are said on the pane.
const SHOWN_FOR: Duration = Duration::from_secs(60);
/// The span of a rig's own clock line: a minute of paired reads.
const OWN_LINE_S: f64 = 60.0;

/// A transfer function between this rig and `rig`, as the control thread follows it.
#[derive(Debug)]
pub(super) struct CrossTf {
    rig: RigId,
    side: RemoteSide,
    /// The gate last sent to its job.
    sent: Option<ClockGate>,
    state: CrossTfState,
    /// It averaged since its job started: a later return to agreement is a relock.
    ran: bool,
    /// Since when and first why it is stopped, after it ran.
    stopped: Option<(Instant, StopWhy)>,
    relock: Option<Relock>,
    /// When the relock settled (tracked again), for how long it is said.
    settled: Option<Instant>,
    slip: Option<(DelaySlip, Instant)>,
    drift: Option<f64>,
    late: bool,
}

/// The gate a relation sets: only agreement averages. The window is the span of the
/// evidence that failed, so the job holds the result from before it.
pub(super) fn gate_of(rel: &PeerRelation) -> ClockGate {
    match rel {
        PeerRelation::Agree => ClockGate::Run,
        PeerRelation::Measuring => ClockGate::Stop {
            why: StopWhy::Measuring,
            window_s: 0.0,
        },
        PeerRelation::NotLocked { reason } => ClockGate::Stop {
            why: StopWhy::NotLocked,
            window_s: match reason {
                NotLockedReason::CrossRate { over, .. } => over.0,
                NotLockedReason::OwnClock { .. } => OWN_LINE_S,
            },
        },
        PeerRelation::CantTell { .. } => ClockGate::Stop {
            why: StopWhy::CantTell,
            window_s: 0.0,
        },
    }
}

/// Which side of `kind` is remote, and on which rig, when it is a transfer function between
/// this rig and one other: both channels on one remote rig share its clock and are not one.
pub(super) fn cross_side(kind: &MeasKind, inputs: &[RemoteInput]) -> Option<(RigId, RemoteSide)> {
    let MeasKind::Transfer { config } = kind else {
        return None;
    };
    let rig = |ch: u16| {
        let slot = remote_slot(ch)?;
        inputs.iter().find(|r| r.slot == slot).map(|r| r.rig)
    };
    match (rig(config.reference_input), rig(config.measurement_input)) {
        (Some(r), None) => Some((r, RemoteSide::Reference)),
        (None, Some(r)) => Some((r, RemoteSide::Measurement)),
        _ => None,
    }
}

impl CrossTf {
    pub(super) fn new(rig: RigId, side: RemoteSide) -> Self {
        Self {
            rig,
            side,
            sent: None,
            state: CrossTfState::Stopped {
                why: StopWhy::Measuring,
                held: false,
            },
            ran: false,
            stopped: None,
            relock: None,
            settled: None,
            slip: None,
            drift: None,
            late: false,
        }
    }

    /// The relation with its rig is `rel` at `now`; the gate to send when it changed.
    /// `tracking`: its delay tracking is on (a relock then waits for it).
    pub(super) fn judge(
        &mut self,
        rel: &PeerRelation,
        tracking: bool,
        now: Instant,
    ) -> Option<ClockGate> {
        let gate = gate_of(rel);
        if self.sent == Some(gate) {
            return None;
        }
        self.sent = Some(gate);
        match gate {
            ClockGate::Run => {
                if self.ran
                    && let Some((since, from)) = self.stopped.take()
                {
                    self.relock = Some(Relock {
                        after: Seconds(now.duration_since(since).as_secs_f64()),
                        from,
                        outcome: if tracking {
                            RelockOutcome::Tracking
                        } else {
                            RelockOutcome::FindAgain
                        },
                    });
                    self.settled = None;
                }
                self.ran = true;
                self.stopped = None;
                self.state = CrossTfState::Running;
            }
            ClockGate::Stop { why, .. } => {
                let held = matches!(self.state, CrossTfState::Stopped { held: true, .. });
                if self.ran && self.stopped.is_none() {
                    self.stopped = Some((now, why));
                }
                self.state = CrossTfState::Stopped { why, held };
            }
        }
        Some(gate)
    }

    /// What its job reported; `fs` the session rate.
    pub(super) fn event(&mut self, e: &CrossEvent, fs: f64, now: Instant) {
        match e {
            CrossEvent::Stopped { held } => {
                if let CrossTfState::Stopped { why, .. } = self.state {
                    self.state = CrossTfState::Stopped { why, held: *held };
                }
            }
            CrossEvent::Relocked { was, now: arrival } => {
                if let Some(r) = &mut self.relock {
                    r.outcome = RelockOutcome::Tracked {
                        delay: Seconds(arrival / fs),
                        was: Seconds(was / fs),
                    };
                    self.settled = Some(now);
                }
            }
            CrossEvent::RelockAmbiguous(_) => {
                if let Some(r) = &mut self.relock {
                    r.outcome = RelockOutcome::Ambiguous;
                }
            }
            CrossEvent::FindAgain => {
                if let Some(r) = &mut self.relock {
                    r.outcome = RelockOutcome::FindAgain;
                }
            }
            CrossEvent::Slipped { by } => {
                self.slip = Some((
                    DelaySlip {
                        by: Seconds(*by as f64 / fs),
                    },
                    now,
                ));
            }
            CrossEvent::Drift(d) => self.drift = *d,
            CrossEvent::Late(on) => self.late = *on,
        }
    }

    /// The operator answered: a pick or a find settles a relock that waited for one.
    pub(super) fn resolved(&mut self) {
        if self.relock.is_some_and(|r| {
            matches!(
                r.outcome,
                RelockOutcome::Ambiguous | RelockOutcome::FindAgain
            )
        }) {
            self.relock = None;
        }
    }

    /// Its entry at `now`: a settled relock and a slip are said for a minute.
    pub(super) fn entry(&mut self, meas: MeasId, now: Instant) -> CrossRigTransfer {
        if self
            .settled
            .is_some_and(|t| now.duration_since(t) >= SHOWN_FOR)
        {
            self.relock = None;
            self.settled = None;
        }
        if self
            .slip
            .is_some_and(|(_, t)| now.duration_since(t) >= SHOWN_FOR)
        {
            self.slip = None;
        }
        CrossRigTransfer {
            meas,
            rig: self.rig,
            side: self.side,
            state: self.state,
            relock: self.relock,
            slip: self.slip.map(|(s, _)| s),
            delay_drift: self.drift.map(Seconds),
            late: self.late,
        }
    }
}

impl Control {
    /// Follows `m` as a transfer function between rigs when it is one, and tells its job
    /// how the clocks stand.
    pub(super) fn cross_start(&mut self, m: &Measurement) {
        let inputs = self
            .session
            .as_ref()
            .map(|rt| rt.open.config.remote_inputs.clone())
            .unwrap_or_default();
        match cross_side(&m.config.kind, &inputs) {
            Some((rig, side)) => {
                self.remote.cross.insert(m.id, CrossTf::new(rig, side));
                self.remote_publish();
            }
            None => self.cross_stop(m.id),
        }
    }

    pub(super) fn cross_stop(&mut self, id: MeasId) {
        if self.remote.cross.remove(&id).is_some() {
            self.remote_publish();
        }
    }

    /// Sends each cross-rig job the gate its rig's relation sets, where it changed.
    pub(super) fn cross_sync(&mut self) {
        let now = Instant::now();
        let mut sends = Vec::new();
        let tracking: BTreeMap<MeasId, bool> = self
            .store
            .state()
            .measurements
            .iter()
            .map(|m| (m.id, m.delay.as_ref().is_some_and(|d| d.tracking)))
            .collect();
        for (id, x) in &mut self.remote.cross {
            let rel = self
                .remote
                .watches
                .get(&x.rig)
                .map(|w| w.clock.relation)
                .unwrap_or(PeerRelation::CantTell {
                    reason: ac2_proto::model::CantTellReason::NoAudio,
                });
            let t = tracking.get(id).copied().unwrap_or(false);
            if let Some(g) = x.judge(&rel, t, now) {
                sends.push((*id, g));
            }
        }
        for (id, g) in sends {
            if let Some(j) = self.jobs.get(&id) {
                j.send(JobCmd::Clocks(g));
            }
        }
    }

    /// The cross-rig entries of the `remote` entity.
    pub(super) fn cross_entries(&mut self) -> Vec<CrossRigTransfer> {
        let now = Instant::now();
        self.remote
            .cross
            .iter_mut()
            .map(|(id, x)| x.entry(*id, now))
            .collect()
    }

    /// The operator set the delay or ran a find on `meas`.
    pub(super) fn cross_resolved(&mut self, meas: MeasId) {
        if let Some(x) = self.remote.cross.get_mut(&meas) {
            x.resolved();
        }
    }

    /// A cross-rig job reports.
    pub(super) fn cross_event(&mut self, meas: MeasId, epoch: SessionEpoch, e: CrossEvent) {
        let Some(fs) = self
            .session
            .as_ref()
            .filter(|rt| rt.epoch == epoch)
            .map(|rt| f64::from(rt.sample_rate))
        else {
            return;
        };
        match &e {
            CrossEvent::Slipped { by } => {
                if let Err(err) = self.slip_delay(meas, *by, fs) {
                    tracing::warn!("delay not moved with the slip: {}", err.msg);
                }
            }
            CrossEvent::RelockAmbiguous(r) => {
                let f = conv::delay_finding(r, fs, WallNs(wall_ns()));
                if let Ok(mut m) = self.meas(meas).cloned()
                    && let Some(d) = &mut m.delay
                {
                    // The job paused tracking: the operator picks, as after a find (1c).
                    d.awaiting_pick = matches!(f.outcome, DelayOutcome::Ambiguous { .. });
                    d.last_finding = Some(f);
                    self.commit(Change::Measurement(Patch::Set(m)));
                }
            }
            _ => {}
        }
        if let Some(x) = self.remote.cross.get_mut(&meas) {
            x.event(&e, fs, Instant::now());
        }
        self.remote_publish();
    }

    /// Moves the delay of `meas` by a slip of `by` samples, keeping the operator's offset.
    fn slip_delay(&mut self, meas: MeasId, by: i64, fs: f64) -> Result<(), ProtoError> {
        let d = self.transfer_delay(meas)?;
        let to = (d.applied_samples + by as f64) / fs;
        if !to.is_finite() {
            return Err(perr(ErrorCode::Invalid, "no delay"));
        }
        self.set_delay(meas, Seconds(to), DelaySource::Tracking)
            .map(|_| ())
    }
}

#[cfg(test)]
#[path = "cross_rig_tests.rs"]
mod tests;
