//! An alignment math channel's frame (`docs/design/alignment.md`): the fit of
//! [`ac2_core::align`] over the operands' complex values on one time base, the curve it
//! publishes (their sum with the fit applied, referred to A's delay) and the alignment
//! view's curves.

use std::f64::consts::TAU;

use ac2_core::align::{self as core, NoFit, Params};
use ac2_proto::model::{
    AlignBand, AlignCandidate, AlignCurves, AlignFit, AlignNoFit, AlignOutcome, AlignSide,
    AlignState, PhaseBasis, Summation,
};
use ac2_proto::units::Seconds;
use num_complex::Complex64;

use super::{Input, MathError, MathResult, complex, lower_coherence, polar};
use crate::columns::Columns;

fn candidate(c: core::Candidate) -> AlignCandidate {
    AlignCandidate {
        delay: Seconds(c.delay.abs()),
        // A later than B: B waits for it.
        delayed: if c.delay >= 0.0 {
            AlignSide::B
        } else {
            AlignSide::A
        },
        inverted: c.inverted,
        residual_deg: c.residual_deg,
        score: c.score,
    }
}

fn summation(s: core::Summation) -> Summation {
    Summation {
        mean_db: s.mean_db,
        worst_db: s.worst_db,
    }
}

/// Aligns `b` with `a` in `band`, searching delays up to `max_delay` either way. Both need
/// phase and one time base: the relative arrival is what is fitted.
pub fn align(
    a: &Input<'_>,
    b: &Input<'_>,
    freqs: &[f64],
    band: AlignBand,
    max_delay: Seconds,
) -> Result<(MathResult, AlignState, AlignCurves), MathError> {
    if a.columns.phase_deg.is_none() {
        return Err(MathError::NoPhase(0));
    }
    if b.columns.phase_deg.is_none() {
        return Err(MathError::NoPhase(1));
    }
    if a.time_base.is_none() || a.time_base != b.time_base {
        return Err(MathError::NoSharedTimeBase);
    }
    let n = freqs.len();
    // Each operand's phase is referred to its own inserted delay; on one time base
    // `H = h·e^{−j2πfτ}` is what reached the mic.
    let on_clock = |inp: &Input<'_>, i: usize| {
        complex(inp.columns, i).map(|h| h * Complex64::from_polar(1.0, -TAU * freqs[i] * inp.delay))
    };
    let nan = Complex64::new(f64::NAN, f64::NAN);
    let ha: Vec<Complex64> = (0..n).map(|i| on_clock(a, i).unwrap_or(nan)).collect();
    let hb: Vec<Complex64> = (0..n).map(|i| on_clock(b, i).unwrap_or(nan)).collect();
    if (0..n).all(|i| !(ha[i].is_finite() && hb[i].is_finite())) {
        return Err(MathError::NoOverlap);
    }
    let coh = lower_coherence(a.columns, b.columns)
        .map(|c| c.iter().map(|v| f64::from(*v)).collect::<Vec<f64>>());
    let fit = core::fit(
        freqs,
        &ha,
        &hb,
        coh.as_deref(),
        &Params {
            lo: band.lo.0,
            hi: band.hi.0,
            max_delay: max_delay.0,
        },
    );
    let (delay, inverted, outcome) = match fit {
        Ok(f) => (
            f.best.delay,
            f.best.inverted,
            AlignOutcome::Fit {
                fit: AlignFit {
                    best: candidate(f.best),
                    other_polarity: f.other_polarity.map(candidate),
                    runner_up: f.runner_up.map(candidate),
                    before: summation(f.before),
                    after: summation(f.after),
                    columns: u32::try_from(f.columns).unwrap_or(u32::MAX),
                },
            },
        ),
        Err(why) => (
            0.0,
            false,
            AlignOutcome::NoFit {
                why: match why {
                    NoFit::FewColumns { found } => AlignNoFit::FewColumns {
                        found: u32::try_from(found).unwrap_or(u32::MAX),
                    },
                    NoFit::NoWeight => AlignNoFit::NoWeight,
                },
            },
        ),
    };
    let mut mag = Vec::with_capacity(n);
    let mut phase = Vec::with_capacity(n);
    let mut state = AlignCurves {
        phase_before: Vec::with_capacity(n),
        phase_after: Vec::with_capacity(n),
        sum_before: Vec::with_capacity(n),
        sum_after: Vec::with_capacity(n),
    };
    let deg = |r: f64| r.to_degrees() as f32;
    for i in 0..n {
        let (x, y, f) = (ha[i], hb[i], freqs[i]);
        if !(x.is_finite() && y.is_finite() && x.norm() > 0.0 && y.norm() > 0.0) {
            mag.push(f32::NAN);
            phase.push(f32::NAN);
            for v in [
                &mut state.phase_before,
                &mut state.phase_after,
                &mut state.sum_before,
                &mut state.sum_after,
            ] {
                v.push(f32::NAN);
            }
            continue;
        }
        let after = core::aligned_sum(f, x, y, delay, inverted);
        // Referred to A's delay, as `A + B` is by default.
        let (m, p) = polar(after * Complex64::from_polar(1.0, TAU * f * a.delay));
        mag.push(m);
        phase.push(p);
        state
            .phase_before
            .push(deg(core::residual(f, x, y, 0.0, false)));
        state
            .phase_after
            .push(deg(core::residual(f, x, y, delay, inverted)));
        let db = |s| core::summation_db(x, y, s).map_or(f32::NAN, |v| v as f32);
        state.sum_before.push(db(x + y));
        state.sum_after.push(db(after));
    }
    Ok((
        MathResult {
            columns: Columns {
                mag_db: mag,
                phase_deg: Some(phase),
                coherence: None,
            },
            delay: Seconds(a.delay),
            phase: PhaseBasis::SharedTimeBase,
        },
        AlignState { outcome },
        state,
    ))
}

#[cfg(test)]
mod tests;
