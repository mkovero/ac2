//! Spatial walks (`docs/design/spatial-walk.md` §5) and the plan library they walk.
//!
//! A walk names its measurements, its captures and its average; the owners checks refuse
//! deleting any of them while it does. Moving the mic (a walk's goto, the advance after a
//! capture or skip, a redo) restarts every walk measurement's averaging and turns its delay
//! tracking on, so the average at the new point holds nothing of the carry. A loaded walk's
//! measurements get the same on their first run after the load.

use std::collections::HashSet;
use std::path::Path;

use ac2_proto::event::{Change, Patch};
use ac2_proto::model::{
    CaptureContext, EqMark, MathConfig, MathDomain, MathExpr, MathReference, MeasConfig, MeasKind,
    MeasScope, Operand, PlanSpec, PointState, TraceOwner, TracePosition, WalkKind, WalkPoint,
    WalkPointSpec, WalkState,
};
use ac2_proto::units::{MeasId, Seconds, TraceId, WalkId};
use ac2_proto::{ErrorCode, ProtoError, ReplyBody};

use super::{Control, walk_steps};
use crate::jobs::JobCmd;
use crate::util::perr;

/// Most characters of a trace name (`ac2_traces::meta`).
const MAX_TRACE_NAME: usize = 128;

impl Control {
    // -- plan library ---------------------------------------------------------------------

    /// `plan.save`: the plan, checked, in the library (and its file, when the daemon keeps
    /// one).
    pub(super) fn plan_save(&mut self, plan: PlanSpec) -> Result<ReplyBody, ProtoError> {
        check_plan_name(&plan.name)?;
        ac2_room::plan::validate(&plan)
            .map_err(|e| perr(ErrorCode::Invalid, format!("plan {}: {e}", plan.name)))?;
        // Plan files are named by the plan: two names one case-insensitive file system
        // cannot tell apart are one plan.
        if let Some(other) = self
            .store
            .state()
            .plans
            .iter()
            .find(|p| p.name != plan.name && p.name.to_lowercase() == plan.name.to_lowercase())
        {
            return Err(perr(
                ErrorCode::Refused,
                format!("the library holds a plan named {}", other.name),
            ));
        }
        if let Some(dir) = &self.s.plan_dir {
            write_plan(dir, &plan).map_err(|e| {
                perr(
                    ErrorCode::Internal,
                    format!("plan {} not written: {e}", plan.name),
                )
            })?;
        }
        self.commit(Change::Plan(Patch::Set(plan.clone())));
        Ok(ReplyBody::Plan(plan))
    }

    /// `plan.delete`: out of the library; the session's copies stay.
    pub(super) fn plan_delete(&mut self, name: &str) -> Result<ReplyBody, ProtoError> {
        if !self.store.state().plans.iter().any(|p| p.name == name) {
            return Err(perr(
                ErrorCode::NotFound,
                format!("the library holds no plan named {name}"),
            ));
        }
        if let Some(dir) = &self.s.plan_dir {
            let path = dir.join(format!("{name}.json"));
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(perr(
                        ErrorCode::Internal,
                        format!("plan {name} not deleted: {e}"),
                    ));
                }
            }
        }
        let rev = self.commit(Change::Plan(Patch::Deleted(name.to_owned())));
        Ok(ReplyBody::Ack { rev })
    }

    // -- walks ------------------------------------------------------------------------------

    /// `walk.new`: the walk at its first point, its measurements restarted there.
    pub(super) fn walk_new(
        &mut self,
        name: String,
        plan: String,
        kind: WalkKind,
        meas: Vec<MeasId>,
        points: Vec<WalkPointSpec>,
    ) -> Result<ReplyBody, ProtoError> {
        check_walk_name(&name)?;
        if meas.is_empty() || meas.len() > WalkState::MAX_MICS {
            return Err(perr(
                ErrorCode::Invalid,
                format!(
                    "a walk captures with 1 … {} measurements, one per mic",
                    WalkState::MAX_MICS
                ),
            ));
        }
        if (1..meas.len()).any(|k| meas[..k].contains(&meas[k])) {
            return Err(perr(
                ErrorCode::Invalid,
                "a walk names each measurement once: one per mic",
            ));
        }
        let mut inputs: Vec<u16> = Vec::with_capacity(meas.len());
        for &id in &meas {
            let m = self.meas(id)?;
            if !matches!(m.config.kind, MeasKind::Transfer { .. }) {
                return Err(perr(
                    ErrorCode::Invalid,
                    format!(
                        "{} is not a transfer measurement: a live walk captures transfers",
                        m.config.name
                    ),
                ));
            }
            if m.config.scope != MeasScope::Session {
                return Err(perr(
                    ErrorCode::Invalid,
                    format!("{} is a rig meter", m.config.name),
                ));
            }
            // One mic stands at one point: two measurements of one input would capture the
            // same mic at two places.
            if let MeasKind::Transfer { config } = &m.config.kind {
                if inputs.contains(&config.measurement_input) {
                    return Err(perr(
                        ErrorCode::Invalid,
                        format!(
                            "{} measures an input another of the walk's measurements does: \
                             each mic needs its own input",
                            m.config.name
                        ),
                    ));
                }
                inputs.push(config.measurement_input);
            }
            if let Some(w) = self.walk_of(id) {
                return Err(perr(
                    ErrorCode::Refused,
                    format!("{} captures for the walk {} already", m.config.name, w.name),
                ));
            }
        }
        check_points(&points)?;
        let copy = self.store.state().walk_plans.iter().any(|p| p.name == plan);
        if !copy {
            let Some(p) = self
                .store
                .state()
                .plans
                .iter()
                .find(|p| p.name == plan)
                .cloned()
            else {
                return Err(perr(ErrorCode::NotFound, format!("no plan named {plan}")));
            };
            self.commit(Change::WalkPlan(Patch::Set(p)));
        }
        let id = WalkId(self.next_walk);
        self.next_walk += 1;
        let first = meas[0];
        let mut w = WalkState {
            id,
            name,
            plan,
            kind,
            points: points
                .into_iter()
                .map(|p| WalkPoint {
                    label: p.label,
                    at: p.at,
                    step: 0,
                    mic: first,
                    state: PointState::Pending,
                })
                .collect(),
            meas,
            step: Some(0),
            average: None,
        };
        walk_steps::regroup(&mut w.points, &w.meas);
        self.commit(Change::Walk(Patch::Set(w.clone())));
        self.walk_moved(&w);
        tracing::info!("walk {id} ({}) started", w.name);
        Ok(ReplyBody::Walk(w))
    }

    /// `walk.update`: renamed and its points edited, matched by label.
    pub(super) fn walk_update(
        &mut self,
        walk: WalkId,
        name: String,
        points: Vec<WalkPointSpec>,
    ) -> Result<ReplyBody, ProtoError> {
        check_walk_name(&name)?;
        check_points(&points)?;
        let mut w = self.walk(walk)?.clone();
        if let Some(gone) = w.points.iter().find(|p| {
            matches!(p.state, PointState::Captured { .. })
                && !points.iter().any(|s| s.label == p.label)
        }) {
            return Err(perr(
                ErrorCode::Refused,
                format!(
                    "{} is captured; skip or redo it before taking it out of the walk",
                    gone.label
                ),
            ));
        }
        let standing = |w: &WalkState| -> Vec<(String, MeasId)> {
            w.standing()
                .into_iter()
                .map(|i| (w.points[i].label.clone(), w.points[i].mic))
                .collect()
        };
        let before = standing(&w);
        let new: Vec<WalkPoint> = points
            .into_iter()
            .map(|s| match w.points.iter().find(|p| p.label == s.label) {
                // A capture was made where the point was: moving it on the plan would
                // misplace the capture.
                Some(p) if matches!(p.state, PointState::Captured { .. }) => p.clone(),
                Some(p) => WalkPoint {
                    at: s.at,
                    ..p.clone()
                },
                None => WalkPoint {
                    label: s.label,
                    at: s.at,
                    step: 0,
                    mic: w.meas[0],
                    state: PointState::Pending,
                },
            })
            .collect();
        w.name = name;
        w.points = new;
        walk_steps::regroup(&mut w.points, &w.meas);
        // The mics stay where they stand when a step still holds one of their points.
        w.step = before
            .iter()
            .find_map(|(l, _)| {
                w.points
                    .iter()
                    .find(|p| &p.label == l && matches!(p.state, PointState::Pending))
                    .map(|p| p.step)
            })
            .or_else(|| walk_steps::next_step(&w, None));
        let moved = standing(&w) != before && w.step.is_some();
        self.commit(Change::Walk(Patch::Set(w.clone())));
        if moved {
            self.walk_moved(&w);
        }
        Ok(ReplyBody::Walk(w))
    }

    /// `walk.goto`: the mics move to step `step`.
    pub(super) fn walk_goto(&mut self, walk: WalkId, step: u16) -> Result<ReplyBody, ProtoError> {
        let mut w = self.walk(walk)?.clone();
        if step >= w.steps() {
            return Err(perr(
                ErrorCode::NotFound,
                format!("the walk {} has no step {step}", w.name),
            ));
        }
        w.step = Some(step);
        self.commit(Change::Walk(Patch::Set(w.clone())));
        self.walk_moved(&w);
        Ok(ReplyBody::Walk(w))
    }

    /// `walk.capture`: every mic of the current step captured where it stands, then on to
    /// the next step with a pending point.
    pub(super) fn walk_capture(&mut self, walk: WalkId) -> Result<ReplyBody, ProtoError> {
        let mut w = self.walk(walk)?.clone();
        let Some(step) = w.step else {
            return Err(perr(
                ErrorCode::Invalid,
                "every point of the walk is captured or skipped; redo one to capture again",
            ));
        };
        let standing = w.standing();
        if standing.is_empty() {
            let labels: Vec<&str> = w
                .step_points(step)
                .into_iter()
                .map(|i| w.points[i].label.as_str())
                .collect();
            return Err(perr(
                ErrorCode::Refused,
                format!(
                    "{} captured or skipped; redo one to capture again",
                    labels.join(", ")
                ),
            ));
        }
        let mut done: Vec<(usize, TraceId, CaptureContext)> = Vec::with_capacity(standing.len());
        for i in standing {
            let p = w.points[i].clone();
            let name = self.capture_name(&p.label, p.mic);
            let position = TracePosition {
                label: p.label.clone(),
                at: p.at,
                plan: w.plan.clone(),
                walk,
            };
            let traces: Vec<TraceId> = done.iter().map(|d| d.1).collect();
            match self.trace_capture(p.mic, name, None, Some(position)) {
                Ok(ReplyBody::Trace(t)) => {
                    // The context is what the trace records, so the warning and the trace's
                    // own metadata never disagree.
                    let context = CaptureContext {
                        at: t.created_at,
                        mic: t.mic.clone(),
                        eq_in: t
                            .eq_in
                            .iter()
                            .map(|e| EqMark {
                                set: e.set,
                                rev: e.rev,
                            })
                            .collect(),
                    };
                    done.push((i, t.id, context));
                }
                Ok(_) => {
                    self.drop_captures(&traces);
                    return Err(perr(ErrorCode::Internal, "a capture answered no trace"));
                }
                Err(e) => {
                    // One step: every mic of it captures, or none does.
                    self.drop_captures(&traces);
                    return Err(e);
                }
            }
        }
        for (i, trace, context) in done {
            w.points[i].state = PointState::Captured { trace, context };
        }
        w.step = walk_steps::next_step(&w, Some(step));
        self.commit(Change::Walk(Patch::Set(w.clone())));
        let w = self.walk_average(walk)?;
        if w.step.is_some() {
            self.walk_moved(&w);
        }
        Ok(ReplyBody::Walk(w))
    }

    /// `walk.skip`: `point` left out; when that leaves its step, the current one, nothing
    /// pending, on to the next step with a pending point.
    pub(super) fn walk_skip(&mut self, walk: WalkId, point: u16) -> Result<ReplyBody, ProtoError> {
        let mut w = self.walk(walk)?.clone();
        point_of(&w, point)?;
        let i = usize::from(point);
        if matches!(w.points[i].state, PointState::Skipped) {
            return Err(perr(
                ErrorCode::Refused,
                format!("{} is skipped already", w.points[i].label),
            ));
        }
        if let PointState::Captured { trace, .. } = &w.points[i].state {
            self.supersede(&[*trace]);
        }
        w.points[i].state = PointState::Skipped;
        let moved = match w.step {
            Some(s) if !walk_steps::has_pending(&w, s) => {
                w.step = walk_steps::next_step(&w, Some(s));
                w.step.is_some()
            }
            _ => false,
        };
        self.commit(Change::Walk(Patch::Set(w.clone())));
        let w = self.walk_average(walk)?;
        if moved {
            self.walk_moved(&w);
        }
        Ok(ReplyBody::Walk(w))
    }

    /// `walk.redo`: `point` pending again (its capture kept, superseded) and the mics at its
    /// step.
    pub(super) fn walk_redo(&mut self, walk: WalkId, point: u16) -> Result<ReplyBody, ProtoError> {
        let mut w = self.walk(walk)?.clone();
        point_of(&w, point)?;
        let i = usize::from(point);
        if let PointState::Captured { trace, .. } = &w.points[i].state {
            self.supersede(&[*trace]);
        }
        walk_steps::repend(&mut w, i);
        w.step = Some(w.points[i].step);
        self.commit(Change::Walk(Patch::Set(w.clone())));
        let w = self.walk_average(walk)?;
        self.walk_moved(&w);
        Ok(ReplyBody::Walk(w))
    }

    /// `walk.swap`: the mics at `points` move round by one.
    pub(super) fn walk_swap(
        &mut self,
        walk: WalkId,
        points: &[u16],
    ) -> Result<ReplyBody, ProtoError> {
        let mut w = self.walk(walk)?.clone();
        walk_steps::swap(&mut w, points)?;
        self.commit(Change::Walk(Patch::Set(w.clone())));
        self.walk_moved(&w);
        Ok(ReplyBody::Walk(w))
    }

    /// `walk.delete`: the walk goes; its traces and its average stay.
    pub(super) fn walk_delete(&mut self, walk: WalkId) -> Result<ReplyBody, ProtoError> {
        let w = self.walk(walk)?.clone();
        for id in &w.meas {
            self.walk_resume.remove(id);
        }
        let mut rev = self.commit(Change::Walk(Patch::Deleted(walk)));
        if !self.store.state().walks.iter().any(|o| o.plan == w.plan) {
            rev = self.commit(Change::WalkPlan(Patch::Deleted(w.plan.clone())));
        }
        tracing::info!("walk {walk} ({}) deleted; its traces stay", w.name);
        Ok(ReplyBody::Ack { rev })
    }

    // -- invariants -------------------------------------------------------------------------

    /// The walk capturing with `meas`, if any.
    fn walk_of(&self, meas: MeasId) -> Option<&WalkState> {
        self.store
            .state()
            .walks
            .iter()
            .find(|w| w.meas.contains(&meas))
    }

    /// The walk naming measurement `meas` (capturing with it or averaging into it).
    pub(super) fn walk_naming_meas(&self, meas: MeasId) -> Option<&WalkState> {
        self.store
            .state()
            .walks
            .iter()
            .find(|w| w.meas.contains(&meas) || w.average == Some(meas))
    }

    /// The walk naming trace `trace` as a point's capture.
    pub(super) fn walk_naming_trace(&self, trace: TraceId) -> Option<&WalkState> {
        self.store
            .state()
            .walks
            .iter()
            .find(|w| w.captured_traces().contains(&trace))
    }

    /// Refuses deleting trace `trace` while a walk names it.
    pub(super) fn check_walk_trace_delete(&self, trace: TraceId) -> Result<(), ProtoError> {
        match self.walk_naming_trace(trace) {
            None => Ok(()),
            Some(w) => Err(perr(
                ErrorCode::Refused,
                format!(
                    "trace {trace} is a capture of the walk {}; redo or skip its point, or \
                     delete the walk, first",
                    w.name
                ),
            )),
        }
    }

    /// Refuses deleting measurement `meas`, or with it the traces `traces`, while a walk
    /// names them.
    pub(super) fn check_walk_meas_delete(
        &self,
        meas: MeasId,
        traces: &[TraceId],
    ) -> Result<(), ProtoError> {
        if let Some(w) = self.walk_naming_meas(meas) {
            return Err(perr(
                ErrorCode::Refused,
                format!(
                    "measurement {meas} belongs to the walk {}; delete the walk first",
                    w.name
                ),
            ));
        }
        traces
            .iter()
            .try_for_each(|&t| self.check_walk_trace_delete(t))
    }

    /// Refuses turning a walk's measurement into another kind: the walk captures transfers
    /// with it, and averages them into its average.
    pub(super) fn check_walk_meas_update(
        &self,
        meas: MeasId,
        new: &MeasKind,
    ) -> Result<(), ProtoError> {
        let Some(w) = self.walk_naming_meas(meas) else {
            return Ok(());
        };
        let keeps = if w.average == Some(meas) {
            matches!(new, MeasKind::Math { .. })
        } else {
            matches!(new, MeasKind::Transfer { .. })
        };
        if keeps {
            Ok(())
        } else {
            Err(perr(
                ErrorCode::Refused,
                format!(
                    "measurement {meas} belongs to the walk {}: it keeps its kind while the \
                     walk names it",
                    w.name
                ),
            ))
        }
    }

    // -- helpers ----------------------------------------------------------------------------

    fn walk(&self, id: WalkId) -> Result<&WalkState, ProtoError> {
        self.store
            .state()
            .walks
            .iter()
            .find(|w| w.id == id)
            .ok_or_else(|| perr(ErrorCode::NotFound, format!("no walk {id}")))
    }

    /// The mic is at a new point: each walk measurement's averaging restarts and its delay
    /// tracking follows the arrival from here.
    fn walk_moved(&mut self, w: &WalkState) {
        for &id in &w.meas {
            self.restart_at_point(id);
        }
    }

    /// One walk measurement starts over at the mic's point: its averaging restarts and its
    /// delay tracking is on.
    fn restart_at_point(&mut self, id: MeasId) {
        self.walk_resume.remove(&id);
        let Ok(m) = self.meas(id) else {
            return;
        };
        let mut m = m.clone();
        if let Some(j) = self.jobs.get(&id) {
            j.send(JobCmd::Reset);
            j.send(JobCmd::Track { enabled: true });
        }
        if let Some(d) = &mut m.delay
            && !d.tracking
        {
            d.tracking = true;
            self.commit(Change::Measurement(Patch::Set(m)));
        }
    }

    /// A loaded walk's measurements that run now and have not since the load restart as on
    /// a move: the mic has been placed again since the session was saved, so the saved
    /// delay and average describe a placement that is gone. Called wherever jobs start.
    pub(super) fn resume_walks(&mut self) {
        let running: Vec<MeasId> = self
            .walk_resume
            .iter()
            .copied()
            .filter(|id| self.jobs.contains_key(id))
            .collect();
        for id in running {
            self.restart_at_point(id);
        }
    }

    /// *P3 · Vilppu*: the point and the mic (its input's mic name, else the measurement's).
    fn capture_name(&self, label: &str, meas: MeasId) -> String {
        let input = match self.meas(meas).map(|m| &m.config.kind) {
            Ok(MeasKind::Transfer { config }) => Some(config.measurement_input),
            _ => None,
        };
        let mic = input
            .and_then(|i| self.cal_and_mic(i).1)
            .map(|m| m.name)
            .or_else(|| self.meas(meas).ok().map(|m| m.config.name.clone()))
            .unwrap_or_else(|| format!("measurement {meas}"));
        format!("{label} · {mic}")
            .chars()
            .take(MAX_TRACE_NAME)
            .collect()
    }

    /// Takes back the captures of a step that did not complete.
    fn drop_captures(&mut self, traces: &[TraceId]) {
        for &t in traces {
            self.traces.remove(t);
            self.commit(Change::Trace(Patch::Deleted(t)));
        }
    }

    /// Marks a redone or skipped point's captures superseded: kept, out of the average.
    fn supersede(&mut self, traces: &[TraceId]) {
        for &id in traces {
            if let Some(mut t) = self
                .store
                .state()
                .traces
                .iter()
                .find(|t| t.id == id)
                .cloned()
            {
                t.superseded = true;
                self.commit(Change::Trace(Patch::Set(t)));
            }
        }
    }

    /// The walk's average made to average its captures now: made at the second capture, its
    /// operands updated (the math channel restarts with them). Once made it stays while redos
    /// and skips leave fewer than two captures, so the panes showing it keep it; its frames
    /// then say NO AVERAGE (`FEW_OPERANDS`). Returns the walk as it is after.
    fn walk_average(&mut self, walk: WalkId) -> Result<WalkState, ProtoError> {
        let mut w = self.walk(walk)?.clone();
        let of: Vec<Operand> = w
            .captured_traces()
            .into_iter()
            .map(|trace| Operand::Trace { trace })
            .collect();
        let enough = of.len() >= MathConfig::MIN_AVERAGE;
        match (w.average, enough) {
            (None, false) => {}
            (None, true) => {
                let owner = TraceOwner::Meas {
                    meas: w.meas.first().copied().unwrap_or(MeasId(0)),
                };
                let config = MeasConfig {
                    name: average_name(&w.name),
                    kind: MeasKind::Math {
                        config: MathConfig::power_average(owner, MathDomain::Transfer, of),
                    },
                    scope: MeasScope::Session,
                };
                let m = self.meas_create(config)?;
                self.meas_start(m.id)?;
                w.average = Some(m.id);
                self.commit(Change::Walk(Patch::Set(w.clone())));
            }
            (Some(id), _) => {
                let m = self.meas(id)?.clone();
                let MeasKind::Math { config } = &m.config.kind else {
                    return Ok(w);
                };
                let MathExpr::Average { of: old, method } = &config.expr else {
                    return Ok(w);
                };
                if *old != of {
                    let mut config = config.clone();
                    let method = *method;
                    // With no capture left there is no operand to refer the phase to; the
                    // next capture becomes the reference again.
                    let reference = match (config.reference, of.first()) {
                        (MathReference::Operand { operand }, Some(first))
                            if !of.contains(&operand) =>
                        {
                            MathReference::Operand { operand: *first }
                        }
                        (MathReference::Operand { .. }, None) => MathReference::Fixed {
                            delay: Seconds(0.0),
                        },
                        (MathReference::Fixed { .. }, Some(first)) if old.is_empty() => {
                            MathReference::Operand { operand: *first }
                        }
                        (r, _) => r,
                    };
                    config.expr = MathExpr::Average { of, method };
                    config.reference = reference;
                    let next = MeasConfig {
                        kind: MeasKind::Math { config },
                        ..m.config.clone()
                    };
                    self.meas_update(id, m.config_rev, next)?;
                }
            }
        }
        Ok(w)
    }

    /// A loaded session's walks and plan copies, after its measurements and traces: a
    /// capture the file no longer holds makes its point pending again, so the walk never
    /// names what is gone.
    pub(super) fn load_walks(&mut self, walks: Vec<WalkState>, plans: Vec<PlanSpec>) {
        for p in plans {
            self.commit(Change::WalkPlan(Patch::Set(p)));
        }
        let traces: HashSet<TraceId> = self.store.state().traces.iter().map(|t| t.id).collect();
        for mut w in walks {
            self.next_walk = self.next_walk.max(w.id.0.saturating_add(1));
            let mut lost: Vec<usize> = Vec::new();
            for (i, p) in w.points.iter_mut().enumerate() {
                if let PointState::Captured { trace, .. } = &p.state
                    && !traces.contains(trace)
                {
                    tracing::warn!(
                        "session load: walk {} point {} lost its capture",
                        w.id,
                        p.label
                    );
                    lost.push(i);
                }
            }
            // Pending again for its mic, or for a free one of its step.
            for i in lost {
                walk_steps::repend(&mut w, i);
            }
            if w.step.is_none() {
                w.step = walk_steps::next_step(&w, None);
            }
            if w.average.is_some_and(|a| self.meas(a).is_err()) {
                w.average = None;
            }
            self.walk_resume.extend(w.meas.iter().copied());
            self.commit(Change::Walk(Patch::Set(w)));
        }
        self.resume_walks();
    }

    /// Clears the walks and plan copies before a load replaces them.
    pub(super) fn clear_walks(&mut self) {
        self.walk_resume.clear();
        let walks: Vec<WalkId> = self.store.state().walks.iter().map(|w| w.id).collect();
        for id in walks {
            self.commit(Change::Walk(Patch::Deleted(id)));
        }
        let plans: Vec<String> = self
            .store
            .state()
            .walk_plans
            .iter()
            .map(|p| p.name.clone())
            .collect();
        for name in plans {
            self.commit(Change::WalkPlan(Patch::Deleted(name)));
        }
    }
}

/// Checks a loaded session's walks before anything changes: every walk names measurements
/// the session holds and a plan copy it holds that validates, under unique ids.
pub(super) fn check_walks(
    walks: &[WalkState],
    plans: &[PlanSpec],
    meas: &[MeasId],
) -> Result<(), ProtoError> {
    let bad = |m: String| perr(ErrorCode::Invalid, m);
    for p in plans {
        ac2_room::plan::validate(p).map_err(|e| bad(format!("plan {}: {e}", p.name)))?;
        if plans.iter().filter(|o| o.name == p.name).count() > 1 {
            return Err(bad(format!("plan {} appears twice in the session", p.name)));
        }
    }
    for w in walks {
        if walks.iter().filter(|o| o.id == w.id).count() > 1 {
            return Err(bad(format!("walk {} appears twice in the session", w.id)));
        }
        if !plans.iter().any(|p| p.name == w.plan) {
            return Err(bad(format!(
                "walk {} walks plan {}, which the session does not hold",
                w.id, w.plan
            )));
        }
        if let Some(m) = w.meas.iter().chain(&w.average).find(|m| !meas.contains(m)) {
            return Err(bad(format!(
                "walk {} names measurement {m}, which the session does not hold",
                w.id
            )));
        }
        walk_steps::check(w)?;
        check_walk_name(&w.name)?;
        let specs: Vec<WalkPointSpec> = w
            .points
            .iter()
            .map(|p| WalkPointSpec {
                label: p.label.clone(),
                at: p.at,
            })
            .collect();
        check_points(&specs)?;
    }
    Ok(())
}

/// The plan library in `dir`: every `<name>.json` that parses, is named as its file and
/// validates. One that does not is left on disk and logged.
pub(crate) fn load_plans(dir: &Path) -> Vec<PlanSpec> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut plans: Vec<PlanSpec> = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let read = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|b| serde_json::from_slice::<PlanSpec>(&b).map_err(|e| e.to_string()))
            .and_then(|p| {
                let stem = path.file_stem().and_then(|s| s.to_str());
                if stem == Some(p.name.as_str()) {
                    Ok(p)
                } else {
                    Err(format!("it holds the plan {}", p.name))
                }
            })
            .and_then(|p| match ac2_room::plan::validate(&p) {
                Ok(()) => Ok(p),
                Err(e) => Err(e.to_string()),
            });
        match read {
            Ok(p) => plans.push(p),
            Err(e) => tracing::warn!("plan file {} skipped: {e}", path.display()),
        }
    }
    plans.sort_by(|a, b| a.name.cmp(&b.name));
    plans
}

fn write_plan(dir: &Path, plan: &PlanSpec) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let bytes = serde_json::to_vec_pretty(plan).map_err(std::io::Error::other)?;
    ac2_paths::write_atomic(&dir.join(format!("{}.json", plan.name)), &bytes)
}

fn check_plan_name(name: &str) -> Result<(), ProtoError> {
    ac2_traces::session::validate_name(name).map_err(|_| {
        perr(
            ErrorCode::Invalid,
            format!(
                "{name:?} is no plan name: letters, digits, space, '-', '_' and '.', not \
                 starting with '.'"
            ),
        )
    })
}

fn check_walk_name(name: &str) -> Result<(), ProtoError> {
    if name.trim().is_empty() || name.trim() != name {
        return Err(perr(
            ErrorCode::Invalid,
            "a walk needs a name, without leading or trailing spaces",
        ));
    }
    if name.chars().count() > WalkState::MAX_NAME {
        return Err(perr(
            ErrorCode::Invalid,
            format!("walk names are at most {} characters", WalkState::MAX_NAME),
        ));
    }
    Ok(())
}

/// Points a walk can stand at: one to [`WalkState::MAX_POINTS`] (an average takes at most
/// that many operands), each named uniquely, on or above the floor.
fn check_points(points: &[WalkPointSpec]) -> Result<(), ProtoError> {
    let bad = |m: String| perr(ErrorCode::Invalid, m);
    if points.is_empty() || points.len() > WalkState::MAX_POINTS {
        return Err(bad(format!(
            "a walk has 1 … {} points",
            WalkState::MAX_POINTS
        )));
    }
    for (i, p) in points.iter().enumerate() {
        if p.label.trim().is_empty()
            || p.label.trim() != p.label
            || p.label.chars().count() > WalkState::MAX_NAME
        {
            return Err(bad(format!(
                "point {i}: a label of 1 … {} characters, without leading or trailing spaces",
                WalkState::MAX_NAME
            )));
        }
        if points[..i].iter().any(|o| o.label == p.label) {
            return Err(bad(format!("point label {} appears twice", p.label)));
        }
        if !p.at.iter().all(|v| v.is_finite()) || p.at[2] < 0.0 {
            return Err(bad(format!(
                "point {}: a finite position on or above the floor",
                p.label
            )));
        }
    }
    Ok(())
}

fn point_of(w: &WalkState, point: u16) -> Result<&WalkPoint, ProtoError> {
    w.points.get(usize::from(point)).ok_or_else(|| {
        perr(
            ErrorCode::NotFound,
            format!("the walk {} has no point {point}", w.name),
        )
    })
}

fn average_name(walk: &str) -> String {
    format!("{walk} · average")
}
