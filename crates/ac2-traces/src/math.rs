//! What a math channel computes from its operands (`docs/design/math-channels.md`): the
//! daemon gathers each operand's columns on the result's grid — a live measurement's current
//! result, a stored trace's columns — and this module combines them. The daemon's live math
//! job and every test use the same functions, so a capture of a math channel and the
//! analytic value agree.
//!
//! # Transfer functions
//!
//! Each operand `k` was measured with its own inserted delay `τₖ` removed
//! (`hₖ = Hₖ·e^{+j2πfτₖ}`). Operands of one session epoch share one time base (decision 8a):
//! their delays are on one clock, so `Hₖ` itself is known and operands can be combined as
//! the complex values they are.
//!
//! - `A ÷ B = H_a / H_b`: the reference delay cancels; the result states delay 0, so its
//!   phase shows A's arrival relative to B. `A × B = H_a·H_b` states `τa + τb` (a cascade's
//!   delays add) and its phase is `h_a·h_b`. Neither needs a shared time base to be what
//!   it says *as displayed*; without one the phase combines each operand as aligned by its
//!   own delay and the result says so ([`PhaseBasis::OwnAlignments`]). An operand without
//!   phase (a target curve) leaves a magnitude-only result.
//! - `A + B` and `A − B` are the complex sum and difference of the operands re-referred to
//!   one delay `τ_ref`: `h_a·e^{−j2πf(τa−τ_ref)} ± h_b·e^{−j2πf(τb−τ_ref)}`. What they sum
//!   to depends on their relative arrival, so both need phase and a shared time base; without
//!   one they are refused rather than computed from an arrival nobody measured.
//! - The average is `trace.average`'s ([`crate::ops::average_on_time_base`]); across time
//!   bases only its power average (magnitude, no phase) exists.
//!
//! Coherence: a ratio or a cascade is only as trustworthy as its less coherent operand, so
//! `÷` and `×` carry the lower γ² of the two per column — a display mask, not an estimate.
//! A sum or difference has no coherence (it would need the operands' cross-spectrum); an
//! average carries the plain mean of its operands', as `trace.average` does.
//!
//! # Levels (spectrum, RTA)
//!
//! Levels have no phase. `A − B` is the level difference in dB, `A + B` the power sum
//! `10·lg(10^{a/10} + 10^{b/10})` (incoherent sources adding), the average the power mean.
//! Ratios and cascades of levels are not defined here: a level difference is `A − B`.
//!
//! A column has a value only where every operand has one: a gap stays a gap.

use std::f64::consts::TAU;
use std::fmt;

use ac2_core::average::DelayReference;
use ac2_proto::GridDef;
use ac2_proto::model::{AverageMethod, MathOp, PhaseBasis};
use ac2_proto::units::{Seconds, SessionEpoch};
use num_complex::Complex64;

use crate::columns::{Columns, StoredTrace, frequencies, resample};
use crate::ops::{OpError, average_on_time_base};

/// How the operands are combined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combine {
    /// `a op b` (two operands).
    Binary(MathOp),
    /// The average of every operand.
    Average(AverageMethod),
}

/// One operand, on the result's grid.
#[derive(Debug, Clone, Copy)]
pub struct Input<'a> {
    /// Columns (NaN = no value).
    pub columns: &'a Columns,
    /// The inserted delay its phase was measured with, seconds.
    pub delay: f64,
    /// The session epoch whose time base its phase is in; `None` for an independent one
    /// (an import, a derived trace).
    pub time_base: Option<SessionEpoch>,
}

/// A math result.
#[derive(Debug, Clone, PartialEq)]
pub struct MathResult {
    /// Columns on the operands' grid.
    pub columns: Columns,
    /// Delay the phase is referred to.
    pub delay: Seconds,
    /// What the phase is relative to.
    pub phase: PhaseBasis,
}

/// Why operands cannot be combined. Operands are named by their index in the input.
#[derive(Debug, Clone, PartialEq)]
pub enum MathError {
    /// The operator takes another number of operands.
    Count,
    /// The operation needs phase and this operand has none.
    NoPhase(usize),
    /// The operation combines phase across operands that share no time base.
    NoSharedTimeBase,
    /// Coherence weighting with an operand that has no coherence.
    NoCoherence(usize),
    /// Levels have no ratio or product.
    NotForLevels(MathOp),
    /// Levels average on power only.
    PowerOnly,
    /// The reference delay is not finite, or names no operand.
    BadReference,
    /// No column has a value in every operand.
    NoOverlap,
}

impl MathError {
    /// The refusal in words, operands named by `name`.
    pub fn describe(&self, name: impl Fn(usize) -> String) -> String {
        match self {
            Self::Count => "this operator takes two operands".into(),
            Self::NoPhase(k) => format!(
                "{} has no phase: a sum or difference adds complex values (use ÷ for a \
                 magnitude comparison)",
                name(*k)
            ),
            Self::NoSharedTimeBase => "the operands share no time base (an import, or a \
                capture from an earlier audio session): their relative arrival is unknown, so \
                they cannot be summed, differenced or averaged with phase; use ÷, ×, or a \
                power average"
                .into(),
            Self::NoCoherence(k) => {
                format!("{} has no coherence to weight by", name(*k))
            }
            Self::NotForLevels(op) => format!(
                "spectra and RTA bands are levels: they take − (level difference) and + \
                 (power sum), not {}",
                op_symbol(*op)
            ),
            Self::PowerOnly => "spectra and RTA bands average on power only".into(),
            Self::BadReference => "the phase reference must be an operand or a finite delay".into(),
            Self::NoOverlap => "the operands share no frequency with a value".into(),
        }
    }
}

impl fmt::Display for MathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe(|k| format!("operand {}", k + 1)))
    }
}

impl std::error::Error for MathError {}

/// `÷`, `×`, `+`, `−`.
pub fn op_symbol(op: MathOp) -> &'static str {
    match op {
        MathOp::Divide => "÷",
        MathOp::Multiply => "×",
        MathOp::Add => "+",
        MathOp::Subtract => "−",
    }
}

/// `t`'s columns as measured (an applied mic curve baked in, as it corrects what was
/// measured) on `grid`, resampled when its own grid differs.
pub fn trace_on_grid(t: &StoredTrace, grid: &GridDef, freqs: &[f64]) -> Columns {
    let t = crate::mic::bake(t);
    if t.grid.id() == grid.id() {
        t.columns
    } else {
        resample(&frequencies(&t.grid), &t.columns, freqs)
    }
}

/// Transfer math on one grid (`freqs`). `reference` is the delay a sum, difference or
/// average is referred to (an operand index, or a fixed delay).
pub fn transfer(
    combine: Combine,
    inputs: &[Input<'_>],
    grid: &GridDef,
    freqs: &[f64],
    reference: DelayReference,
) -> Result<MathResult, MathError> {
    let shared = shared(inputs);
    match combine {
        Combine::Binary(op) => {
            let [a, b] = inputs else {
                return Err(MathError::Count);
            };
            match op {
                MathOp::Divide | MathOp::Multiply => Ok(ratio_or_cascade(op, a, b, freqs, shared)),
                MathOp::Add | MathOp::Subtract => {
                    sum_or_difference(op, a, b, freqs, shared, reference)
                }
            }
        }
        Combine::Average(method) => average(method, inputs, grid, freqs, shared, reference),
    }
}

/// Level math (spectrum or RTA) on one grid.
pub fn levels(combine: Combine, inputs: &[Input<'_>]) -> Result<MathResult, MathError> {
    let n = inputs.first().map_or(0, |i| i.columns.len());
    let level = |k: usize, i: usize| {
        let v = f64::from(*inputs[k].columns.mag_db.get(i).unwrap_or(&f32::NAN));
        v.is_finite().then_some(v)
    };
    let mag: Vec<f32> = match combine {
        Combine::Binary(op) => {
            if inputs.len() != 2 {
                return Err(MathError::Count);
            }
            let f = match op {
                MathOp::Subtract => |a: f64, b: f64| a - b,
                MathOp::Add => |a: f64, b: f64| power_db(power(a) + power(b)),
                MathOp::Divide | MathOp::Multiply => return Err(MathError::NotForLevels(op)),
            };
            (0..n)
                .map(|i| match (level(0, i), level(1, i)) {
                    (Some(a), Some(b)) => f(a, b) as f32,
                    _ => f32::NAN,
                })
                .collect()
        }
        Combine::Average(method) => {
            if method != AverageMethod::Power {
                return Err(MathError::PowerOnly);
            }
            if inputs.len() < 2 {
                return Err(MathError::Count);
            }
            (0..n)
                .map(|i| {
                    let p: Option<f64> =
                        (0..inputs.len()).try_fold(0.0, |s, k| Some(s + power(level(k, i)?)));
                    p.map_or(f32::NAN, |p| power_db(p / inputs.len() as f64) as f32)
                })
                .collect()
        }
    };
    if mag.iter().all(|v| v.is_nan()) {
        return Err(MathError::NoOverlap);
    }
    Ok(MathResult {
        columns: Columns {
            mag_db: mag,
            phase_deg: None,
            coherence: None,
        },
        delay: Seconds(0.0),
        phase: PhaseBasis::NoPhase,
    })
}

fn power(db: f64) -> f64 {
    10f64.powf(db / 10.0)
}

/// A power as a level; no power has no level (a gap, not −∞).
fn power_db(p: f64) -> f64 {
    if p > 0.0 && p.is_finite() {
        10.0 * p.log10()
    } else {
        f64::NAN
    }
}

/// Every operand in one session epoch's time base.
fn shared(inputs: &[Input<'_>]) -> bool {
    let first = inputs.first().and_then(|i| i.time_base);
    first.is_some() && inputs.iter().all(|i| i.time_base == first)
}

fn complex(c: &Columns, i: usize) -> Option<Complex64> {
    let m = f64::from(*c.mag_db.get(i)?);
    let p = f64::from(*c.phase_deg.as_ref()?.get(i)?);
    (m.is_finite() && p.is_finite())
        .then(|| Complex64::from_polar(10f64.powf(m / 20.0), p.to_radians()))
}

/// Magnitude dB and phase degrees of `z`; a zero (perfect cancellation) has neither.
fn polar(z: Complex64) -> (f32, f32) {
    let r = z.norm();
    if r > 0.0 && r.is_finite() {
        ((20.0 * r.log10()) as f32, z.arg().to_degrees() as f32)
    } else {
        (f32::NAN, f32::NAN)
    }
}

/// The lower γ² of two operands per column (either missing: the other's).
fn lower_coherence(a: &Columns, b: &Columns) -> Option<Vec<f32>> {
    match (&a.coherence, &b.coherence) {
        (Some(x), Some(y)) => Some(
            x.iter()
                .zip(y)
                .map(|(p, q)| {
                    if p.is_nan() || q.is_nan() {
                        f32::NAN
                    } else {
                        p.min(*q)
                    }
                })
                .collect(),
        ),
        (Some(x), None) | (None, Some(x)) => Some(x.clone()),
        (None, None) => None,
    }
}

fn ratio_or_cascade(
    op: MathOp,
    a: &Input<'_>,
    b: &Input<'_>,
    freqs: &[f64],
    shared: bool,
) -> MathResult {
    let n = freqs.len();
    let (ca, cb) = (a.columns, b.columns);
    let divide = op == MathOp::Divide;
    let mag: Vec<f32> = (0..n)
        .map(|i| {
            let (x, y) = (ca.mag_db[i], cb.mag_db[i]);
            if divide { x - y } else { x + y }
        })
        .collect();
    let with_phase = ca.phase_deg.is_some() && cb.phase_deg.is_some();
    let (phase, delay, basis) = if !with_phase {
        (None, 0.0, PhaseBasis::NoPhase)
    } else {
        let basis = if shared {
            PhaseBasis::SharedTimeBase
        } else {
            PhaseBasis::OwnAlignments
        };
        // A ratio of operands on one clock keeps their relative arrival (delay 0 removed);
        // a cascade's delays add, and its phase is the operands' own-aligned phases summed.
        let (shift, delay) = match (divide, shared) {
            (true, true) => (a.delay - b.delay, 0.0),
            (true, false) => (0.0, 0.0),
            (false, _) => (0.0, a.delay + b.delay),
        };
        let phase = (0..n)
            .map(|i| {
                let (Some(za), Some(zb)) = (complex(ca, i), complex(cb, i)) else {
                    return f32::NAN;
                };
                let z = if divide { za / zb } else { za * zb };
                let z = z * Complex64::from_polar(1.0, -TAU * freqs[i] * shift);
                z.arg().to_degrees() as f32
            })
            .collect();
        (Some(phase), delay, basis)
    };
    let mag = match &phase {
        // A column without phase in a phase-bearing result is a gap in both.
        Some(p) => mag
            .iter()
            .zip(p)
            .map(|(m, p): (&f32, &f32)| if p.is_nan() { f32::NAN } else { *m })
            .collect(),
        None => mag,
    };
    MathResult {
        columns: Columns {
            mag_db: mag,
            phase_deg: phase,
            coherence: lower_coherence(ca, cb),
        },
        delay: Seconds(delay),
        phase: basis,
    }
}

fn reference_delay(reference: DelayReference, inputs: &[Input<'_>]) -> Result<f64, MathError> {
    let d = match reference {
        DelayReference::Trace(k) => inputs.get(k).ok_or(MathError::BadReference)?.delay,
        DelayReference::Fixed(d) => d,
    };
    if d.is_finite() {
        Ok(d)
    } else {
        Err(MathError::BadReference)
    }
}

fn sum_or_difference(
    op: MathOp,
    a: &Input<'_>,
    b: &Input<'_>,
    freqs: &[f64],
    shared: bool,
    reference: DelayReference,
) -> Result<MathResult, MathError> {
    for (k, x) in [a, b].iter().enumerate() {
        if x.columns.phase_deg.is_none() {
            return Err(MathError::NoPhase(k));
        }
    }
    if !shared {
        return Err(MathError::NoSharedTimeBase);
    }
    let tref = reference_delay(reference, &[*a, *b])?;
    let sign = if op == MathOp::Add { 1.0 } else { -1.0 };
    let (mag, phase): (Vec<f32>, Vec<f32>) = freqs
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let (Some(za), Some(zb)) = (complex(a.columns, i), complex(b.columns, i)) else {
                return (f32::NAN, f32::NAN);
            };
            let rerefer =
                |z: Complex64, d: f64| z * Complex64::from_polar(1.0, -TAU * f * (d - tref));
            polar(rerefer(za, a.delay) + sign * rerefer(zb, b.delay))
        })
        .unzip();
    if mag.iter().all(|v| v.is_nan()) {
        return Err(MathError::NoOverlap);
    }
    Ok(MathResult {
        columns: Columns {
            mag_db: mag,
            phase_deg: Some(phase),
            coherence: None,
        },
        delay: Seconds(tref),
        phase: PhaseBasis::SharedTimeBase,
    })
}

fn average(
    method: AverageMethod,
    inputs: &[Input<'_>],
    grid: &GridDef,
    freqs: &[f64],
    shared: bool,
    reference: DelayReference,
) -> Result<MathResult, MathError> {
    if inputs.len() < 2 {
        return Err(MathError::Count);
    }
    if method == AverageMethod::CoherenceWeighted
        && let Some(k) = inputs.iter().position(|i| i.columns.coherence.is_none())
    {
        return Err(MathError::NoCoherence(k));
    }
    let no_phase = inputs.iter().position(|i| i.columns.phase_deg.is_none());
    if shared && no_phase.is_none() {
        let cols: Vec<Columns> = inputs.iter().map(|i| i.columns.clone()).collect();
        let delays: Vec<f64> = inputs.iter().map(|i| i.delay).collect();
        reference_delay(reference, inputs)?;
        let d =
            average_on_time_base(grid, freqs, &cols, &delays, method, reference).map_err(|e| {
                match e {
                    OpError::BadReference => MathError::BadReference,
                    _ => MathError::NoOverlap,
                }
            })?;
        return Ok(MathResult {
            columns: d.columns,
            delay: d.delay,
            phase: PhaseBasis::SharedTimeBase,
        });
    }
    if method != AverageMethod::Power {
        return Err(match no_phase {
            Some(k) => MathError::NoPhase(k),
            None => MathError::NoSharedTimeBase,
        });
    }
    // Power average of magnitude alone: no shared time base to put a phase on.
    let n = freqs.len();
    let mag: Vec<f32> = (0..n)
        .map(|i| {
            let p: Option<f64> = inputs.iter().try_fold(0.0, |s, x| {
                let v = f64::from(x.columns.mag_db[i]);
                v.is_finite().then(|| s + 10f64.powf(v / 10.0))
            });
            p.map_or(f32::NAN, |p| power_db(p / inputs.len() as f64) as f32)
        })
        .collect();
    if mag.iter().all(|v| v.is_nan()) {
        return Err(MathError::NoOverlap);
    }
    let coherence = inputs
        .iter()
        .all(|x| x.columns.coherence.is_some())
        .then(|| {
            (0..n)
                .map(|i| {
                    let s: f32 = inputs
                        .iter()
                        .map(|x| x.columns.coherence.as_ref().map_or(f32::NAN, |c| c[i]))
                        .sum();
                    s / inputs.len() as f32
                })
                .collect()
        });
    Ok(MathResult {
        columns: Columns {
            mag_db: mag,
            phase_deg: None,
            coherence,
        },
        delay: Seconds(0.0),
        phase: PhaseBasis::NoPhase,
    })
}

#[cfg(test)]
mod tests;
