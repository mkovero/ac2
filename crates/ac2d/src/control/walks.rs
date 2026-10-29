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
    CaptureChain, CaptureContext, EqMark, MathConfig, MathDomain, MathExpr, MathReference,
    MeasConfig, MeasKind, MeasScope, Operand, PlanSpec, PointState, TraceOwner, TracePosition,
    WalkKind, WalkPoint, WalkPointSpec, WalkState,
};
use ac2_proto::units::{MeasId, Seconds, TraceId, WalkId, WallNs};
use ac2_proto::{ErrorCode, ProtoError, ReplyBody};

use super::Control;
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
        match meas.len() {
            0 => {
                return Err(perr(
                    ErrorCode::Invalid,
                    "a walk needs the measurement that captures at its points",
                ));
            }
            1 => {}
            _ => {
                return Err(perr(
                    ErrorCode::Unsupported,
                    "a walk captures with one measurement in this version; several mics per \
                     walk come with stage 3",
                ));
            }
        }
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
        let w = WalkState {
            id,
            name,
            plan,
            kind,
            meas,
            points: points
                .into_iter()
                .map(|p| WalkPoint {
                    label: p.label,
                    at: p.at,
                    mic: None,
                    state: PointState::Pending,
                })
                .collect(),
            current: Some(0),
            average: None,
        };
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
        let at_label = w
            .current
            .and_then(|i| w.points.get(usize::from(i)))
            .map(|p| p.label.clone());
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
                    mic: None,
                    state: PointState::Pending,
                },
            })
            .collect();
        w.name = name;
        w.points = new;
        let kept = at_label.and_then(|l| w.points.iter().position(|p| p.label == l));
        let moved = match kept {
            Some(i) => {
                w.current = Some(index(i));
                false
            }
            None => {
                w.current = next_pending(&w.points, None);
                w.current.is_some()
            }
        };
        self.commit(Change::Walk(Patch::Set(w.clone())));
        if moved {
            self.walk_moved(&w);
        }
        Ok(ReplyBody::Walk(w))
    }

    /// `walk.goto`: the mic moves to `point`.
    pub(super) fn walk_goto(&mut self, walk: WalkId, point: u16) -> Result<ReplyBody, ProtoError> {
        let mut w = self.walk(walk)?.clone();
        point_of(&w, point)?;
        w.current = Some(point);
        self.commit(Change::Walk(Patch::Set(w.clone())));
        self.walk_moved(&w);
        Ok(ReplyBody::Walk(w))
    }

    /// `walk.capture`: every walk measurement captured at the current point, then on to
    /// the next pending one.
    pub(super) fn walk_capture(&mut self, walk: WalkId) -> Result<ReplyBody, ProtoError> {
        let mut w = self.walk(walk)?.clone();
        let Some(at) = w.current else {
            return Err(perr(
                ErrorCode::Invalid,
                "every point of the walk is captured or skipped; redo one to capture again",
            ));
        };
        let p = point_of(&w, at)?.clone();
        if matches!(p.state, PointState::Captured { .. }) {
            return Err(perr(
                ErrorCode::Refused,
                format!("{} is captured; redo it to capture again", p.label),
            ));
        }
        let plan = w.plan.clone();
        let mut traces: Vec<TraceId> = Vec::with_capacity(w.meas.len());
        let mut chains: Vec<CaptureChain> = Vec::with_capacity(w.meas.len());
        let mut at_ns: Option<WallNs> = None;
        for &meas in &w.meas {
            let name = self.capture_name(&p.label, meas);
            let position = TracePosition {
                label: p.label.clone(),
                at: p.at,
                plan: plan.clone(),
                walk,
            };
            match self.trace_capture(meas, name, None, Some(position)) {
                Ok(ReplyBody::Trace(t)) => {
                    traces.push(t.id);
                    // The context is what the traces record, so the warning and the
                    // traces' own metadata never disagree.
                    at_ns.get_or_insert(t.created_at);
                    chains.push(CaptureChain {
                        mic: t.mic.clone(),
                        eq_in: t
                            .eq_in
                            .iter()
                            .map(|e| EqMark {
                                set: e.set,
                                rev: e.rev,
                            })
                            .collect(),
                    });
                }
                Ok(_) => {
                    self.drop_captures(&traces);
                    return Err(perr(ErrorCode::Internal, "a capture answered no trace"));
                }
                Err(e) => {
                    // One step: every walk measurement captures, or none does.
                    self.drop_captures(&traces);
                    return Err(e);
                }
            }
        }
        let i = usize::from(at);
        w.points[i].mic = w.meas.first().copied();
        let context = CaptureContext {
            at: at_ns.unwrap_or(WallNs(crate::util::wall_ns())),
            chains,
        };
        w.points[i].state = PointState::Captured { traces, context };
        w.current = next_pending(&w.points, Some(at));
        self.commit(Change::Walk(Patch::Set(w.clone())));
        let w = self.walk_average(walk)?;
        if w.current.is_some() {
            self.walk_moved(&w);
        }
        Ok(ReplyBody::Walk(w))
    }

    /// `walk.skip`: the current point left out, then on to the next pending one.
    pub(super) fn walk_skip(&mut self, walk: WalkId) -> Result<ReplyBody, ProtoError> {
        let mut w = self.walk(walk)?.clone();
        let Some(at) = w.current else {
            return Err(perr(
                ErrorCode::Invalid,
                "every point of the walk is captured or skipped",
            ));
        };
        point_of(&w, at)?;
        let i = usize::from(at);
        if let PointState::Captured { traces, .. } = &w.points[i].state {
            self.supersede(traces);
        }
        w.points[i].state = PointState::Skipped;
        w.current = next_pending(&w.points, Some(at));
        self.commit(Change::Walk(Patch::Set(w.clone())));
        let w = self.walk_average(walk)?;
        if w.current.is_some() {
            self.walk_moved(&w);
        }
        Ok(ReplyBody::Walk(w))
    }

    /// `walk.redo`: `point` pending again (its captures kept, superseded) and the mic
    /// there.
    pub(super) fn walk_redo(&mut self, walk: WalkId, point: u16) -> Result<ReplyBody, ProtoError> {
        let mut w = self.walk(walk)?.clone();
        point_of(&w, point)?;
        let i = usize::from(point);
        if let PointState::Captured { traces, .. } = &w.points[i].state {
            self.supersede(traces);
        }
        w.points[i].state = PointState::Pending;
        w.points[i].mic = None;
        w.current = Some(point);
        self.commit(Change::Walk(Patch::Set(w.clone())));
        let w = self.walk_average(walk)?;
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
            for p in &mut w.points {
                if let PointState::Captured { traces: t, .. } = &p.state
                    && !t.iter().all(|t| traces.contains(t))
                {
                    tracing::warn!(
                        "session load: walk {} point {} lost its capture",
                        w.id,
                        p.label
                    );
                    p.state = PointState::Pending;
                    p.mic = None;
                }
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
        if w.meas.len() != 1 {
            return Err(bad(format!(
                "walk {} captures with {} measurements; one in this version",
                w.id,
                w.meas.len()
            )));
        }
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
        if w.current.is_some_and(|c| usize::from(c) >= w.points.len()) {
            return Err(bad(format!("walk {} stands at no point", w.id)));
        }
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

/// The first pending point after `after` (from the start when `None`), wrapping round;
/// `None` when every point is captured or skipped.
fn next_pending(points: &[WalkPoint], after: Option<u16>) -> Option<u16> {
    let n = points.len();
    let start = after.map_or(0, |a| usize::from(a) + 1);
    (0..n)
        .map(|k| (start + k) % n)
        .find(|&i| matches!(points[i].state, PointState::Pending) && Some(index(i)) != after)
        .map(index)
}

fn index(i: usize) -> u16 {
    // A walk holds at most `MAX_POINTS` points.
    u16::try_from(i).unwrap_or(u16::MAX)
}

fn average_name(walk: &str) -> String {
    format!("{walk} · average")
}
