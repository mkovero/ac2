//! Sweep walks (`docs/design/spatial-walk.md` §5, §8, §11): a run of the walk's sweep
//! measurement at a step records every mic standing there, one trace each, and is the step's
//! capture; the walk's room parameters are the positions' means.
//!
//! The run is an ordinary `sweep.run`: it plays under the stimulus lease, armed, at the
//! measurement's level, refused above the global maximum, as every sweep does. What the walk
//! adds is planned when the run starts (which inputs it records, where each stands, what its
//! trace is named) and taken up when the run is done. A run that is refused, fails or is
//! aborted leaves the points pending; the walk never moves while its sweep runs.

use ac2_core::room::positions::spread;
use ac2_proto::event::{Change, Patch};
use ac2_proto::model::{
    PointState, PositionMean, RoomAcoustics, RoomBand, RoomValue, TracePosition, WalkKind,
    WalkRoom, WalkRoomBand, WalkState,
};
use ac2_proto::units::{Hz, MeasId, TraceId, WalkId};
use ac2_proto::{ErrorCode, ProtoError};

use super::walks::capture_context;
use super::{Control, walk_steps};
use crate::util::perr;

/// Where a walk's sweep run records, fixed when it starts.
#[derive(Debug, Clone)]
pub(super) struct WalkSweepAt {
    pub(super) walk: WalkId,
    pub(super) step: u16,
    /// The point each of the run's inputs stands at, in the run's input order.
    pub(super) points: Vec<usize>,
}

/// What a run of a sweep walk's measurement records: the standing mics' inputs, each one's
/// trace name and position, and where.
pub(super) struct WalkSweepPlan {
    pub(super) at: WalkSweepAt,
    pub(super) inputs: Vec<u16>,
    pub(super) names: Vec<String>,
    pub(super) positions: Vec<TracePosition>,
}

impl Control {
    /// The walk the sweep in progress records for, if any.
    pub(super) fn walk_sweeping(&self) -> Option<WalkId> {
        self.sweep.as_ref()?.walk.as_ref().map(|a| a.walk)
    }

    /// For a run of sweep measurement `meas`: when a sweep walk records with it and its mics
    /// stand at a step, what the run records there. `None`: an ordinary run (no walk, or
    /// every point of it is captured or skipped).
    pub(super) fn walk_sweep_plan(&self, meas: MeasId) -> Option<WalkSweepPlan> {
        let w = self.walk_of(meas).filter(|w| w.kind == WalkKind::Sweep)?;
        let step = w.step?;
        let standing = w.standing();
        if standing.is_empty() {
            return None;
        }
        let names = standing
            .iter()
            .map(|&i| self.capture_name(&w.points[i].label, w.points[i].mic))
            .collect();
        let positions = standing
            .iter()
            .map(|&i| TracePosition {
                label: w.points[i].label.clone(),
                at: w.points[i].at,
                plan: w.plan.clone(),
                walk: w.id,
            })
            .collect();
        Some(WalkSweepPlan {
            inputs: standing.iter().map(|&i| w.points[i].mic.input).collect(),
            names,
            positions,
            at: WalkSweepAt {
                walk: w.id,
                step,
                points: standing,
            },
        })
    }

    /// A walk's run is done with `traces` (in its input order): each standing point is
    /// captured by its trace and the walk goes on to the next step with a pending point.
    /// A walk deleted meanwhile leaves the traces as ordinary runs.
    pub(super) fn walk_swept(&mut self, at: &WalkSweepAt, traces: &[TraceId]) {
        let Ok(w) = self.walk(at.walk) else {
            return;
        };
        let mut w = w.clone();
        for (&i, &trace) in at.points.iter().zip(traces) {
            let Some(meta) = self.store.state().traces.iter().find(|t| t.id == trace) else {
                continue;
            };
            let context = capture_context(meta);
            if let Some(p) = w.points.get_mut(i)
                && matches!(p.state, PointState::Pending)
            {
                p.state = PointState::Captured { trace, context };
            }
        }
        w.step = walk_steps::next_step(&w, Some(at.step));
        let id = w.id;
        tracing::info!(
            "walk {id} ({}): sweep captured at step {}",
            w.name,
            at.step + 1
        );
        self.commit(Change::Walk(Patch::Set(w)));
        if let Err(e) = self.walk_results(id) {
            tracing::warn!("walk {id}: average not updated: {}", e.msg);
        }
    }

    /// The walk's average and, for a sweep walk, its room parameters made to follow its
    /// captures now. Returns the walk as it is after.
    pub(super) fn walk_results(&mut self, walk: WalkId) -> Result<WalkState, ProtoError> {
        let mut w = self.walk_average(walk)?;
        let room = self.walk_room(&w);
        if room != w.room {
            w.room = room;
            self.commit(Change::Walk(Patch::Set(w.clone())));
        }
        Ok(w)
    }

    /// A sweep walk's room parameters over its captured positions: per band, the mean, least
    /// and greatest of each parameter over the positions that give it. `None` for a live walk,
    /// or while no capture carries room parameters.
    pub(super) fn walk_room(&self, w: &WalkState) -> Option<WalkRoom> {
        if w.kind != WalkKind::Sweep {
            return None;
        }
        let rooms: Vec<&RoomAcoustics> = w
            .captured_traces()
            .into_iter()
            .filter_map(|t| self.traces.sweep(t)?.room.as_ref())
            .collect();
        walk_room(&rooms)
    }
}

/// The room parameters over `rooms` (one per position, a single source).
pub(super) fn walk_room(rooms: &[&RoomAcoustics]) -> Option<WalkRoom> {
    if rooms.is_empty() {
        return None;
    }
    let broadband: Vec<Option<&RoomBand>> = rooms.iter().map(|r| Some(&r.broadband)).collect();
    let bands = |of: fn(&RoomAcoustics) -> &[RoomBand]| -> Vec<WalkRoomBand> {
        let mut centres: Vec<f64> = Vec::new();
        for b in rooms.iter().flat_map(|r| of(r)) {
            if let Some(c) = b.centre
                && !centres.iter().any(|&o| same_band(o, c.0))
            {
                centres.push(c.0);
            }
        }
        centres.sort_by(f64::total_cmp);
        centres
            .into_iter()
            .map(|c| {
                let at: Vec<Option<&RoomBand>> = rooms
                    .iter()
                    .map(|r| {
                        of(r)
                            .iter()
                            .find(|b| b.centre.is_some_and(|o| same_band(o.0, c)))
                    })
                    .collect();
                band(Some(Hz(c)), &at)
            })
            .collect()
    };
    Some(WalkRoom {
        positions: u16::try_from(rooms.len()).unwrap_or(u16::MAX),
        sources: 1,
        broadband: band(None, &broadband),
        octave: bands(|r| &r.octave),
        third: bands(|r| &r.third),
    })
}

/// Two positions' bands are one band when their mid-band frequencies agree to well inside
/// the third-octave spacing (a factor 1.26).
fn same_band(a: f64, b: f64) -> bool {
    (a / b - 1.0).abs() < 0.01
}

/// One band over the positions: `at` holds each position's band, `None` where its analysis
/// has no such band (a position swept over a narrower range).
fn band(centre: Option<Hz>, at: &[Option<&RoomBand>]) -> WalkRoomBand {
    let over = |f: fn(&RoomBand) -> RoomValue| -> Option<PositionMean> {
        let values: Vec<Option<f64>> = at.iter().map(|b| b.and_then(|b| f(b).value())).collect();
        spread(&values).map(|s| PositionMean {
            mean: s.mean,
            min: s.min,
            max: s.max,
            n: u16::try_from(s.n).unwrap_or(u16::MAX),
        })
    };
    WalkRoomBand {
        centre,
        edt: over(|b| b.edt),
        t20: over(|b| b.t20),
        c80: over(|b| b.c80),
        d50: over(|b| b.d50),
    }
}

/// Refuses `walk.capture` on a sweep walk: its capture is a run of its sweep measurement.
pub(super) fn sweep_capture_refusal(w: &WalkState) -> ProtoError {
    perr(
        ErrorCode::Refused,
        format!(
            "the walk {} captures by running its sweep (sweep.run under the stimulus lease): \
             the run records every mic where it stands",
            w.name
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::RoomRefusal;
    use ac2_proto::units::Seconds;

    fn value(v: Option<f64>) -> RoomValue {
        match v {
            Some(value) => RoomValue::Value { value },
            None => RoomValue::Refused {
                reason: RoomRefusal::NoDecay,
            },
        }
    }

    fn room_band(centre: Option<f64>, t20: Option<f64>, c80: f64) -> RoomBand {
        RoomBand {
            centre: centre.map(Hz),
            onset: Seconds(0.0),
            truncation: Seconds(1.0),
            decay_range: None,
            edt: value(Some(1.0)),
            t20: value(t20),
            t30: value(None),
            c50: value(None),
            c80: value(Some(c80)),
            d50: value(Some(0.5)),
            curvature: None,
        }
    }

    fn room(t20_125: Option<f64>, c80: f64) -> RoomAcoustics {
        RoomAcoustics {
            broadband: room_band(None, Some(1.0), c80),
            octave: vec![
                room_band(Some(125.0), t20_125, c80),
                room_band(Some(250.0), Some(0.9), c80),
            ],
            third: Vec::new(),
            span_end: Seconds(2.0),
        }
    }

    #[test]
    fn per_band_mean_and_range_leave_out_a_position_that_does_not_give_it() {
        let rooms = [room(Some(1.2), 1.0), room(None, 3.0), room(Some(1.6), 2.0)];
        let r = walk_room(&rooms.iter().collect::<Vec<_>>()).expect("rooms");
        assert_eq!((r.positions, r.sources), (3, 1));
        let t20 = r.octave[0].t20.expect("T20");
        assert!((t20.mean - 1.4).abs() < 1e-12);
        assert_eq!((t20.min, t20.max, t20.n), (1.2, 1.6, 2));
        let c80 = r.octave[1].c80.expect("C80");
        assert!((c80.mean - 2.0).abs() < 1e-12);
        assert_eq!((c80.min, c80.max, c80.n), (1.0, 3.0, 3));
        assert_eq!(r.octave[0].centre, Some(Hz(125.0)));
        assert_eq!(r.broadband.centre, None);
    }

    #[test]
    fn a_band_no_position_gives_has_no_value_and_none_without_rooms() {
        let rooms = [room(None, 1.0), room(None, 1.0)];
        let r = walk_room(&rooms.iter().collect::<Vec<_>>()).expect("rooms");
        assert_eq!(r.octave[0].t20, None);
        assert_eq!(walk_room(&[]), None);
    }

    #[test]
    fn a_band_one_position_lacks_counts_the_others() {
        let mut narrow = room(Some(1.0), 1.0);
        narrow.octave.remove(0);
        let rooms = [room(Some(2.0), 1.0), narrow];
        let r = walk_room(&rooms.iter().collect::<Vec<_>>()).expect("rooms");
        assert_eq!(r.octave.len(), 2);
        assert_eq!(r.octave[0].t20.expect("T20").n, 1);
        assert_eq!(r.octave[1].t20.expect("T20").n, 2);
    }
}
