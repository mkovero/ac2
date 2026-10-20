//! Alignment of two systems in their overlap (`docs/design/alignment.md`): the delay and
//! polarity that make two transfer functions `A` and `B`, measured on one time base, add
//! instead of cancel across a band they share (sub and main around their crossover, main
//! and a fill or a delay tower where both cover a seat).
//!
//! The phase difference `Δφ(f) = arg(A/B)` of two systems differing by an arrival `τ` and a
//! polarity `p` is `−2πfτ + pπ` (mod 2π), plus whatever their filters' phases differ by.
//! The fit scores a candidate `(τ, p)` by the weighted mean cosine of the residual phase
//! `Δφ + 2πfτ − pπ`, weighted by `|A|·|B|·γ²`: `|A|·|B|` is the size of the interaction
//! term in `|A + B|² = |A|² + |B|² + 2|A||B|cos Δφ`, so maximising the score maximises the
//! band's summed power, and a column where one system is far below the other (it cannot
//! cancel anything) barely counts. The score needs no unwrapping; its local maxima are the
//! wraps, about one period of the band's centre apart, each refined by a weighted least
//! squares fit of the residual's slope.

use std::f64::consts::{PI, TAU};

use num_complex::Complex64;

/// Where to look.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// Lowest frequency of the overlap, Hz.
    pub lo: f64,
    /// Highest frequency of the overlap, Hz.
    pub hi: f64,
    /// Largest delay either way that is searched, seconds.
    pub max_delay: f64,
}

/// One way to align the two.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    /// How much later A arrives than B, seconds: positive, B is delayed by it; negative, A
    /// by its magnitude.
    pub delay: f64,
    /// The delayed one's polarity is inverted.
    pub inverted: bool,
    /// Weighted RMS of the residual phase difference over the band, degrees.
    pub residual_deg: f64,
    /// Weighted mean cosine of the residual phase difference, −1 … 1 (1: in phase across
    /// the band).
    pub score: f64,
}

/// How fully the two add, `20·lg(|A + B| / (|A| + |B|))` dB per column (0: fully; very
/// negative: they cancel), over the band.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summation {
    /// Weighted mean, dB.
    pub mean_db: f64,
    /// Lowest column, dB.
    pub worst_db: f64,
}

/// The fit.
#[derive(Debug, Clone, PartialEq)]
pub struct Fit {
    /// The best alignment.
    pub best: Candidate,
    /// The best alignment of the other polarity.
    pub other_polarity: Option<Candidate>,
    /// Another wrap of the best's polarity that fits nearly as well
    /// ([`RUNNER_UP_SCORE`]): the band is too narrow to tell them apart.
    pub runner_up: Option<Candidate>,
    /// Summation as measured.
    pub before: Summation,
    /// Summation with the best alignment applied.
    pub after: Summation,
    /// Columns of the band that took part.
    pub columns: usize,
}

/// Why there is no fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoFit {
    /// Fewer than [`MIN_COLUMNS`] columns with a value in both in the band.
    FewColumns {
        /// Columns found.
        found: usize,
    },
    /// No column carries weight: no coherence, or one system silent across the band.
    NoWeight,
}

/// Fewest columns a fit needs: a slope and its residual.
pub const MIN_COLUMNS: usize = 3;
/// A same-polarity candidate whose score is within this of the best is reported.
pub const RUNNER_UP_SCORE: f64 = 0.1;
/// Scan steps per period of the band's top frequency (22.5° of residual between steps).
const STEPS_PER_PERIOD: f64 = 16.0;
/// Most scan steps: bounds the work of a long search over a high band; the refinement
/// still finds a maximum the coarser scan brackets.
const MAX_STEPS: usize = 40_000;

/// `d` wrapped into (−π, π].
pub fn wrap(d: f64) -> f64 {
    let w = d - TAU * ((d + PI) / TAU).floor();
    if w <= -PI { w + TAU } else { w }
}

/// The residual phase difference at `f` of `a`, `b` aligned by `delay` (A's lateness, as
/// [`Candidate::delay`]) and `inverted`, radians in (−π, π].
pub fn residual(f: f64, a: Complex64, b: Complex64, delay: f64, inverted: bool) -> f64 {
    let flip = if inverted { PI } else { 0.0 };
    wrap((a / b).arg() + TAU * f * delay - flip)
}

/// What `a` and `b` sum to at `f` with the delayed one delayed by `|delay|` and inverted
/// when `inverted`.
pub fn aligned_sum(f: f64, a: Complex64, b: Complex64, delay: f64, inverted: bool) -> Complex64 {
    let sign = if inverted { -1.0 } else { 1.0 };
    let shift = Complex64::from_polar(sign, -TAU * f * delay.abs());
    if delay >= 0.0 {
        a + b * shift
    } else {
        a * shift + b
    }
}

/// `20·lg(|sum| / (|a| + |b|))`, dB; `None` where both are zero or not finite.
pub fn summation_db(a: Complex64, b: Complex64, sum: Complex64) -> Option<f64> {
    let whole = a.norm() + b.norm();
    let v = 20.0 * (sum.norm() / whole).log10();
    (whole > 0.0 && !v.is_nan()).then_some(v.max(-200.0))
}

struct Col {
    f: f64,
    z: Complex64,
    a: Complex64,
    b: Complex64,
    w: f64,
}

fn score(cols: &[Col], wsum: f64, delay: f64) -> f64 {
    cols.iter()
        .map(|c| c.w * (c.z * Complex64::from_polar(1.0, TAU * c.f * delay)).re)
        .sum::<f64>()
        / wsum
}

fn rms_deg(cols: &[Col], wsum: f64, delay: f64, inverted: bool) -> f64 {
    let s: f64 = cols
        .iter()
        .map(|c| c.w * residual(c.f, c.a, c.b, delay, inverted).powi(2))
        .sum();
    (s / wsum).sqrt().to_degrees()
}

/// Gauss–Newton on the wrapped residual: the weighted least squares slope of the phase
/// difference unwrapped around this wrap. Each step is kept within `step` so it stays on
/// the wrap the scan bracketed.
fn refine(cols: &[Col], delay: f64, inverted: bool, step: f64, max_delay: f64) -> f64 {
    let mut t = delay;
    let den: f64 = cols.iter().map(|c| c.w * (TAU * c.f).powi(2)).sum();
    for _ in 0..30 {
        let num: f64 = cols
            .iter()
            .map(|c| c.w * residual(c.f, c.a, c.b, t, inverted) * TAU * c.f)
            .sum();
        let d = (-num / den).clamp(-step, step);
        t = (t + d).clamp(-max_delay, max_delay);
        if d.abs() < 1e-12 {
            break;
        }
    }
    t
}

fn summation(cols: &[Col], delay: f64, inverted: bool) -> Summation {
    let mut mean = 0.0;
    let mut worst = f64::INFINITY;
    let mut wsum_used = 0.0;
    for c in cols {
        let sum = aligned_sum(c.f, c.a, c.b, delay, inverted);
        if let Some(s) = summation_db(c.a, c.b, sum) {
            mean += c.w * s;
            wsum_used += c.w;
            worst = worst.min(s);
        }
    }
    Summation {
        mean_db: if wsum_used > 0.0 {
            mean / wsum_used
        } else {
            f64::NAN
        },
        worst_db: if worst.is_finite() { worst } else { f64::NAN },
    }
}

/// Fits the alignment of `a` to `b` over `p`'s band. `freqs`, `a`, `b` and `coherence`
/// (γ² per column, the lower of the two systems'; `None`: every column counts by its
/// magnitudes alone) are per column; a column with a non-finite value takes no part.
pub fn fit(
    freqs: &[f64],
    a: &[Complex64],
    b: &[Complex64],
    coherence: Option<&[f64]>,
    p: &Params,
) -> Result<Fit, NoFit> {
    let mut cols = Vec::new();
    for (k, &f) in freqs.iter().enumerate() {
        let (Some(&ak), Some(&bk)) = (a.get(k), b.get(k)) else {
            break;
        };
        if !(f >= p.lo && f <= p.hi) {
            continue;
        }
        let g = coherence.map_or(Some(1.0), |c| c.get(k).copied());
        let Some(g) = g.filter(|g| g.is_finite()) else {
            continue;
        };
        let (ma, mb) = (ak.norm(), bk.norm());
        if !(ak.is_finite() && bk.is_finite() && ma > 0.0 && mb > 0.0) {
            continue;
        }
        cols.push(Col {
            f,
            z: ak * bk.conj() / (ma * mb),
            a: ak,
            b: bk,
            w: ma * mb * g.clamp(0.0, 1.0),
        });
    }
    if cols.len() < MIN_COLUMNS {
        return Err(NoFit::FewColumns { found: cols.len() });
    }
    // Relative weights: the magnitudes' scale is the operands' and does not matter.
    let wmax = cols.iter().map(|c| c.w).fold(0.0, f64::max);
    if !(wmax > 0.0 && wmax.is_finite()) {
        return Err(NoFit::NoWeight);
    }
    for c in &mut cols {
        c.w /= wmax;
    }
    let wsum: f64 = cols.iter().map(|c| c.w).sum();
    let top = cols.iter().map(|c| c.f).fold(0.0, f64::max);
    let mut step = 1.0 / (STEPS_PER_PERIOD * top);
    let max = p.max_delay.max(0.0);
    let mut n = (2.0 * max / step).ceil() as usize + 1;
    if n > MAX_STEPS {
        n = MAX_STEPS;
        step = 2.0 * max / (n - 1) as f64;
    }
    let at = |i: usize| {
        if n == 1 {
            0.0
        } else {
            -max + 2.0 * max * i as f64 / (n - 1) as f64
        }
    };
    let s: Vec<f64> = (0..n).map(|i| score(&cols, wsum, at(i))).collect();
    let mut found: Vec<Candidate> = Vec::new();
    for inverted in [false, true] {
        let v = |i: usize| if inverted { -s[i] } else { s[i] };
        for i in 0..n {
            let left = i == 0 || v(i) >= v(i - 1);
            let right = i + 1 == n || v(i) > v(i + 1);
            if !(left && right && v(i) > 0.0) {
                continue;
            }
            let t = refine(&cols, at(i), inverted, step, max);
            let sc = score(&cols, wsum, t) * if inverted { -1.0 } else { 1.0 };
            if found
                .iter()
                .any(|c| c.inverted == inverted && (c.delay - t).abs() < step / 2.0)
            {
                continue;
            }
            found.push(Candidate {
                delay: t,
                inverted,
                residual_deg: rms_deg(&cols, wsum, t, inverted),
                score: sc,
            });
        }
    }
    // Best first; among equal scores the smaller delay (less to add).
    found.sort_by(|x, y| {
        y.score
            .total_cmp(&x.score)
            .then(x.delay.abs().total_cmp(&y.delay.abs()))
    });
    let Some(&best) = found.first() else {
        return Err(NoFit::NoWeight);
    };
    let other_polarity = found.iter().find(|c| c.inverted != best.inverted).copied();
    let runner_up = found
        .iter()
        .skip(1)
        .find(|c| c.inverted == best.inverted)
        .filter(|c| c.score >= best.score - RUNNER_UP_SCORE)
        .copied();
    Ok(Fit {
        best,
        other_polarity,
        runner_up,
        before: summation(&cols, 0.0, false),
        after: summation(&cols, best.delay, best.inverted),
        columns: cols.len(),
    })
}

#[cfg(test)]
mod tests;
