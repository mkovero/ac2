//! The 3D spectrograph's rows glide back from where one frame put them to where the next
//! does ([`ac2_scene::spectrograph_3d::Skyline::drawn`]), as the live spectrum curve glides
//! ([`super::glide`]): each frame moves them a slot or more at once, which drawn as received
//! reads as a stutter at the frame rate. The glide lasts about the time until the next
//! frame, so the rows move on steadily while frames come and stand still once they stop.

use super::*;

use std::time::Instant;

use ac2_scene::curve_ease;
use ac2_scene::spectrograph::SpectrographHistory;

/// A 3D spectrograph's rows on their way to where its history's newest slot puts them.
#[derive(Clone, Debug, PartialEq)]
pub struct RowGlide {
    /// The history's generation and newest slot it glides to; with any other it is over.
    pub generation: u64,
    pub to_slot: u64,
    /// Where the rows were drawn when the frame was taken, in history slots.
    pub from: f64,
    pub start: Instant,
    /// How long it lasts, s ([`curve_ease::Pacing`]).
    pub duration_s: f64,
}

impl RowGlide {
    /// Where the rows are drawn at `now`, in history slots, while the glide runs.
    fn at(&self, now: Instant) -> Option<f64> {
        let k = curve_ease::progress(
            now.saturating_duration_since(self.start).as_secs_f64(),
            self.duration_s,
        )?;
        Some(self.from + (self.to_slot as f64 - self.from) * k)
    }

    /// How many slots the rows of `h` are short of their places at `now`; 0 once the glide
    /// is over or `h` moved on without it.
    pub fn lag_slots(&self, h: &SpectrographHistory, now: Instant) -> f64 {
        if h.generation() != self.generation || h.newest_slot() != Some(self.to_slot) {
            return 0.0;
        }
        self.at(now)
            .map_or(0.0, |p| (self.to_slot as f64 - p).max(0.0))
    }
}

/// What a history was before a frame went in: its generation and newest slot.
pub(super) type HistoryMark = (u64, Option<u64>);

pub(super) fn mark(h: &SpectrographHistory) -> HistoryMark {
    (h.generation(), h.newest_slot())
}

impl RigState {
    /// Starts measurement `meas`'s row glide to its history's newest slot, which a frame
    /// captured at `wall_ns` and drained at `now` moved on from `before`. A history started
    /// over is drawn as it is.
    pub(super) fn keep_row_glide(
        &mut self,
        meas: MeasId,
        before: HistoryMark,
        now: Instant,
        wall_ns: u64,
    ) {
        let Some(h) = self.spectrographs.get(&meas) else {
            return;
        };
        let (generation, newest) = mark(h);
        let (Some(to), Some(old)) = (newest, before.1.filter(|_| before.0 == generation)) else {
            self.row_glide.remove(&meas);
            self.row_pacing.remove(&meas);
            return;
        };
        if to <= old {
            // Another frame in the newest slot: the rows stay where they go.
            return;
        }
        // Mid-glide, the rows move on from where they are drawn, so they never jump.
        let from = self
            .row_glide
            .get(&meas)
            .filter(|g| g.generation == generation && g.to_slot == old)
            .and_then(|g| g.at(now))
            .unwrap_or(old as f64);
        // Paced by slot advances, not frames: frames faster than slots do not move the rows.
        let duration_s = self
            .row_pacing
            .entry(meas)
            .or_default()
            .glide_s(now, wall_ns)
            .unwrap_or(curve_ease::MAX_S);
        self.row_glide.insert(
            meas,
            RowGlide {
                generation,
                to_slot: to,
                from,
                start: now,
                duration_s,
            },
        );
    }
}

impl AppState {
    /// How many history slots pane `pane`'s 3D rows are short of their places at `now`
    /// ([`ac2_scene::spectrograph_3d::Skyline::drawn`]); 0 with the glide off
    /// (Settings › Display, "jumps to each new frame") or at rest.
    pub fn row_lag(&self, pane: PaneId, now: Instant) -> f64 {
        if !self.prefs.spec_ease {
            return 0.0;
        }
        // A hidden measurement's view draws no rows, so nothing glides there.
        let Some(m) = self.pane_meas(pane).filter(|m| !self.meas_hidden(m)) else {
            return 0.0;
        };
        match (
            self.rig.row_glide.get(&m.id),
            self.rig.spectrographs.get(&m.id),
        ) {
            (Some(g), Some(h)) => g.lag_slots(h, now),
            _ => 0.0,
        }
    }

    /// Some 3D spectrograph on screen still glides at `now`: only then does the pacer run
    /// for it.
    pub fn rows_gliding(&self, now: Instant) -> bool {
        let three_d = self.three_d();
        self.prefs.spec_ease
            && !self.rig.row_glide.is_empty()
            && self.visible_panes().into_iter().any(|p| {
                self.layout.view(p).is_some_and(|v| v.shows_3d(three_d))
                    && self.row_lag(p, now) > 0.0
            })
    }
}
