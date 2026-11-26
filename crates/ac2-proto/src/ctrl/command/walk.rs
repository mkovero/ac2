//! Arguments of the commands that set up a spatial walk: its plan, measurements and points.

use serde::{Deserialize, Serialize};

use crate::model::{WalkKind, WalkPointSpec};
use crate::units::{MeasId, WalkId};

/// Arguments of `walk.new`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalkNewArgs {
    /// Walk name.
    pub name: String,
    /// Plan name.
    pub plan: String,
    /// What a capture is.
    pub kind: WalkKind,
    /// The measurements capturing at the points: a live walk's in mic order, a sweep
    /// walk's one.
    pub meas: Vec<MeasId>,
    /// The points, in walking order (at most [`crate::model::WalkState::MAX_POINTS`]).
    pub points: Vec<WalkPointSpec>,
}

/// Arguments of `walk.update`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalkUpdateArgs {
    /// The walk.
    pub walk: WalkId,
    /// New name.
    pub name: String,
    /// The points, in walking order.
    pub points: Vec<WalkPointSpec>,
}
