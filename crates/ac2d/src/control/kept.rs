//! The result each stopped measurement keeps for `trace.capture`: the one its job formed as
//! it stopped, from the same state as its final frames, so the curve the views still show
//! can be stored. It goes when it no longer describes the measurement: the measurement
//! starts again, its averages would have started over had it been running (a reset, a new
//! delay, a new configuration), it is removed, or the audio closes.

use std::collections::HashMap;

use ac2_proto::frame::{FrameData, MathState};
use ac2_proto::model::{CalState, EqSnapshot, MeasKind, Measurement, MicState, Operand};
use ac2_proto::units::{MeasId, Rev, WallNs};

use super::{Captured, Control};
use crate::jobs::SmoothingChange;

/// What a stopped measurement's result was measured with, taken as it stopped: a capture
/// after the stop says what the curve is of, not what the inputs, marks or names became.
#[derive(Clone, Debug)]
pub(super) struct AtStop {
    pub(super) cal: CalState,
    pub(super) mic: Option<MicState>,
    pub(super) eq_in: Vec<EqSnapshot>,
    /// A math trace's operands, named as they were.
    pub(super) names: Vec<(Operand, String)>,
    /// When it stopped: the capture's time.
    pub(super) wall: WallNs,
}

#[derive(Clone, Debug)]
pub(super) struct KeptResult {
    pub(super) captured: Captured,
    pub(super) at: AtStop,
}

#[derive(Debug, Default)]
pub(super) struct Kept(HashMap<MeasId, KeptResult>);

/// What a math trace's frame says it combined.
pub(super) fn math_state(d: &FrameData) -> Option<&MathState> {
    match d {
        FrameData::Tf(f) => f.meta.math.as_deref(),
        FrameData::Spec(f) => f.meta.math.as_deref(),
        FrameData::Rta(f) => f.meta.math.as_deref(),
        _ => None,
    }
}

impl Control {
    /// What `m`'s result `c` is measured with, as of now (its stop).
    pub(super) fn at_stop(&self, m: &Measurement, c: &Captured) -> AtStop {
        let input = match &m.config.kind {
            MeasKind::Transfer { config } => Some(config.measurement_input),
            MeasKind::Spectrum { config } => Some(config.input),
            MeasKind::Rta { config } => Some(config.input),
            _ => None,
        };
        let (cal, mic) = input.map_or((CalState::Uncalibrated, None), |i| self.cal_and_mic(i));
        let (eq_in, names) = match &m.config.kind {
            MeasKind::Math { config } => (
                self.eq_in_math(config),
                math_state(&c.frame.data)
                    .map(|s| {
                        s.operands
                            .iter()
                            .map(|o| (o.operand, self.operand_name(o.operand)))
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            _ => (self.eq_in_live(m.id), Vec::new()),
        };
        AtStop {
            cal,
            mic,
            eq_in,
            names,
            wall: WallNs(crate::util::wall_ns()),
        }
    }
}

impl Kept {
    /// What `meas`'s job left as it stopped; `None` (no result yet) keeps nothing.
    pub(super) fn keep(&mut self, meas: MeasId, c: Option<(Captured, AtStop)>) {
        match c {
            Some((captured, at)) => {
                self.0.insert(meas, KeptResult { captured, at });
            }
            None => self.forget(meas),
        }
    }

    pub(super) fn get(&self, meas: MeasId) -> Option<&KeptResult> {
        self.0.get(&meas)
    }

    pub(super) fn forget(&mut self, meas: MeasId) {
        self.0.remove(&meas);
    }

    pub(super) fn clear(&mut self) {
        self.0.clear();
    }

    /// A new display smoothing of a stopped measurement; whether a kept result took it.
    /// A transfer's result holds its columns unsmoothed and smoothing is how they are
    /// shown and stored (a display edit of the trace), so relabelling the kept result is
    /// what a running job would do with its next frame: the same columns, the new setting,
    /// the new rev. A spectrum's capture depends on its smoothing engine, which only a
    /// running job rebuilds, so its kept result goes.
    pub(super) fn smoothing(&mut self, meas: MeasId, change: &SmoothingChange, rev: Rev) -> bool {
        let Some(c) = self.0.get_mut(&meas) else {
            return false;
        };
        let c = &mut c.captured;
        match (&mut c.frame.data, change) {
            (FrameData::Tf(f), SmoothingChange::Transfer(s)) => {
                f.meta.smoothing = *s;
                c.frame.stamp.config_rev = rev;
                true
            }
            _ => {
                self.forget(meas);
                false
            }
        }
    }
}
