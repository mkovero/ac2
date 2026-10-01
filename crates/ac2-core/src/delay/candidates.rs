//! Detection floor, peak picking, lobe fit and deblending (design Q1 §5).

use num_complex::Complex64;

use super::estimator::Model;

/// Rayleigh envelope: median = σ·√(2 ln 2).
pub(super) const RAYLEIGH_MEDIAN: f64 = 1.1774;

/// Median (mean of the two middle values for an even count). Reorders `v`.
pub(super) fn median(v: &mut [f64]) -> f64 {
    let n = v.len();
    if n == 0 {
        return 0.0;
    }
    let k = n / 2;
    let (left, hi, _) = v.select_nth_unstable_by(k, f64::total_cmp);
    let hi = *hi;
    if n % 2 == 1 {
        hi
    } else {
        let lo = left.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        0.5 * (lo + hi)
    }
}

/// Median envelope m' and detection level κ·m' of a region (§5.1).
///
/// Under a Rayleigh envelope P(e > a) = exp(−a²/2σ²), so over N_ind independent lags the
/// level noise maxima exceed with probability p_fa is median·√(ln(N_ind/p_fa)/ln 2). Lags
/// within `excl` samples of anything above that level are not floor: wide lobes would
/// otherwise inflate the median.
pub(super) fn detection_floor(
    env: &[f64],
    b_eff: f64,
    fs: f64,
    p_fa: f64,
    excl: f64,
    tmp: &mut Vec<f64>,
) -> (f64, f64) {
    let n = env.len();
    let n_ind = (n as f64 / fs * b_eff).max(1.0);
    let kappa = ((n_ind / p_fa).ln() / std::f64::consts::LN_2).sqrt();
    tmp.clear();
    tmp.extend_from_slice(env);
    let mut med = median(tmp);
    let h = excl.ceil().max(0.0) as usize;
    let lvl = med * kappa;
    // prefix count of lags above the level, for a centred ±h dilation
    let mut prefix = Vec::with_capacity(n + 1);
    prefix.push(0usize);
    for &e in env {
        let last = prefix[prefix.len() - 1];
        prefix.push(last + usize::from(e > lvl));
    }
    tmp.clear();
    for (i, &e) in env.iter().enumerate() {
        let lo = i.saturating_sub(h);
        let hi = (i + h + 1).min(n);
        if prefix[hi] == prefix[lo] {
            tmp.push(e);
        }
    }
    if tmp.len() >= n / 4 && !tmp.is_empty() {
        med = median(tmp);
    }
    (med, med * kappa)
}

/// Local maxima of `e` (`e[i] ≥ e[i−1]` and `e[i] > e[i+1]`; an end sample counts when it
/// exceeds its one neighbour).
pub(super) fn local_maxima(e: &[f64], out: &mut Vec<usize>) {
    out.clear();
    let n = e.len();
    if n == 1 {
        return;
    }
    for i in 0..n {
        let is_max = if i == 0 {
            e[0] > e[1]
        } else if i == n - 1 {
            e[i] > e[i - 1]
        } else {
            e[i] >= e[i - 1] && e[i] > e[i + 1]
        };
        if is_max {
            out.push(i);
        }
    }
}

/// Vertex offset of a parabola through three points, in [−½, ½]; 0 when not concave.
pub(super) fn parabolic(y0: f64, y1: f64, y2: f64) -> f64 {
    let den = y0 - 2.0 * y1 + y2;
    if den >= 0.0 {
        return 0.0;
    }
    (0.5 * (y0 - y2) / den).clamp(-0.5, 0.5)
}

/// Fit c·|p(k − τ)| to envelope samples `ev` at integer offsets `ks` (§5.3). Returns τ and
/// the relative RMS misfit. 25-point grid over ±`reach`, refined by 8× down to 1/64 sample,
/// then a parabolic vertex.
fn lobe_fit(ev: &[f64], ks: &[i64], model: &Model, reach: f64) -> (f64, f64) {
    if !ev.iter().all(|v| v.is_finite()) {
        return (0.0, f64::INFINITY);
    }
    let misfit = |tau: f64| -> f64 {
        let (mut em, mut mm, mut cnt) = (0.0, 0.0, 0usize);
        for (&e, &k) in ev.iter().zip(ks) {
            let m = model.abs_at(k as f64 - tau);
            if m >= 0.1 {
                em += e * m;
                mm += m * m;
                cnt += 1;
            }
        }
        if cnt < 3 {
            return f64::INFINITY;
        }
        let amp = em / mm;
        let mut r2 = 0.0;
        for (&e, &k) in ev.iter().zip(ks) {
            let m = model.abs_at(k as f64 - tau);
            if m >= 0.1 {
                r2 += (e - amp * m).powi(2);
            }
        }
        let v = r2.sqrt() / (amp * mm.sqrt());
        if v.is_nan() { f64::INFINITY } else { v }
    };
    let mut centre = 0.0;
    let mut step = reach / 12.0;
    let mut taus = [0.0; 25];
    let mut f = [0.0; 25];
    let mut best = 0;
    for _ in 0..3 {
        for (i, (t, fv)) in taus.iter_mut().zip(f.iter_mut()).enumerate() {
            *t = centre + step * (i as f64 - 12.0);
            *fv = misfit(*t);
        }
        best = 0;
        for i in 1..25 {
            if f[i] < f[best] {
                best = i;
            }
        }
        centre = taus[best];
        if step <= 1.0 / 64.0 {
            break;
        }
        step = (step / 8.0).max(1.0 / 64.0);
    }
    let i = best;
    if i > 0 && i < 24 && f[i - 1..=i + 1].iter().all(|v| v.is_finite()) {
        let tau = taus[i] + parabolic(-f[i - 1], -f[i], -f[i + 1]) * (taus[1] - taus[0]);
        return (tau, misfit(tau));
    }
    (taus[i], f[i])
}

/// One candidate during deblending, in lag-index coordinates.
#[derive(Debug, Clone, Copy)]
pub(super) struct Lobe {
    pub j: usize,
    pub tau: f64,
    pub amp: Complex64,
    pub misfit: f64,
}

/// Joint refinement of candidate times and complex amplitudes (§5.3). Each lobe is fitted
/// after subtracting the complex model of every neighbour within `reach`, because a
/// neighbour's skirt otherwise pulls a weaker arrival's envelope peak.
pub(super) fn deblend(
    hcx: &[Complex64],
    lobes: &mut [Lobe],
    model: &Model,
    width: f64,
    reach: f64,
) {
    let half = ((1.5 * width).ceil() as i64).max(2);
    let n = hcx.len() as i64;
    for l in lobes.iter_mut() {
        l.amp = hcx[l.j] / model.at(l.j as f64 - l.tau);
        l.misfit = 0.0;
    }
    let reach_fit = (0.05 * width).max(1.5);
    let mut ks: Vec<i64> = Vec::new();
    let mut r: Vec<Complex64> = Vec::new();
    let mut ev: Vec<f64> = Vec::new();
    for _ in 0..2 {
        for a in 0..lobes.len() {
            let j = lobes[a].j as i64;
            ks.clear();
            ks.extend((-half..=half).filter(|k| j + k >= 0 && j + k < n));
            r.clear();
            r.extend(ks.iter().map(|k| hcx[(j + k) as usize]));
            for b in 0..lobes.len() {
                if b != a && (lobes[b].tau - lobes[a].tau).abs() < reach {
                    let (tb, ab) = (lobes[b].tau, lobes[b].amp);
                    for (rv, k) in r.iter_mut().zip(&ks) {
                        *rv -= ab * model.at((j + k) as f64 - tb);
                    }
                }
            }
            ev.clear();
            ev.extend(r.iter().map(|v| v.norm()));
            let (tau, mm) = lobe_fit(&ev, &ks, model, reach_fit);
            if tau.abs() < reach_fit {
                lobes[a].tau = j as f64 + tau;
            }
            let rel = lobes[a].tau - j as f64;
            let mut i0 = 0;
            for (i, k) in ks.iter().enumerate() {
                if (*k as f64 - rel).abs() < (ks[i0] as f64 - rel).abs() {
                    i0 = i;
                }
            }
            lobes[a].amp = r[i0] / model.at((j + ks[i0]) as f64 - lobes[a].tau);
            lobes[a].misfit = mm;
        }
    }
}
