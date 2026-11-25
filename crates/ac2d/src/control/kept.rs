//! The result each stopped measurement keeps for `trace.capture`: the one its job formed as
//! it stopped, from the same state as its final frames, so the curve the views still show
//! can be stored. It goes when it no longer describes the measurement: the measurement
//! starts again, its averages would have started over had it been running (a reset, a new
//! delay, a new configuration), it is removed, or the audio closes.

use std::collections::HashMap;

use ac2_proto::frame::FrameData;
use ac2_proto::units::{MeasId, Rev};

use super::Captured;
use crate::jobs::SmoothingChange;

#[derive(Debug, Default)]
pub(super) struct Kept(HashMap<MeasId, Captured>);

impl Kept {
    /// What `meas`'s job left as it stopped; `None` (no result yet) keeps nothing.
    pub(super) fn keep(&mut self, meas: MeasId, c: Option<Captured>) {
        match c {
            Some(c) => {
                self.0.insert(meas, c);
            }
            None => self.forget(meas),
        }
    }

    pub(super) fn get(&self, meas: MeasId) -> Option<&Captured> {
        self.0.get(&meas)
    }

    pub(super) fn forget(&mut self, meas: MeasId) {
        self.0.remove(&meas);
    }

    pub(super) fn clear(&mut self) {
        self.0.clear();
    }

    /// A new display smoothing of a stopped measurement. A running transfer goes on
    /// averaging and its next result carries the new setting over the same unsmoothed
    /// columns, so the kept result takes it too. A spectrum's capture depends on its
    /// smoothing engine, which only a running job rebuilds, so its kept result goes.
    pub(super) fn smoothing(&mut self, meas: MeasId, change: &SmoothingChange, rev: Rev) {
        let Some(c) = self.0.get_mut(&meas) else {
            return;
        };
        match (&mut c.frame.data, change) {
            (FrameData::Tf(f), SmoothingChange::Transfer(s)) => {
                f.meta.smoothing = *s;
                c.frame.stamp.config_rev = rev;
            }
            _ => self.forget(meas),
        }
    }
}
