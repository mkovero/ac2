//! `sim.run`, `sim.fit`, `sim.cancel`: room simulations on a worker (`crate::sim`), their
//! progress and results as `State.sim`. A change to the simulated plan, the deletion of the
//! walk it is heard at, or a session close stops the run.

use std::time::Duration;

use ac2_proto::event::{Change, Patch};
use ac2_proto::model::{
    PlanSpec, PointState, SimAbsorption, SimCancel, SimFitRequest, SimKind, SimReceivers,
    SimRefusal, SimRequest, SimSettings, SimStage, SimState, SimStatus, WalkKind,
};
use ac2_proto::units::{SimId, WalkId};
use ac2_proto::{ErrorCode, ErrorDetail, ProtoError, ReplyBody};
use ac2_roomsim::PerBand;

use super::Control;
use crate::sim::run::{Job, SimEnd};
use crate::sim::{self, Mode, SimMsg, Worker, plan};
use crate::util::{perr, perr_detail};

/// How long shutdown waits for a cancelled worker's thread to exit.
const STOP_BOUND: Duration = Duration::from_secs(2);

/// The simulation running.
pub(super) struct ActiveSim {
    worker: Worker,
    state: SimState,
    /// The plan as simulated, and whether it is the library's (else the session's copy).
    plan: PlanSpec,
    library: bool,
    /// The walk it is heard at, when it is a walk's points.
    walk: Option<WalkId>,
}

/// A fit kept for later runs on the same plan.
pub(super) struct FitMemo {
    plan: PlanSpec,
    walk: WalkId,
    scale: PerBand,
    absorption: SimAbsorption,
}

/// The simulation state of the control thread.
#[derive(Default)]
pub(super) struct Sims {
    active: Option<ActiveSim>,
    fit: Option<FitMemo>,
    next: u32,
}

fn refused(reason: SimRefusal) -> ProtoError {
    let msg = match &reason {
        SimRefusal::Outdoor => "the plan has no ceiling: outdoor plans are not simulated".into(),
        SimRefusal::UnknownMaterial { surface, name } => {
            format!("{surface:?} names {name:?}, which is not in the simulator's material table")
        }
        SimRefusal::Plan { reason } => format!("the plan cannot be simulated: {reason}"),
        SimRefusal::NoSpeaker { name } => format!("the plan has no speaker named {name:?}"),
        SimRefusal::Outside { name } => format!("{name} is not inside the room"),
        SimRefusal::Empty => "a simulation needs a source and a receiver".into(),
        SimRefusal::NotASweepWalk => "a live walk measured no decay: fit to a sweep walk".into(),
        SimRefusal::NoMeasuredT20 => "the walk measured no T20 in any octave band".into(),
        SimRefusal::Busy => "a simulation is running: cancel it first (sim.cancel)".into(),
    };
    perr_detail(ErrorCode::Invalid, msg, ErrorDetail::Sim { reason })
}

fn check_settings(s: &SimSettings) -> Result<(), ProtoError> {
    if s.image_order > SimSettings::MAX_IMAGE_ORDER {
        return Err(perr(
            ErrorCode::Invalid,
            format!("image order at most {}", SimSettings::MAX_IMAGE_ORDER),
        ));
    }
    if s.rays_per_band > SimSettings::MAX_RAYS {
        return Err(perr(
            ErrorCode::Invalid,
            format!("at most {} rays per band", SimSettings::MAX_RAYS),
        ));
    }
    if !(s.budget.0.is_finite() && s.budget.0 > 0.0 && s.budget.0 <= SimSettings::MAX_BUDGET_S) {
        return Err(perr(
            ErrorCode::Invalid,
            format!("the budget lies in 0…{} s", SimSettings::MAX_BUDGET_S),
        ));
    }
    Ok(())
}

impl Control {
    /// The plan `name`: the session's copy when it holds one (`false`), else the library's.
    fn sim_plan(&self, name: &str) -> Result<(PlanSpec, bool), ProtoError> {
        let st = self.store.state();
        st.walk_plans
            .iter()
            .find(|p| p.name == name)
            .map(|p| (p.clone(), false))
            .or_else(|| {
                st.plans
                    .iter()
                    .find(|p| p.name == name)
                    .map(|p| (p.clone(), true))
            })
            .ok_or_else(|| perr(ErrorCode::NotFound, format!("no plan named {name:?}")))
    }

    /// `sim.run`.
    pub(super) fn sim_run(&mut self, r: SimRequest) -> Result<ReplyBody, ProtoError> {
        if self.sims.active.is_some() {
            return Err(refused(SimRefusal::Busy));
        }
        check_settings(&r.settings)?;
        let (plan, library) = self.sim_plan(&r.plan)?;
        let prism = plan::prism(&plan).map_err(refused)?;
        let (points, walk) = match r.receivers {
            SimReceivers::Points { points } => (points, None),
            SimReceivers::Walk { walk } => {
                let w = self.walk(walk)?;
                if w.plan != r.plan {
                    return Err(perr(
                        ErrorCode::Invalid,
                        format!("walk {} walks plan {:?}, not {:?}", w.name, w.plan, r.plan),
                    ));
                }
                let points = w
                    .points
                    .iter()
                    .map(|p| ac2_proto::model::WalkPointSpec {
                        label: p.label.clone(),
                        at: p.at,
                    })
                    .collect();
                (points, Some(walk))
            }
        };
        if r.sources.is_empty() || points.is_empty() {
            return Err(refused(SimRefusal::Empty));
        }
        if points.len() > SimSettings::MAX_RECEIVERS
            || points.len() * r.sources.len() > SimSettings::MAX_RESULTS
        {
            return Err(perr(
                ErrorCode::Invalid,
                format!(
                    "at most {} receivers, {} over every source",
                    SimSettings::MAX_RECEIVERS,
                    SimSettings::MAX_RESULTS
                ),
            ));
        }
        let sources = r
            .sources
            .iter()
            .map(|n| plan::speaker(&plan, &prism, n).map(|at| (n.clone(), at)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(refused)?;
        let receivers = points
            .iter()
            .map(|p| plan::inside(&prism, &p.label, p.at).map(|at| (p.label.clone(), at)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(refused)?;
        let (scale, absorption) = match &self.sims.fit {
            Some(f) if f.plan == plan => (Some(f.scale), f.absorption.clone()),
            _ => (None, SimAbsorption::Assumed),
        };
        let job = Job {
            prism,
            sources,
            receivers,
            settings: r.settings,
        };
        let mode = Mode::Run {
            scale,
            absorption: absorption.clone(),
        };
        self.sim_start(job, mode, plan, library, walk, SimKind::Run, absorption)
    }

    /// `sim.fit`.
    pub(super) fn sim_fit(&mut self, r: SimFitRequest) -> Result<ReplyBody, ProtoError> {
        if self.sims.active.is_some() {
            return Err(refused(SimRefusal::Busy));
        }
        check_settings(&r.settings)?;
        let w = self.walk(r.walk)?.clone();
        if w.kind != WalkKind::Sweep {
            return Err(refused(SimRefusal::NotASweepWalk));
        }
        let room = w.room.as_ref().ok_or(refused(SimRefusal::NoMeasuredT20))?;
        let measured = sim::fit::measured_t20(room);
        if measured.iter().all(Option::is_none) {
            return Err(refused(SimRefusal::NoMeasuredT20));
        }
        let (plan, library) = self.sim_plan(&w.plan)?;
        let prism = plan::prism(&plan).map_err(refused)?;
        let source = plan::speaker(&plan, &prism, &r.source).map_err(refused)?;
        let receivers = w
            .points
            .iter()
            .filter(|p| matches!(p.state, PointState::Captured { .. }))
            .map(|p| plan::inside(&prism, &p.label, p.at).map(|at| (p.label.clone(), at)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(refused)?;
        if receivers.is_empty() {
            return Err(refused(SimRefusal::NoMeasuredT20));
        }
        let job = Job {
            prism,
            sources: vec![(r.source.clone(), source)],
            receivers,
            settings: r.settings,
        };
        let mode = Mode::Fit {
            walk: w.id,
            positions: room.positions,
            measured,
        };
        let kind = SimKind::Fit { walk: w.id };
        self.sim_start(
            job,
            mode,
            plan,
            library,
            Some(w.id),
            kind,
            SimAbsorption::Assumed,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn sim_start(
        &mut self,
        job: Job,
        mode: Mode,
        plan: PlanSpec,
        library: bool,
        walk: Option<WalkId>,
        kind: SimKind,
        absorption: SimAbsorption,
    ) -> Result<ReplyBody, ProtoError> {
        self.sims.next += 1;
        let id = SimId(self.sims.next);
        let state = SimState {
            id,
            plan: plan.name.clone(),
            kind,
            settings: job.settings,
            status: SimStatus::Running {
                stage: SimStage::ImageSources,
                done: 0,
                total: 0,
                source: 0,
                pass: u8::from(matches!(kind, SimKind::Fit { .. })),
            },
            bands: sim::run::bands(),
            absorption,
            sources: Vec::new(),
            schroeder: None,
            valid_above: None,
            eyring: Vec::new(),
            rays_per_band: 0,
            budget_exhausted: false,
            lost_rays: 0,
        };
        let worker = sim::spawn(id, job, mode, self.s.to_self.clone()).map_err(|e| {
            perr(
                ErrorCode::Internal,
                format!("cannot start the simulation: {e}"),
            )
        })?;
        self.commit(Change::Sim(state.clone()));
        self.sims.active = Some(ActiveSim {
            worker,
            state: state.clone(),
            plan,
            library,
            walk,
        });
        Ok(ReplyBody::Sim(state))
    }

    /// `sim.cancel`.
    pub(super) fn sim_cancel(&mut self) -> Result<ReplyBody, ProtoError> {
        match self.stop_sim(SimCancel::Operator) {
            Some(s) => Ok(ReplyBody::Sim(s)),
            None => Err(perr(ErrorCode::Refused, "no simulation is running")),
        }
    }

    /// Stops the running simulation for `why`: its state reads cancelled at once, and its
    /// worker's later messages are ignored.
    pub(super) fn stop_sim(&mut self, why: SimCancel) -> Option<SimState> {
        let a = self.sims.active.take()?;
        a.worker
            .cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let state = SimState {
            status: SimStatus::Cancelled { why },
            ..a.state
        };
        self.commit(Change::Sim(state.clone()));
        Some(state)
    }

    /// Why `change` stops the running simulation, if it does: its plan changed or went, or
    /// the walk it is heard at was deleted. A deleted walk also drops a fit to it.
    pub(super) fn sim_stopped_by(&mut self, change: &Change) -> Option<SimCancel> {
        if let Change::Walk(Patch::Deleted(id)) = change
            && self.sims.fit.as_ref().is_some_and(|f| f.walk == *id)
        {
            self.sims.fit = None;
        }
        let a = self.sims.active.as_ref()?;
        let plan_moved = |library: bool, p: &Patch<PlanSpec, String>| {
            library == a.library
                && match p {
                    Patch::Set(p) => p.name == a.plan.name && *p != a.plan,
                    Patch::Deleted(n) => *n == a.plan.name,
                }
        };
        match change {
            Change::Plan(p) if plan_moved(true, p) => Some(SimCancel::PlanChanged),
            Change::WalkPlan(p) if plan_moved(false, p) => Some(SimCancel::PlanChanged),
            Change::Walk(Patch::Deleted(id)) if a.walk == Some(*id) => Some(SimCancel::WalkDeleted),
            _ => None,
        }
    }

    /// A worker's message.
    pub(super) fn sim_message(&mut self, id: SimId, msg: SimMsg) {
        let Some(a) = self.sims.active.as_mut().filter(|a| a.state.id == id) else {
            return;
        };
        match msg {
            SimMsg::Progress(status) => {
                if a.state.status != status {
                    a.state.status = status;
                    let s = a.state.clone();
                    self.commit(Change::Sim(s));
                }
            }
            SimMsg::Ended(result) => {
                let Some(a) = self.sims.active.take() else {
                    return;
                };
                let state = match *result {
                    Ok(done) => {
                        if let (Some(scale), SimKind::Fit { walk }) = (done.fitted, a.state.kind) {
                            self.sims.fit = Some(FitMemo {
                                plan: a.plan.clone(),
                                walk,
                                scale,
                                absorption: done.absorption.clone(),
                            });
                        }
                        done.pass.into_state(&a.state, done.absorption)
                    }
                    Err(SimEnd::Cancelled) => SimState {
                        status: SimStatus::Cancelled {
                            why: SimCancel::Operator,
                        },
                        ..a.state
                    },
                    Err(SimEnd::Failed(msg)) => SimState {
                        status: SimStatus::Failed { msg },
                        ..a.state
                    },
                };
                self.commit(Change::Sim(state));
            }
        }
    }

    /// Shutdown: stops the worker and waits a little for its thread.
    pub(super) fn shutdown_sim(&mut self) {
        if let Some(a) = self.sims.active.take() {
            a.worker.stop(STOP_BOUND);
        }
    }
}
