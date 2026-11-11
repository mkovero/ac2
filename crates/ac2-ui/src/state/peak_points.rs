//! Peak points: the curve a legend click picked in a pane, whose peaks that
//! pane then shows (`docs/design/legend-peak-points.md`). A pick is a reading gesture,
//! like the cursor: this app's alone, unsaved, off the wire, never on the undo stack.

use super::*;
use ac2_scene::spectrum::peaks::{Peak, PeakInput, PeakTracker, find_peaks};

/// A pane's pick: the curve, the rig whose ids name it, and the daemon that numbered them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeakPick {
    pub rig: RigId,
    pub key: TraceKey,
    /// The daemon run whose ids these are: another run (restarted, swapped in) numbers
    /// afresh, so the same id may name another curve.
    pub incarnation: Option<ac2_proto::units::DaemonIncarnation>,
}

impl AppState {
    /// The curve pane `pane` shows peak points of: its pick, while the pane shows the
    /// pick's rig.
    pub fn peak_points_of(&self, pane: PaneId) -> Option<TraceKey> {
        let pick = self.pane_local.peak_points.get(&pane)?;
        let v = self.layout.view(pane)?;
        (self.pane_rig(v) == pick.rig).then_some(pick.key)
    }

    /// A legend click on `key` in pane `p` (of the rig in `rig`, known to it):
    /// picks it, or clears the pick when it was the picked one. A transfer pane's click
    /// while min / max is off leaves the pick alone: the pick only chooses min / max's
    /// curve, so there it would change nothing.
    pub(in crate::state) fn toggle_peak_pick(&mut self, p: PaneId, key: TraceKey) {
        if self.layout.kind(p) == PaneKind::Transfer && !self.view.tf.extremes {
            return;
        }
        let pick = PeakPick {
            rig: self.rig.id,
            key,
            incarnation: self.incarnation(),
        };
        if self.pane_local.peak_points.get(&p) == Some(&pick) {
            self.pane_local.peak_points.remove(&p);
            return;
        }
        self.pane_local.peak_points.insert(p, pick);
        // A stopped or stale curve may send no frame again: its tracker starts from the
        // newest frame kept, so its points show at once.
        if let TraceKey::Live(_) = key
            && let Some(d) = self.rig.data.clone()
        {
            let picked = self.peak_picked();
            self.rig.fold_peak_tracks(&d, &picked);
        }
        self.keep_pick_peaks();
    }

    /// The measurements whose live curve a pane showing the rig in `rig` picked: their
    /// peaks are tracked whatever their mode.
    pub(super) fn peak_picked(&self) -> BTreeSet<MeasId> {
        self.pane_local
            .peak_points
            .iter()
            .filter(|(p, pick)| pick.rig == self.rig.id && self.pane_ours(**p))
            .filter_map(|(_, pick)| match pick.key {
                TraceKey::Live(m) => Some(m),
                TraceKey::Stored(_) => None,
            })
            .collect()
    }

    /// Whether pane `pane` (of the rig in `rig`) has curve `key` as a member: the
    /// measurement's live curve drawn there as its own or compared, or the stored trace.
    /// Membership, not a trace in the scene: a curve before its first frame, or after a
    /// reconnect, is still on the pane.
    fn on_pane_member(&self, pane: PaneId, key: TraceKey) -> bool {
        let kind = self.layout.kind(pane);
        let shown = self.pane_meas(pane);
        match key {
            TraceKey::Live(id) => self.meas(id).is_some_and(|m| {
                self.live_on_pane(kind, m, shown) || self.live_compared_on(kind, m, pane)
            }),
            TraceKey::Stored(id) => self.trace_meta(id).is_ok_and(|t| {
                self.on_pane(kind, &t, shown) || self.trace_compared_on(kind, &t, pane)
            }),
        }
    }

    /// After every message: a pick stays only while its pane is a leaf of the layout,
    /// shows the pick's rig, and has the curve as a member of the same daemon run (kept
    /// while the rig's link is down: a reconnect to the same daemon leaves the curve on
    /// the pane). Other clients change
    /// membership too (hiding, regrouping, what a kind shows), and a closed pane's id may
    /// come back with other content after an undo; checked here, never at draw time, a
    /// pick dropped never comes back by itself.
    pub(super) fn prune_peak_points(&mut self) {
        if self.pane_local.peak_points.is_empty() {
            return;
        }
        let leaves = self.layout.panes();
        let picks: Vec<(PaneId, PeakPick)> = self
            .pane_local
            .peak_points
            .iter()
            .map(|(p, k)| (*p, *k))
            .collect();
        for (pane, pick) in picks {
            let keep = leaves.contains(&pane)
                && self
                    .layout
                    .view(pane)
                    .is_some_and(|v| self.pane_rig(v) == pick.rig)
                && self
                    .visit(pick.rig, |st| {
                        // A link down or not yet mirrored knows no membership: the pick
                        // waits for the mirror. A mirror of another daemon run drops it,
                        // also when the link went down in between.
                        let member = match st.daemon() {
                            None => true,
                            Some(_) => {
                                pick.incarnation == st.incarnation()
                                    && st.on_pane_member(pane, pick.key)
                            }
                        };
                        if member {
                            st.keep_pick_peaks();
                        }
                        member
                    })
                    .unwrap_or(false);
            if !keep {
                self.pane_local.peak_points.remove(&pane);
            }
        }
    }

    /// The run of the daemon the rig in `rig` mirrors.
    fn incarnation(&self) -> Option<ac2_proto::units::DaemonIncarnation> {
        self.rig.mirror.as_ref().and_then(|m| m.incarnation)
    }

    /// The peaks of each stored trace a pane picked on the rig in `rig`, found once: a
    /// stored trace is fixed until its data is replaced ([`RigState::keep_trace`]).
    fn keep_pick_peaks(&mut self) {
        let ids: Vec<TraceId> = self
            .pane_local
            .peak_points
            .values()
            .filter(|p| p.rig == self.rig.id)
            .filter_map(|p| match p.key {
                TraceKey::Stored(t) => Some(t),
                TraceKey::Live(_) => None,
            })
            .collect();
        for id in ids {
            if self.rig.folded.trace_peaks.contains_key(&id) {
                continue;
            }
            let Some(peaks) = self
                .rig
                .traces
                .get(&id)
                .and_then(|(t, g)| stored_peaks(t, g))
            else {
                continue;
            };
            self.rig.folded.trace_peaks.insert(id, peaks);
        }
    }
}

/// A stored spectrum's five peaks by the tracking rules, as a tracker started on it gives
/// them. A stored spectrum keeps no unresolved ranges or band metadata, so its peaks span
/// its whole range. `None`: not a spectrum or RTA.
fn stored_peaks(t: &ac2_proto::model::TraceData, g: &GridDef) -> Option<Vec<Peak>> {
    let narrow = match t.meta.kind {
        ac2_proto::model::TraceKind::Spectrum { .. } => true,
        ac2_proto::model::TraceKind::Rta { .. } => false,
        _ => return None,
    };
    let cols = ac2_scene::grid::columns(g);
    let mut tracker = PeakTracker::default();
    tracker.update(&find_peaks(&PeakInput {
        freqs: &cols.freqs,
        level: &t.mag_db,
        validity: None,
        unresolved: None,
        narrow,
    }));
    Some(tracker.peaks().to_vec())
}
