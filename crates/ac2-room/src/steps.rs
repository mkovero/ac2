//! A walk with several mics (`docs/design/spatial-walk.md` §6): its points grouped into steps,
//! each step placing every mic once, so that between steps the mics' cables move as little as
//! possible. Pure and deterministic: the same points and mic count give the same steps.

use crate::geom::dist;

/// One step: per mic (in the walk's mic order) the index of the point it stands at, `None`
/// when it sits the step out (fewer points left than mics).
pub type Step = Vec<Option<usize>>;

/// The points (plan x, y; metres, in walking order) grouped into steps of `mics`.
///
/// One mic keeps the walking order: a step per point. With several, the first step puts mic
/// *k* at the *k*-th point (the operator reads the walk from P1), and each later step is the
/// nearest-neighbour assignment from where every mic stood last: the points left and the
/// mics are paired one to one so the summed distance the mics are carried is least
/// ([`assign`]). When fewer points are left than mics, the mics nearest them go and the
/// others sit out. Distance is on the floor plan: a cable runs along the floor, and ear
/// height is the same at every seat of a zone.
pub fn group(points: &[[f64; 2]], mics: usize) -> Vec<Step> {
    if mics == 0 || points.is_empty() {
        return Vec::new();
    }
    if mics == 1 {
        return (0..points.len()).map(|i| vec![Some(i)]).collect();
    }
    let first = mics.min(points.len());
    let mut steps: Vec<Step> = vec![(0..mics).map(|k| (k < first).then_some(k)).collect()];
    // Where every mic stood last; a mic never placed has nowhere to come from, and that
    // only happens when the first step already took every point.
    let mut last: Vec<[f64; 2]> = (0..first).map(|k| points[k]).collect();
    let mut left: Vec<usize> = (first..points.len()).collect();
    while !left.is_empty() {
        let to: Vec<[f64; 2]> = left.iter().map(|&i| points[i]).collect();
        let mut step: Step = vec![None; mics];
        let mut taken: Vec<usize> = Vec::new();
        if left.len() >= last.len() {
            // Every mic moves, each to a distinct point left.
            for (k, j) in assign(&last, &to).into_iter().enumerate() {
                step[k] = Some(left[j]);
                taken.push(j);
            }
        } else {
            // Every point left is taken, each by a distinct mic.
            for (j, k) in assign(&to, &last).into_iter().enumerate() {
                step[k] = Some(left[j]);
                taken.push(j);
            }
        }
        for (k, at) in step.iter().enumerate() {
            if let Some(i) = at {
                last[k] = points[*i];
            }
        }
        taken.sort_unstable();
        for j in taken.into_iter().rev() {
            left.remove(j);
        }
        steps.push(step);
    }
    steps
}

/// The least-total-distance pairing of every `from` with a distinct `to` (`from.len()` ≤
/// `to.len()`): for each `from`, the index of its `to`. Exact (the Hungarian method), so
/// the pairing is the least, not a greedy approximation of it; deterministic for a given
/// order of the inputs.
///
/// # Panics
/// When `from` holds more points than `to`.
pub fn assign(from: &[[f64; 2]], to: &[[f64; 2]]) -> Vec<usize> {
    assert!(from.len() <= to.len(), "assign: more sources than targets");
    let n = from.len();
    let m = to.len();
    let cost = |i: usize, j: usize| dist(from[i - 1], to[j - 1]);
    // Potentials of rows and columns; `p[j]` the row matched to column j (0: none, rows and
    // columns 1-based so column 0 is the free start of each augmenting path).
    let mut u = vec![0.0f64; n + 1];
    let mut v = vec![0.0f64; m + 1];
    let mut p = vec![0usize; m + 1];
    let mut way = vec![0usize; m + 1];
    for i in 1..=n {
        p[0] = i;
        let mut j0 = 0usize;
        let mut minv = vec![f64::INFINITY; m + 1];
        let mut used = vec![false; m + 1];
        loop {
            used[j0] = true;
            let i0 = p[j0];
            let mut delta = f64::INFINITY;
            let mut j1 = 0usize;
            for j in 1..=m {
                if used[j] {
                    continue;
                }
                let cur = cost(i0, j) - u[i0] - v[j];
                if cur < minv[j] {
                    minv[j] = cur;
                    way[j] = j0;
                }
                if minv[j] < delta {
                    delta = minv[j];
                    j1 = j;
                }
            }
            for j in 0..=m {
                if used[j] {
                    u[p[j]] += delta;
                    v[j] -= delta;
                } else {
                    minv[j] -= delta;
                }
            }
            j0 = j1;
            if p[j0] == 0 {
                break;
            }
        }
        loop {
            let j1 = way[j0];
            p[j0] = p[j1];
            j0 = j1;
            if j0 == 0 {
                break;
            }
        }
    }
    let mut out = vec![0usize; n];
    for j in 1..=m {
        if p[j] != 0 {
            out[p[j] - 1] = j - 1;
        }
    }
    out
}
