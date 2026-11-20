//! What a Dynamics pane shows of its curves (`docs/design/dynamics.md`, "App: the Dynamics
//! pane"): its view, the two curves Steps and Table compare, the picked step, the metric
//! and the two toggles. The step is kept as values, not indices, so a curve of another run
//! picks its own nearest step.

use serde::{Deserialize, Serialize};

use super::RigRef;
use crate::units::{Dbfs, Seconds, TraceId};

/// The Dynamics pane's views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DynamicsView {
    /// The level × length grid, one metric per cell.
    Steps,
    /// The picked step's gain-reduction (or THD) envelopes, onset above release.
    Envelopes,
    /// Release time and recovery against burst length.
    Release,
    /// The step table of A and B and their difference.
    Table,
}

impl DynamicsView {
    /// In the order `G` steps through them.
    pub const ALL: [DynamicsView; 4] = [
        DynamicsView::Steps,
        DynamicsView::Envelopes,
        DynamicsView::Release,
        DynamicsView::Table,
    ];
}

/// What a Steps cell shows: one column of the step table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DynamicsMetric {
    #[serde(rename = "gr_end")]
    GrEnd,
    #[serde(rename = "attack_tau")]
    AttackTau,
    #[serde(rename = "attack_2db")]
    Attack2Db,
    #[serde(rename = "settle_0_5db")]
    Settle05Db,
    #[serde(rename = "overshoot")]
    Overshoot,
    #[serde(rename = "release_tau")]
    ReleaseTau,
    #[serde(rename = "recovery_2db")]
    Recovery2Db,
    #[serde(rename = "recovery_0_5db")]
    Recovery05Db,
    #[serde(rename = "thd_attack")]
    ThdAttack,
    #[serde(rename = "thd_settled")]
    ThdSettled,
    #[serde(rename = "thd_release")]
    ThdRelease,
    #[serde(rename = "thump_start")]
    ThumpStart,
    #[serde(rename = "thump_end")]
    ThumpEnd,
    #[serde(rename = "drift")]
    Drift,
}

impl DynamicsMetric {
    /// In the order `K` steps through them: the table's columns left to right.
    pub const ALL: [DynamicsMetric; 14] = [
        DynamicsMetric::GrEnd,
        DynamicsMetric::AttackTau,
        DynamicsMetric::Attack2Db,
        DynamicsMetric::Settle05Db,
        DynamicsMetric::Overshoot,
        DynamicsMetric::ReleaseTau,
        DynamicsMetric::Recovery2Db,
        DynamicsMetric::Recovery05Db,
        DynamicsMetric::ThdAttack,
        DynamicsMetric::ThdSettled,
        DynamicsMetric::ThdRelease,
        DynamicsMetric::ThumpStart,
        DynamicsMetric::ThumpEnd,
        DynamicsMetric::Drift,
    ];
}

/// Curve A or B of a Dynamics pane: one measured input of one dynamics trace. Trace ids
/// are per daemon, so the curve names the rig whose id it is, as a pane entry does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DynamicsSide {
    /// The pane's default: A the first input of the first trace; B the trace's second
    /// input, else the first input of the second trace, else none.
    Default,
    /// None (B only: Steps and Table show A alone).
    Off,
    /// This curve.
    Curve {
        rig: RigRef,
        trace: TraceId,
        /// The measured input, zero-based.
        input: u16,
    },
}

/// The step shared by every view: a burst level and length.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicsStepPick {
    pub level: Dbfs,
    pub burst: Seconds,
}

/// A Dynamics pane's view and picks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicsPane {
    pub view: DynamicsView,
    pub a: DynamicsSide,
    pub b: DynamicsSide,
    /// `None`: the middle level, the longest burst of curve A.
    pub step: Option<DynamicsStepPick>,
    /// What a Steps cell shows.
    pub metric: DynamicsMetric,
    /// Envelopes draw THD (H2–H5 summed) instead of gain reduction.
    pub thd: bool,
    /// Release draws every level of curve A instead of the picked one of every curve.
    pub all_levels: bool,
}

impl Default for DynamicsPane {
    fn default() -> Self {
        Self {
            view: DynamicsView::Steps,
            a: DynamicsSide::Default,
            b: DynamicsSide::Default,
            step: None,
            metric: DynamicsMetric::GrEnd,
            thd: false,
            all_levels: false,
        }
    }
}

impl DynamicsPane {
    /// Why it cannot be drawn, if it cannot: A off (Steps and Table always show A), a
    /// picked step not finite or with a burst of no length.
    pub fn check(&self) -> Result<(), String> {
        if self.a == DynamicsSide::Off {
            return Err("a Dynamics pane's curve A is a curve or the default, not off".into());
        }
        if let Some(s) = self.step
            && !(s.level.0.is_finite() && s.burst.0.is_finite() && s.burst.0 > 0.0)
        {
            return Err(format!(
                "a Dynamics pane's step is a finite level and a burst above 0 s, not {} dBFS \
                 · {} s",
                s.level.0, s.burst.0
            ));
        }
        Ok(())
    }
}
