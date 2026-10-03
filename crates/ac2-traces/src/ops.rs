//! Capture, averaging and A−B math on stored traces.
//!
//! # Time base
//!
//! A captured transfer function is stored with its own inserted delay `τₖ` removed
//! (`hₖ = Hₖ·e^{+j2πfτₖ}`, see `ac2_core::average`). Traces captured in the same session
//! epoch share one time reference, so their phases can be re-referred to a common delay and
//! combined. Every other trace (imported, averaged, math, another epoch) has only its own
//! alignment (decision 8a): combining its phase with another trace's would invent a
//! relative arrival nobody measured, so operations that need phase refuse it, and power
//! averaging keeps the magnitude only.
//!
//! Display edits (offset, polarity, nudge) are not applied: operations work on the columns
//! as measured, and the result gets fresh edits.

use std::fmt;

use ac2_core::average as core;
use ac2_proto::FrameData;
use ac2_proto::GridDef;
use ac2_proto::frame::ValidityMask;
use ac2_proto::model::{AverageMethod, DelayReference, MathOp, TraceKind, TraceSource};
use ac2_proto::units::{Seconds, SessionEpoch, TraceId};
use num_complex::Complex64;

use crate::columns::{Columns, StoredTrace, frequencies, resample, wrap_deg};

/// Why an operation was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum OpError {
    /// Averaging needs at least two traces.
    TooFewTraces,
    /// A trace is listed twice.
    Duplicate(TraceId),
    /// The kinds cannot be combined (transfer with spectrum, spectrum with RTA, …).
    MixedKinds,
    /// This kind cannot take part (targets are not averaged; spectra are not divided).
    KindNotSupported(TraceKind),
    /// Spectra / RTA bands on different grids (band power does not interpolate).
    GridMismatch,
    /// The method combines phase, but the traces have no shared time base.
    NoSharedTimeBase,
    /// A trace without phase where the operation needs it.
    NoPhase(TraceId),
    /// Coherence weighting with a trace that has no coherence.
    NoCoherence(TraceId),
    /// The phase reference trace is not one of the inputs.
    ReferenceNotInput(TraceId),
    /// The reference delay is not finite.
    BadReference,
    /// Spectrum / RTA traces average on power only.
    PowerOnly,
    /// The grids do not overlap: no column has values from every trace.
    NoOverlap,
}

impl fmt::Display for OpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooFewTraces => write!(f, "averaging needs at least two traces"),
            Self::Duplicate(t) => write!(f, "trace {t} is listed twice"),
            Self::MixedKinds => write!(
                f,
                "these traces cannot be combined (transfer and target combine with each other; spectra and RTA only with their own kind and scale)"
            ),
            Self::KindNotSupported(k) => write!(f, "not supported for {} traces", kind_name(*k)),
            Self::GridMismatch => write!(
                f,
                "spectrum / RTA traces must share one grid (band powers are not interpolated)"
            ),
            Self::NoSharedTimeBase => write!(
                f,
                "complex and coherence-weighted averages combine phase, which needs traces captured in the same session epoch; use power"
            ),
            Self::NoPhase(t) => write!(f, "trace {t} has no phase"),
            Self::NoCoherence(t) => write!(f, "trace {t} has no coherence to weight by"),
            Self::ReferenceNotInput(t) => {
                write!(f, "the phase reference trace {t} is not one of the inputs")
            }
            Self::BadReference => write!(f, "the reference delay must be finite"),
            Self::PowerOnly => write!(f, "spectrum and RTA traces average on power only"),
            Self::NoOverlap => write!(f, "the traces share no frequency range"),
        }
    }
}

impl std::error::Error for OpError {}

/// Lower-case name of a kind for messages.
pub fn kind_name(k: TraceKind) -> &'static str {
    match k {
        TraceKind::Transfer => "transfer",
        TraceKind::Target => "target",
        TraceKind::Spectrum { .. } => "spectrum",
        TraceKind::Rta { .. } => "RTA",
        TraceKind::Sweep => "sweep",
    }
}

/// Transfer functions and sweep responses (a sweep's fundamental is a transfer function on
/// the same time base) combine with each other.
pub fn transfer_like(k: TraceKind) -> bool {
    matches!(k, TraceKind::Transfer | TraceKind::Sweep)
}

/// Columns of a new trace made by an operation.
#[derive(Debug, Clone, PartialEq)]
pub struct Derived {
    /// Kind of the result.
    pub kind: TraceKind,
    /// Grid of the result.
    pub grid: GridDef,
    /// Data.
    pub columns: Columns,
    /// Delay the result's phase is referred to.
    pub delay: Seconds,
}

/// What a trace stores of a published frame: transfer (magnitude, phase, coherence),
/// spectrum or RTA level. A column whose validity mask is not clear is stored as NaN, so a
/// gap stays a gap. `None` for frame kinds that are not traces.
pub fn capture_columns(data: &FrameData) -> Option<(TraceKind, Columns)> {
    let gate = |v: &[f32], m: &[ValidityMask]| -> Vec<f32> {
        v.iter()
            .enumerate()
            .map(|(i, x)| match m.get(i) {
                Some(mask) if *mask == ValidityMask::NONE => *x,
                _ => f32::NAN,
            })
            .collect()
    };
    match data {
        FrameData::Tf(f) => Some((
            TraceKind::Transfer,
            Columns {
                mag_db: gate(&f.mag, &f.validity),
                phase_deg: Some(gate(&f.phase, &f.validity)),
                coherence: Some(gate(&f.coh, &f.validity)),
            },
        )),
        FrameData::Spec(f) => Some((
            TraceKind::Spectrum {
                scale: f.meta.scale,
            },
            Columns {
                mag_db: gate(&f.level, &f.validity),
                phase_deg: None,
                coherence: None,
            },
        )),
        FrameData::Rta(f) => Some((
            TraceKind::Rta {
                scale: f.meta.scale,
            },
            Columns {
                mag_db: gate(&f.level, &f.validity),
                phase_deg: None,
                coherence: None,
            },
        )),
        _ => None,
    }
}

/// The session epoch whose time base a trace's phase is in, if any.
pub fn shared_epoch(t: &StoredTrace) -> Option<SessionEpoch> {
    match t.meta.source {
        TraceSource::Captured { epoch, .. } | TraceSource::IrCapture { epoch, .. } => Some(epoch),
        _ => None,
    }
}

fn all_shared(ts: &[&StoredTrace]) -> bool {
    let first = ts.first().and_then(|t| shared_epoch(t));
    first.is_some() && ts.iter().all(|t| shared_epoch(t) == first)
}

/// `t`'s columns on `grid` (resampled when its own grid differs).
fn on_grid(t: &StoredTrace, grid: &GridDef, freqs: &[f64]) -> Columns {
    if t.grid.id() == grid.id() {
        t.columns.clone()
    } else {
        resample(&frequencies(&t.grid), &t.columns, freqs)
    }
}

fn f64s(v: &[f32]) -> Vec<f64> {
    v.iter().map(|x| f64::from(*x)).collect()
}

fn f32s(v: impl IntoIterator<Item = f64>) -> Vec<f32> {
    v.into_iter().map(|x| x as f32).collect()
}

/// Averages `traces` into a new trace on the first trace's grid.
///
/// Transfer traces: power, complex or coherence-weighted via `ac2_core::average`, phase
/// referred to `reference` (decision 8b) when all inputs share a time base; without one,
/// only power averaging is possible and the result has no phase. Spectrum / RTA traces:
/// power mean of the levels on one shared grid.
pub fn average(
    traces: &[&StoredTrace],
    method: AverageMethod,
    reference: DelayReference,
) -> Result<Derived, OpError> {
    if traces.len() < 2 {
        return Err(OpError::TooFewTraces);
    }
    // A mic curve applied after capture corrects what was measured: combine corrected
    // columns.
    let baked: Vec<StoredTrace> = traces.iter().map(|t| crate::mic::bake(t)).collect();
    let traces: Vec<&StoredTrace> = baked.iter().collect();
    let traces = traces.as_slice();
    for (i, t) in traces.iter().enumerate() {
        if traces[..i].iter().any(|u| u.meta.id == t.meta.id) {
            return Err(OpError::Duplicate(t.meta.id));
        }
    }
    let kind = traces[0].meta.kind;
    if traces
        .iter()
        .any(|t| t.meta.kind != kind && !(transfer_like(t.meta.kind) && transfer_like(kind)))
    {
        return Err(OpError::MixedKinds);
    }
    let grid = traces[0].grid.clone();
    let freqs = frequencies(&grid);
    match kind {
        TraceKind::Target => Err(OpError::KindNotSupported(kind)),
        TraceKind::Spectrum { .. } | TraceKind::Rta { .. } => {
            if method != AverageMethod::Power {
                return Err(OpError::PowerOnly);
            }
            if traces.iter().any(|t| t.grid.id() != grid.id()) {
                return Err(OpError::GridMismatch);
            }
            let n = freqs.len();
            let mag = (0..n).map(|i| {
                let p: Option<f64> = traces.iter().try_fold(0.0, |s, t| {
                    let v = f64::from(*t.columns.mag_db.get(i)?);
                    v.is_finite().then(|| s + 10f64.powf(v / 10.0))
                });
                p.map_or(f64::NAN, |p| 10.0 * (p / traces.len() as f64).log10())
            });
            Ok(Derived {
                kind,
                grid,
                columns: Columns {
                    mag_db: f32s(mag),
                    phase_deg: None,
                    coherence: None,
                },
                delay: Seconds(0.0),
            })
        }
        TraceKind::Transfer | TraceKind::Sweep => {
            average_tf(traces, &grid, &freqs, method, reference)
        }
    }
}

fn average_tf(
    traces: &[&StoredTrace],
    grid: &GridDef,
    freqs: &[f64],
    method: AverageMethod,
    reference: DelayReference,
) -> Result<Derived, OpError> {
    let shared = all_shared(traces);
    if !shared && method != AverageMethod::Power {
        return Err(OpError::NoSharedTimeBase);
    }
    let cols: Vec<Columns> = traces.iter().map(|t| on_grid(t, grid, freqs)).collect();
    if method == AverageMethod::CoherenceWeighted
        && let Some(t) = traces
            .iter()
            .zip(&cols)
            .find(|(_, c)| c.coherence.is_none())
    {
        return Err(OpError::NoCoherence(t.0.meta.id));
    }
    let missing_phase = traces
        .iter()
        .zip(&cols)
        .find(|(_, c)| c.phase_deg.is_none());
    if method != AverageMethod::Power
        && let Some((t, _)) = missing_phase
    {
        return Err(OpError::NoPhase(t.meta.id));
    }
    let with_coherence = cols.iter().all(|c| c.coherence.is_some());
    let mean_coh = |i: usize| -> f64 {
        cols.iter()
            .map(|c| c.coherence.as_ref().map_or(f64::NAN, |k| f64::from(k[i])))
            .sum::<f64>()
            / cols.len() as f64
    };
    let n = freqs.len();
    if !shared || missing_phase.is_some() {
        // Power average of magnitude alone: no time base to put a phase on.
        let mag = (0..n).map(|i| {
            let p: Option<f64> = cols.iter().try_fold(0.0, |s, c| {
                let v = f64::from(c.mag_db[i]);
                v.is_finite().then(|| s + 10f64.powf(v / 10.0))
            });
            p.map_or(f64::NAN, |p| 10.0 * (p / cols.len() as f64).log10())
        });
        let mag = f32s(mag);
        if mag.iter().all(|v| v.is_nan()) {
            return Err(OpError::NoOverlap);
        }
        return Ok(Derived {
            kind: TraceKind::Transfer,
            grid: grid.clone(),
            columns: Columns {
                mag_db: mag,
                phase_deg: None,
                coherence: with_coherence.then(|| f32s((0..n).map(mean_coh))),
            },
            delay: Seconds(0.0),
        });
    }

    let core_ref = match reference {
        DelayReference::Trace { trace } => core::DelayReference::Trace(
            traces
                .iter()
                .position(|t| t.meta.id == trace)
                .ok_or(OpError::ReferenceNotInput(trace))?,
        ),
        DelayReference::Fixed { delay } => {
            if !delay.0.is_finite() {
                return Err(OpError::BadReference);
            }
            core::DelayReference::Fixed(delay.0)
        }
    };
    let h: Vec<Vec<Complex64>> = cols
        .iter()
        .map(|c| {
            let p = c.phase_deg.as_deref().unwrap_or(&[]);
            c.mag_db
                .iter()
                .zip(p)
                .map(|(m, p)| {
                    Complex64::from_polar(
                        10f64.powf(f64::from(*m) / 20.0),
                        f64::from(*p).to_radians(),
                    )
                })
                .collect()
        })
        .collect();
    let coh: Vec<Vec<f64>> = cols
        .iter()
        .map(|c| c.coherence.as_deref().map_or(vec![f64::NAN; n], f64s))
        .collect();
    let valid: Vec<Vec<bool>> = h
        .iter()
        .map(|h| {
            h.iter()
                .map(|z| z.re.is_finite() && z.im.is_finite())
                .collect()
        })
        .collect();
    let inputs: Vec<core::Trace<'_>> = traces
        .iter()
        .enumerate()
        .map(|(k, t)| core::Trace {
            h: &h[k],
            coherence: &coh[k],
            valid: &valid[k],
            delay_s: t.meta.delay.0,
        })
        .collect();
    let core_method = match method {
        AverageMethod::Power => core::AverageMethod::Power,
        AverageMethod::Complex => core::AverageMethod::Complex,
        AverageMethod::CoherenceWeighted => core::AverageMethod::CoherenceWeighted,
    };
    let a = core::average(freqs, &inputs, core_method, core_ref).map_err(|e| match e {
        core::AverageError::NonFiniteReference => OpError::BadReference,
        _ => OpError::NoOverlap,
    })?;
    if !a.valid.iter().any(|v| *v) {
        return Err(OpError::NoOverlap);
    }
    let (mag, phase): (Vec<f32>, Vec<f32>) =
        a.h.iter()
            .zip(&a.valid)
            .map(|(z, ok)| {
                if *ok {
                    (
                        (20.0 * z.norm().log10()) as f32,
                        z.arg().to_degrees() as f32,
                    )
                } else {
                    (f32::NAN, f32::NAN)
                }
            })
            .unzip();
    Ok(Derived {
        kind: TraceKind::Transfer,
        grid: grid.clone(),
        columns: Columns {
            mag_db: mag,
            phase_deg: Some(phase),
            coherence: with_coherence.then(|| f32s(a.mean_coherence.iter().copied())),
        },
        delay: Seconds(a.reference_delay_s),
    })
}

/// A − B on A's grid (B resampled when needed).
///
/// `magnitude_difference`: dB subtraction; the result has no phase. `complex_division`:
/// A / B as complex values; with a shared time base B's phase is first re-referred to A's
/// delay, so the result shows A's arrival relative to B; otherwise each keeps its own
/// alignment. Transfer and target traces combine with each other; spectra and RTA only
/// with their own kind on the same grid, and only by magnitude.
pub fn math(a: &StoredTrace, b: &StoredTrace, op: MathOp) -> Result<Derived, OpError> {
    // Applied mic curves are part of what each side measured.
    let (a, b) = (&crate::mic::bake(a), &crate::mic::bake(b));
    let relative = |k: TraceKind| transfer_like(k) || k == TraceKind::Target;
    let (ka, kb) = (a.meta.kind, b.meta.kind);
    if !(relative(ka) && relative(kb)) && ka != kb {
        return Err(OpError::MixedKinds);
    }
    if !relative(ka) {
        if op == MathOp::ComplexDivision {
            return Err(OpError::KindNotSupported(ka));
        }
        if a.grid.id() != b.grid.id() {
            return Err(OpError::GridMismatch);
        }
    }
    let freqs = frequencies(&a.grid);
    let bc = on_grid(b, &a.grid, &freqs);
    let n = freqs.len();
    let mag: Vec<f32> = (0..n).map(|i| a.columns.mag_db[i] - bc.mag_db[i]).collect();
    if mag.iter().all(|v| v.is_nan()) {
        return Err(OpError::NoOverlap);
    }
    let phase = match op {
        MathOp::MagnitudeDifference => None,
        MathOp::ComplexDivision => {
            let pa = a
                .columns
                .phase_deg
                .as_ref()
                .ok_or(OpError::NoPhase(a.meta.id))?;
            let pb = bc.phase_deg.as_ref().ok_or(OpError::NoPhase(b.meta.id))?;
            let shift = if all_shared(&[a, b]) {
                a.meta.delay.0 - b.meta.delay.0
            } else {
                0.0
            };
            Some(
                (0..n)
                    .map(|i| {
                        let d = f64::from(pa[i]) - f64::from(pb[i]) - 360.0 * freqs[i] * shift;
                        if d.is_finite() {
                            wrap_deg(d) as f32
                        } else {
                            f32::NAN
                        }
                    })
                    .collect(),
            )
        }
    };
    Ok(Derived {
        // A difference of levels is a relative dB curve, whatever it was made from.
        kind: TraceKind::Transfer,
        grid: a.grid.clone(),
        columns: Columns {
            mag_db: mag,
            phase_deg: phase,
            coherence: None,
        },
        delay: Seconds(0.0),
    })
}
