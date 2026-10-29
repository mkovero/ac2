//! A spatial walk (`docs/design/spatial-walk.md` §5): a plan, the measurements that capture
//! at its points (one per mic), the points with what became of each, and the average they
//! feed. Daemon state, so a second client sees it and it outlives an app restart.

use serde::{Deserialize, Serialize};

use super::MicState;
use crate::units::{EqSetId, MeasId, TraceId, WalkId, WallNs};

/// A walk through a plan's measurement positions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalkState {
    /// Id, unique within the session.
    pub id: WalkId,
    /// Its name ("Mains", "Delays").
    pub name: String,
    /// The plan it walks: the session's copy of it (`State::walk_plans`), so a library edit
    /// mid-walk changes nothing.
    pub plan: String,
    /// What a capture at a point is.
    pub kind: WalkKind,
    /// The measurements that capture at each point, one per mic.
    pub meas: Vec<MeasId>,
    /// The positions, in walking order.
    pub points: Vec<WalkPoint>,
    /// The point the mic is at (index into `points`); `None`: every point is captured or
    /// skipped.
    pub current: Option<u16>,
    /// The math channel averaging the captures, made at the second capture; `None` before.
    pub average: Option<MeasId>,
}

impl WalkState {
    /// Most points a walk holds: an average takes at most
    /// [`MathConfig::MAX_AVERAGE`](crate::model::MathConfig::MAX_AVERAGE) operands.
    pub const MAX_POINTS: usize = 64;
    /// Most characters of a walk's or a point's name.
    pub const MAX_NAME: usize = 64;

    /// The traces of the captured points, in point order: what the average averages.
    pub fn captured_traces(&self) -> Vec<TraceId> {
        self.points
            .iter()
            .flat_map(|p| match &p.state {
                PointState::Captured { traces, .. } => traces.as_slice(),
                PointState::Pending | PointState::Skipped => &[],
            })
            .copied()
            .collect()
    }
}

/// What a walk captures at each point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WalkKind {
    /// A settled live transfer of each walk measurement (`walk.capture`).
    LiveTransfer,
}

/// One position of a walk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalkPoint {
    /// Its name on the plan and in its captures' names ("P3").
    pub label: String,
    /// Plan x, plan y, ear height; metres.
    pub at: [f64; 3],
    /// The measurement whose mic stood here; `None` until the point is captured.
    pub mic: Option<MeasId>,
    /// What became of it.
    pub state: PointState,
}

/// What became of a walk's point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PointState {
    /// Not captured yet.
    Pending,
    /// Captured: one trace per walk measurement, in the walk's measurement order.
    Captured {
        /// The captures.
        traces: Vec<TraceId>,
        /// What they were taken under: a walk resumed another day, or after a change in
        /// the chain, averages captures that differ in more than position.
        context: CaptureContext,
    },
    /// Left out by the operator.
    Skipped,
}

/// When a point was captured and the facts of the measurement chain then that change a
/// transfer's result. A transfer is a ratio of two inputs, so the input calibration and the
/// generator level cancel out of it and are not recorded; the mic's correction curve and an
/// EQ the operator marked "in the console" do not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureContext {
    /// When (daemon clock).
    pub at: WallNs,
    /// One per walk measurement, in the walk's measurement order.
    pub chains: Vec<CaptureChain>,
}

/// One walk measurement's chain at a capture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureChain {
    /// The mic on its measurement input and the curve subtracted from its columns, as the
    /// capture's trace records it; `None`: the input names no mic.
    pub mic: Option<MicState>,
    /// The measurement's EQ sets marked "in the console", by set and the revision marked:
    /// what of them the capture contains.
    pub eq_in: Vec<EqMark>,
}

/// An EQ set as marked "in the console": its revision then names its sections exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EqMark {
    /// The set.
    pub set: EqSetId,
    /// Its revision when marked.
    pub rev: u32,
}

/// A point as `walk.new` and `walk.update` give it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalkPointSpec {
    /// Its name, unique in the walk ("P3").
    pub label: String,
    /// Plan x, plan y, ear height; metres.
    pub at: [f64; 3],
}

/// Where a trace was captured (a walk's capture).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TracePosition {
    /// The point's label.
    pub label: String,
    /// Plan x, plan y, ear height; metres.
    pub at: [f64; 3],
    /// The plan's name.
    pub plan: String,
    /// The walk.
    pub walk: WalkId,
}
