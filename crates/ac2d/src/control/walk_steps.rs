//! A walk's steps (`docs/design/spatial-walk.md` §6): which mic stands at which point at each
//! step, from the grouping `ac2_room::steps` makes, and how a swap, a redo and an edit of the
//! points keep it one mic per pending point. Pure: walk state in, walk state out.

use ac2_proto::model::{PointState, WalkPoint, WalkState};
use ac2_proto::units::MeasId;
use ac2_proto::{ErrorCode, ProtoError};

use crate::util::perr;

/// Groups `points` into steps of `meas` (one per mic) and gives each its step and mic. A
/// captured point keeps the mic that captured it: that is a fact of its trace, whatever the
/// grouping would pick now.
pub(super) fn regroup(points: &mut [WalkPoint], meas: &[MeasId]) {
    let at: Vec<[f64; 2]> = points.iter().map(|p| [p.at[0], p.at[1]]).collect();
    for (s, step) in ac2_room::steps::group(&at, meas.len())
        .into_iter()
        .enumerate()
    {
        for (k, i) in step.into_iter().enumerate() {
            let Some(i) = i else { continue };
            let p = &mut points[i];
            p.step = index(s);
            if !matches!(p.state, PointState::Captured { .. }) {
                p.mic = meas[k];
            }
        }
    }
}

/// The first step after `after` (from the start when `None`), wrapping round, that holds a
/// pending point; `None` when every point is captured or skipped.
pub(super) fn next_step(w: &WalkState, after: Option<u16>) -> Option<u16> {
    let n = usize::from(w.steps());
    let start = after.map_or(0, |a| usize::from(a) + 1);
    (0..n)
        .map(|k| index((start + k) % n))
        .find(|&s| Some(s) != after && has_pending(w, s))
}

/// Whether step `step` holds a pending point.
pub(super) fn has_pending(w: &WalkState, step: u16) -> bool {
    w.points
        .iter()
        .any(|p| p.step == step && matches!(p.state, PointState::Pending))
}

/// Point `i` is pending again for its mic. When that mic already stands at another pending
/// point of the step (a swap handed it that one), the point goes to a mic the step leaves
/// free: a step holds at most one point per mic, so with one mic doubled one is free.
pub(super) fn repend(w: &mut WalkState, i: usize) {
    w.points[i].state = PointState::Pending;
    let (step, mic) = (w.points[i].step, w.points[i].mic);
    let busy = |w: &WalkState, m: MeasId| {
        w.points.iter().enumerate().any(|(j, p)| {
            j != i && p.step == step && p.mic == m && matches!(p.state, PointState::Pending)
        })
    };
    if busy(w, mic)
        && let Some(free) = w.meas.iter().copied().find(|&m| !busy(w, m))
    {
        w.points[i].mic = free;
    }
}

/// `walk.swap`: the mics at `points` (pending points of the current step) move round by one,
/// and the pending points of later steps follow the same exchange, so each mic goes on from
/// where it now stands.
pub(super) fn swap(w: &mut WalkState, points: &[u16]) -> Result<(), ProtoError> {
    let bad = |m: String| perr(ErrorCode::Invalid, m);
    let Some(step) = w.step else {
        return Err(bad(
            "every point of the walk is captured or skipped: no mic stands anywhere".into(),
        ));
    };
    if points.len() < 2 {
        return Err(bad("a swap takes the points of at least two mics".into()));
    }
    let mut old: Vec<MeasId> = Vec::with_capacity(points.len());
    for (k, &pt) in points.iter().enumerate() {
        let Some(p) = w.points.get(usize::from(pt)) else {
            return Err(perr(
                ErrorCode::NotFound,
                format!("the walk {} has no point {pt}", w.name),
            ));
        };
        if points[..k].contains(&pt) {
            return Err(bad(format!("{} is named twice", p.label)));
        }
        if p.step != step || !matches!(p.state, PointState::Pending) {
            return Err(perr(
                ErrorCode::Refused,
                format!(
                    "no mic stands at {}: only this step's pending points swap",
                    p.label
                ),
            ));
        }
        old.push(p.mic);
    }
    let n = old.len();
    for (k, &pt) in points.iter().enumerate() {
        w.points[usize::from(pt)].mic = old[(k + n - 1) % n];
    }
    // The mic that stood at the k+1-th point now stands where the k-th one did; later
    // steps were planned from where each stood, so whoever stands there walks on.
    let follow = |m: MeasId| {
        old.iter()
            .position(|&o| o == m)
            .map_or(m, |k| old[(k + n - 1) % n])
    };
    for p in &mut w.points {
        if p.step > step && matches!(p.state, PointState::Pending) {
            p.mic = follow(p.mic);
        }
    }
    Ok(())
}

/// A loaded walk's steps hold together: every point's mic is one of the walk's, no step
/// places a mic at two pending points, and the current step exists.
pub(super) fn check(w: &WalkState) -> Result<(), ProtoError> {
    let bad = |m: String| perr(ErrorCode::Invalid, m);
    if w.meas.is_empty() || w.meas.len() > WalkState::MAX_MICS {
        return Err(bad(format!(
            "walk {} captures with {} measurements; 1 … {}",
            w.id,
            w.meas.len(),
            WalkState::MAX_MICS
        )));
    }
    if (1..w.meas.len()).any(|k| w.meas[..k].contains(&w.meas[k])) {
        return Err(bad(format!("walk {} names a measurement twice", w.id)));
    }
    for (i, p) in w.points.iter().enumerate() {
        if !w.meas.contains(&p.mic) {
            return Err(bad(format!(
                "walk {} point {}: measurement {} is none of the walk's",
                w.id, p.label, p.mic
            )));
        }
        if usize::from(p.step) >= w.points.len() {
            return Err(bad(format!(
                "walk {} point {}: no step {}",
                w.id, p.label, p.step
            )));
        }
        let pending = |q: &WalkPoint| matches!(q.state, PointState::Pending);
        if pending(p)
            && w.points[..i]
                .iter()
                .any(|q| pending(q) && q.step == p.step && q.mic == p.mic)
        {
            return Err(bad(format!(
                "walk {} step {}: one mic at two points",
                w.id, p.step
            )));
        }
    }
    if w.step.is_some_and(|s| s >= w.steps()) {
        return Err(bad(format!("walk {} stands at no step", w.id)));
    }
    Ok(())
}

pub(super) fn index(i: usize) -> u16 {
    // A walk holds at most `MAX_POINTS` points, so at most as many steps.
    u16::try_from(i).unwrap_or(u16::MAX)
}
