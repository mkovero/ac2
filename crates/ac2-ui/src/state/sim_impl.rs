//! The room simulation from the palette (`docs/design/spatial-walk.md` §7): *Simulate room*,
//! *Fit simulation to walk*, *Coverage map*, *Cancel simulation*. The daemon simulates; these
//! pick the walk, its plan and the plan's speakers, as the CLI's `ac2 sim` does.

use super::*;
use ac2_proto::model::{
    MarkerKind, PlanSpec, SimFitRequest, SimMapRequest, SimReceivers, SimRequest, SimSettings,
    SimStatus, WalkKind, WalkState,
};
use ac2_proto::units::{Seconds, WalkId};

/// The speaker markers of `plan`, by name: the sources a simulation of it has.
pub(super) fn speakers(plan: &PlanSpec) -> Vec<String> {
    plan.markers
        .iter()
        .filter(|m| m.kind == MarkerKind::Speaker)
        .map(|m| m.name.clone())
        .collect()
}

impl AppState {
    /// The walk a simulation command acts on: the focused walk pane's, else the walk the
    /// selected trace was captured on.
    fn sim_walk(&self) -> Result<&WalkState, String> {
        if let Ok(w) = self.focused_walk() {
            return Ok(w);
        }
        self.selected_trace_meta()
            .and_then(|t| t.position.as_ref())
            .and_then(|p| self.walk(p.walk))
            .ok_or_else(|| {
                format!(
                    "focus a Spatial walk pane, or pick a trace captured on a walk ({} opens one)",
                    self.keys.reach(CommandId::Walks)
                )
            })
    }

    /// The walk, its plan and the plan's speakers, or why a simulation cannot start.
    fn sim_subject(&self) -> Result<(WalkId, String, Vec<String>), String> {
        let w = self.sim_walk()?;
        let plan = self
            .walk_plan(w)
            .ok_or_else(|| format!("{}: its plan {} is gone", w.name, w.plan))?;
        let sources = speakers(plan);
        if sources.is_empty() {
            return Err(format!(
                "plan {} has no speaker marker to simulate: place one in the plan editor",
                plan.name
            ));
        }
        Ok((w.id, plan.name.clone(), sources))
    }

    /// *Simulate room*: every speaker of the walk's plan at every point of the walk.
    pub(super) fn sim_run(&mut self, out: &mut Vec<Request>) {
        let (walk, plan, sources) = match self.sim_subject() {
            Ok(s) => s,
            Err(e) => return self.warn(e),
        };
        let points = self.walk(walk).map_or(0, |w| w.points.len());
        if points * sources.len() > SimSettings::MAX_RESULTS {
            return self.warn(format!(
                "{} speakers at {points} points is more than {} results: \
                 simulate from the CLI with fewer speakers (ac2 sim run --source)",
                sources.len(),
                SimSettings::MAX_RESULTS
            ));
        }
        let what = format!("simulate {plan}");
        self.call(
            out,
            Command::SimRun {
                request: SimRequest {
                    plan,
                    sources,
                    receivers: SimReceivers::Walk { walk },
                    settings: SimSettings::default(),
                },
            },
            what,
        );
    }

    /// *Fit simulation to walk*: the walk's speaker when its plan has one, else a question
    /// which.
    pub(super) fn sim_fit(&mut self, out: &mut Vec<Request>) {
        let (walk, plan, sources) = match self.sim_subject() {
            Ok(s) => s,
            Err(e) => return self.warn(e),
        };
        if self.walk(walk).is_some_and(|w| w.kind != WalkKind::Sweep) {
            return self
                .warn("a fit needs a sweep walk: its T20 is what the absorption is fitted to");
        }
        if let [one] = sources.as_slice() {
            let source = one.clone();
            return self.sim_fit_from(walk, source, out);
        }
        let choices = sources
            .iter()
            .map(|s| ac2_scene::meas_list::Choice {
                label: s.clone(),
                blocked: None,
            })
            .collect();
        self.overlay = Overlay::Choose(Box::new(ChoicePrompt {
            purpose: ChoicePurpose::SimSource { walk, sources },
            label: plan.clone(),
            title: "Fit simulation to walk".to_owned(),
            lines: vec![format!(
                "plan {plan} has several speakers: which one was measured?"
            )],
            choices,
            owners: Vec::new(),
            index: 0,
            hint: "↑/↓ pick · Enter fits · Esc closes".to_owned(),
        }));
    }

    /// `sim.fit` of `walk` measured from speaker `source`.
    pub(super) fn sim_fit_from(&mut self, walk: WalkId, source: String, out: &mut Vec<Request>) {
        let name = self
            .walk(walk)
            .map_or_else(|| "the walk".to_owned(), |w| w.name.clone());
        self.call(
            out,
            Command::SimFit {
                request: SimFitRequest {
                    walk,
                    source,
                    settings: SimSettings::default(),
                },
            },
            format!("fit the simulation to {name}"),
        );
    }

    /// *Coverage map*: every speaker of the walk's plan over its zones, at the defaults.
    pub(super) fn sim_map(&mut self, out: &mut Vec<Request>) {
        let (_, plan, sources) = match self.sim_subject() {
            Ok(s) => s,
            Err(e) => return self.warn(e),
        };
        let what = format!("map coverage of {plan}");
        self.call(
            out,
            Command::SimMap {
                request: SimMapRequest {
                    plan,
                    sources,
                    spacing: SimMapRequest::DEFAULT_SPACING_M,
                    band: None,
                    window: Seconds(SimMapRequest::DEFAULT_WINDOW_S),
                    image_order: SimMapRequest::DEFAULT_IMAGE_ORDER,
                },
            },
            what,
        );
    }

    /// The Display page's Simulation overlays line; remembered in the prefs.
    pub(super) fn toggle_sim_hints(&mut self) {
        self.view.sim_hints = !self.view.sim_hints;
        self.prefs.sim_hints = self.view.sim_hints;
        self.prefs_dirty = true;
        self.toast(format!(
            "simulation overlays {}",
            crate::settings::sim_hints_text(self.view.sim_hints)
        ));
    }

    /// *Cancel simulation*: stops the running one.
    pub(super) fn sim_cancel(&mut self, out: &mut Vec<Request>) {
        let running = self.daemon().is_some_and(|s| {
            [&s.sim, &s.sim_map]
                .into_iter()
                .flatten()
                .any(|x| matches!(x.status, SimStatus::Running { .. }))
        });
        if !running {
            return self.warn("no simulation is running");
        }
        self.call(out, Command::SimCancel, "cancel the simulation".to_owned());
    }
}
