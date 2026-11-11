//! The ids the control thread hands out: each kind counts on its own, and a number is
//! read and stepped in one place so that no two things of a kind share one.

use ac2_proto::units::{MeasId, SweepId, WalkId};

/// The next id of each kind.
pub(super) struct Ids {
    fanout: u64,
    meas: u32,
    token: u64,
    sweep: u32,
    walk: u32,
}

impl Ids {
    /// Counters at their start; measurements number from `first_meas`, past the ids the
    /// rig meters hold.
    pub(super) fn new(first_meas: u32) -> Self {
        Self {
            fanout: 1,
            meas: first_meas,
            token: 1,
            sweep: 1,
            walk: 1,
        }
    }

    /// An id for a consumer of the fan-out.
    pub(super) fn fanout(&mut self) -> u64 {
        let id = self.fanout;
        self.fanout += 1;
        id
    }

    /// A token for work handed to another thread, matched when it reports back.
    pub(super) fn token(&mut self) -> u64 {
        let t = self.token;
        self.token += 1;
        t
    }

    /// An id for a new measurement.
    pub(super) fn meas(&mut self) -> MeasId {
        let id = MeasId(self.meas);
        self.meas += 1;
        id
    }

    /// A candidate id for a loaded measurement being renumbered. The caller may pass over
    /// candidates, so the counter stops at the top of its range rather than overflow.
    pub(super) fn meas_candidate(&mut self) -> MeasId {
        let id = MeasId(self.meas);
        self.meas = self.meas.saturating_add(1);
        id
    }

    /// Moves the measurement counter past `id`, an id that came with loaded data.
    pub(super) fn meas_past(&mut self, id: MeasId) {
        self.meas = self.meas.max(id.0.saturating_add(1));
    }

    /// An id for a new sweep run.
    pub(super) fn sweep(&mut self) -> SweepId {
        let id = SweepId(self.sweep);
        self.sweep += 1;
        id
    }

    /// An id for a new walk.
    pub(super) fn walk(&mut self) -> WalkId {
        let id = WalkId(self.walk);
        self.walk += 1;
        id
    }

    /// Moves the walk counter past `id`, an id that came with loaded data.
    pub(super) fn walk_past(&mut self, id: WalkId) {
        self.walk = self.walk.max(id.0.saturating_add(1));
    }
}
