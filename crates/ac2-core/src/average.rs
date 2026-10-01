//! Averaging of captured traces: power, complex and coherence-weighted (PLAN.md §3.5).
//!
//! # Common delay reference
//!
//! Every captured trace was measured with its own inserted delay `τₖ` removed: its columns
//! are `hₖ(f) = Hₖ(f)·e^{+j2πfτₖ}` where `Hₖ` is the response on the session's shared time
//! base (positive delay = measurement late, so a delay `τ` is `e^{−j2πfτ}`). Averaging the
//! `hₖ` as stored would mix phases referred to different times: two captures of the same
//! arrival with different inserted delays would cancel, and the relative arrival between
//! positions would be lost.
//!
//! So before averaging every trace is re-referred to one chosen delay `τ_ref`
//! (decision 8b: the selected trace's measured delay, or an explicit value):
//! `hₖ·e^{−j2πf(τₖ − τ_ref)} = Hₖ·e^{+j2πfτ_ref}`. The output states `τ_ref`; its phase
//! means "relative to an arrival at `τ_ref`".
//!
//! # Methods
//!
//! - [`AverageMethod::Power`]: `|H| = sqrt(mean |hₖ|²)` — position-to-position level
//!   without phase cancellation. Power has no phase, so the drawn phase is the argument of
//!   the complex mean (as in `Complex`); it is the best single phase the set supports.
//! - [`AverageMethod::Complex`]: `mean hₖ`. Arrivals that differ in time partially cancel,
//!   as they would acoustically summed.
//! - [`AverageMethod::CoherenceWeighted`]: complex mean weighted by `γ²/(1 − γ²)`. The
//!   variance of an H1 estimate is proportional to `(1 − γ²)/γ²`, so this is the
//!   inverse-variance weight; `γ²` is capped at [`MAX_WEIGHT_COHERENCE`] so a perfectly
//!   coherent column cannot take infinite weight.
//!
//! A column is valid only where **every** trace is valid. Averaging a varying subset would
//! draw steps where positions drop in and out, which would read as response features.
//! Coherence is not re-estimated: [`AveragedTf::mean_coherence`] is the plain mean of the
//! inputs, a display mask, not a coherence of the average.

use std::f64::consts::TAU;

use num_complex::Complex64;

/// Cap on γ² in the coherence weight `γ²/(1 − γ²)` (max weight 999).
pub const MAX_WEIGHT_COHERENCE: f64 = 0.999;

/// How traces are combined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AverageMethod {
    /// RMS magnitude; phase of the complex mean.
    Power,
    /// Complex mean.
    Complex,
    /// Inverse-variance (coherence) weighted complex mean.
    CoherenceWeighted,
}

/// Which delay the averaged phase is referred to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DelayReference {
    /// The delay of trace `index` in the input slice.
    Trace(usize),
    /// An explicit delay in seconds.
    Fixed(f64),
}

/// One captured transfer function on a shared frequency axis.
#[derive(Debug, Clone, Copy)]
pub struct Trace<'a> {
    /// H per column, with this trace's inserted delay removed.
    pub h: &'a [Complex64],
    /// γ² per column.
    pub coherence: &'a [f64],
    /// Column carries a usable estimate.
    pub valid: &'a [bool],
    /// Inserted delay the trace was measured with, seconds (positive = measurement late).
    pub delay_s: f64,
}

/// Averaged trace.
#[derive(Debug, Clone, PartialEq)]
pub struct AveragedTf {
    /// Averaged H, phase relative to `reference_delay_s`.
    pub h: Vec<Complex64>,
    /// Plain mean of input γ² (display mask, not an estimator).
    pub mean_coherence: Vec<f64>,
    /// Column valid in every input (and with non-zero total weight).
    pub valid: Vec<bool>,
    /// Method used.
    pub method: AverageMethod,
    /// Reference as chosen.
    pub reference: DelayReference,
    /// The delay the phase is referred to, seconds.
    pub reference_delay_s: f64,
    /// Number of traces averaged.
    pub count: usize,
}

/// Why an average could not be formed.
#[derive(Debug, Clone, PartialEq)]
pub enum AverageError {
    /// No traces given.
    NoTraces,
    /// A trace's slices differ in length from the frequency axis.
    LengthMismatch {
        /// Trace index.
        trace: usize,
    },
    /// [`DelayReference::Trace`] names a trace that is not in the input.
    ReferenceOutOfRange {
        /// Requested index.
        index: usize,
        /// Number of traces.
        count: usize,
    },
    /// Reference delay is not finite.
    NonFiniteReference,
}

impl std::fmt::Display for AverageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AverageError::NoTraces => write!(f, "no traces to average"),
            AverageError::LengthMismatch { trace } => {
                write!(f, "trace {trace} does not match the frequency axis")
            }
            AverageError::ReferenceOutOfRange { index, count } => {
                write!(f, "reference trace {index} out of range ({count} traces)")
            }
            AverageError::NonFiniteReference => write!(f, "reference delay is not finite"),
        }
    }
}

impl std::error::Error for AverageError {}

/// Average `traces` on the shared axis `freq_hz`, with phase referred to `reference`.
pub fn average(
    freq_hz: &[f64],
    traces: &[Trace<'_>],
    method: AverageMethod,
    reference: DelayReference,
) -> Result<AveragedTf, AverageError> {
    if traces.is_empty() {
        return Err(AverageError::NoTraces);
    }
    let n = freq_hz.len();
    for (k, t) in traces.iter().enumerate() {
        if t.h.len() != n || t.coherence.len() != n || t.valid.len() != n {
            return Err(AverageError::LengthMismatch { trace: k });
        }
    }
    let reference_delay_s = match reference {
        DelayReference::Trace(index) => {
            traces
                .get(index)
                .ok_or(AverageError::ReferenceOutOfRange {
                    index,
                    count: traces.len(),
                })?
                .delay_s
        }
        DelayReference::Fixed(d) => d,
    };
    if !reference_delay_s.is_finite() {
        return Err(AverageError::NonFiniteReference);
    }

    let count = traces.len() as f64;
    let mut h = Vec::with_capacity(n);
    let mut mean_coherence = Vec::with_capacity(n);
    let mut valid = Vec::with_capacity(n);
    for (i, &f) in freq_hz.iter().enumerate() {
        let column_ok = traces.iter().all(|t| {
            t.valid[i] && t.h[i].re.is_finite() && t.h[i].im.is_finite() && t.delay_s.is_finite()
        });
        let coh_mean = traces.iter().map(|t| t.coherence[i]).sum::<f64>() / count;
        mean_coherence.push(coh_mean);
        if !column_ok {
            h.push(Complex64::new(f64::NAN, f64::NAN));
            valid.push(false);
            continue;
        }
        let rereferred = traces.iter().map(|t| {
            t.h[i] * Complex64::from_polar(1.0, -TAU * f * (t.delay_s - reference_delay_s))
        });
        let value = match method {
            AverageMethod::Complex => Some(rereferred.sum::<Complex64>() / count),
            AverageMethod::Power => {
                let (sum, power) = rereferred.fold((Complex64::new(0.0, 0.0), 0.0), |(s, p), z| {
                    (s + z, p + z.norm_sqr())
                });
                Some(Complex64::from_polar((power / count).sqrt(), sum.arg()))
            }
            AverageMethod::CoherenceWeighted => {
                let (sum, wsum) = rereferred.zip(traces).fold(
                    (Complex64::new(0.0, 0.0), 0.0),
                    |(s, ws), (z, t)| {
                        let w = coherence_weight(t.coherence[i]);
                        (s + z * w, ws + w)
                    },
                );
                (wsum > 0.0).then(|| sum / wsum)
            }
        };
        match value {
            Some(z) => {
                h.push(z);
                valid.push(true);
            }
            None => {
                h.push(Complex64::new(f64::NAN, f64::NAN));
                valid.push(false);
            }
        }
    }
    Ok(AveragedTf {
        h,
        mean_coherence,
        valid,
        method,
        reference,
        reference_delay_s,
        count: traces.len(),
    })
}

/// Inverse-variance weight of an H1 estimate with coherence `gamma2`.
pub fn coherence_weight(gamma2: f64) -> f64 {
    if !gamma2.is_finite() {
        return 0.0;
    }
    let c = gamma2.clamp(0.0, MAX_WEIGHT_COHERENCE);
    c / (1.0 - c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axis() -> Vec<f64> {
        (0..200).map(|i| 20.0 * 1.035f64.powi(i)).collect()
    }

    fn response(f: &[f64]) -> Vec<Complex64> {
        f.iter()
            .map(|f| {
                let s = Complex64::new(0.0, f / 1000.0);
                Complex64::new(1.0, 0.0) / (s + Complex64::new(1.0, 0.0))
            })
            .collect()
    }

    const ALL: [AverageMethod; 3] = [
        AverageMethod::Power,
        AverageMethod::Complex,
        AverageMethod::CoherenceWeighted,
    ];

    #[test]
    fn identical_traces_average_to_identity() {
        let f = axis();
        let h = response(&f);
        let coh = vec![0.8; f.len()];
        let valid = vec![true; f.len()];
        let t = Trace {
            h: &h,
            coherence: &coh,
            valid: &valid,
            delay_s: 2.5e-3,
        };
        for m in ALL {
            let out = average(&f, &[t, t, t], m, DelayReference::Trace(1)).expect("average");
            assert_eq!(out.reference_delay_s, 2.5e-3);
            assert_eq!(out.count, 3);
            for (a, b) in out.h.iter().zip(&h) {
                assert!((a - b).norm() < 1e-12, "{m:?}: {a} vs {b}");
            }
            assert!(out.valid.iter().all(|v| *v));
            assert!(out.mean_coherence.iter().all(|c| (c - 0.8).abs() < 1e-15));
        }
    }

    /// Position A arrives at 1 ms; position B's arrival is 2 ms later. B was captured twice
    /// with different inserted delays. Averaged against A's delay, both captures of B
    /// combine without cancelling and keep B's 2 ms lag relative to A.
    #[test]
    fn different_delays_keep_relative_phase() {
        let f = axis();
        let base = response(&f);
        let delayed = |tau_true: f64, tau_inserted: f64| -> Vec<Complex64> {
            f.iter()
                .zip(&base)
                .map(|(f, h)| *h * Complex64::from_polar(1.0, -TAU * f * (tau_true - tau_inserted)))
                .collect()
        };
        let (ta, tb) = (1.0e-3, 3.0e-3);
        let a = delayed(ta, ta);
        let b1 = delayed(tb, 3.0e-3);
        let b2 = delayed(tb, 2.6e-3);
        let coh = vec![0.9; f.len()];
        let valid = vec![true; f.len()];
        let traces = [
            Trace {
                h: &a,
                coherence: &coh,
                valid: &valid,
                delay_s: ta,
            },
            Trace {
                h: &b1,
                coherence: &coh,
                valid: &valid,
                delay_s: 3.0e-3,
            },
            Trace {
                h: &b2,
                coherence: &coh,
                valid: &valid,
                delay_s: 2.6e-3,
            },
        ];
        for m in ALL {
            let out = average(&f, &traces[1..], m, DelayReference::Fixed(ta)).expect("average");
            assert_eq!(out.reference, DelayReference::Fixed(ta));
            for (i, z) in out.h.iter().enumerate() {
                let expected = base[i] * Complex64::from_polar(1.0, -TAU * f[i] * (tb - ta));
                assert!((z - expected).norm() < 1e-9, "{m:?} at {} Hz", f[i]);
            }
            // Averaging A with itself via the trace reference: phase stays on A's base.
            let solo = average(&f, &traces[..1], m, DelayReference::Trace(0)).expect("avg");
            for (z, h) in solo.h.iter().zip(&base) {
                assert!((z - h).norm() < 1e-12);
            }
        }
        // Without re-referencing (each trace on its own delay) the two captures of B would
        // disagree in phase; check that they do, so the test above is not vacuous.
        let worst = b1
            .iter()
            .zip(&b2)
            .map(|(x, y)| (x - y).norm())
            .fold(0.0, f64::max);
        assert!(worst > 1.0);
    }

    #[test]
    fn power_average_is_rms_magnitude() {
        let f = axis();
        let one = vec![Complex64::new(1.0, 0.0); f.len()];
        let three = vec![Complex64::new(0.0, 3.0); f.len()];
        let coh = vec![1.0; f.len()];
        let valid = vec![true; f.len()];
        let t = |h| Trace {
            h,
            coherence: &coh,
            valid: &valid,
            delay_s: 0.0,
        };
        let out = average(
            &f,
            &[t(&one), t(&three)],
            AverageMethod::Power,
            DelayReference::Fixed(0.0),
        )
        .expect("average");
        for z in &out.h {
            assert!((z.norm() - 5f64.sqrt()).abs() < 1e-12);
            assert!((z.arg() - 3f64.atan2(1.0)).abs() < 1e-12);
        }
    }

    #[test]
    fn coherence_weighting_favours_coherent_trace() {
        let f = axis();
        let good = vec![Complex64::new(1.0, 0.0); f.len()];
        let bad = vec![Complex64::new(0.0, 0.0); f.len()];
        let c_good = vec![0.99; f.len()];
        let c_bad = vec![0.5; f.len()];
        let valid = vec![true; f.len()];
        let out = average(
            &f,
            &[
                Trace {
                    h: &good,
                    coherence: &c_good,
                    valid: &valid,
                    delay_s: 0.0,
                },
                Trace {
                    h: &bad,
                    coherence: &c_bad,
                    valid: &valid,
                    delay_s: 0.0,
                },
            ],
            AverageMethod::CoherenceWeighted,
            DelayReference::Trace(0),
        )
        .expect("average");
        // Weights 99 and 1.
        for z in &out.h {
            assert!((z.re - 0.99).abs() < 1e-12 && z.im.abs() < 1e-15);
        }
    }

    #[test]
    fn invalid_in_any_trace_invalidates_column() {
        let f = axis();
        let h = response(&f);
        let coh = vec![0.9; f.len()];
        let all = vec![true; f.len()];
        let mut gap = all.clone();
        gap[10] = false;
        let out = average(
            &f,
            &[
                Trace {
                    h: &h,
                    coherence: &coh,
                    valid: &all,
                    delay_s: 0.0,
                },
                Trace {
                    h: &h,
                    coherence: &coh,
                    valid: &gap,
                    delay_s: 0.0,
                },
            ],
            AverageMethod::Complex,
            DelayReference::Trace(0),
        )
        .expect("average");
        assert!(!out.valid[10] && out.valid[9] && out.valid[11]);
    }

    #[test]
    fn errors() {
        let f = axis();
        let h = response(&f);
        let coh = vec![0.9; f.len()];
        let valid = vec![true; f.len()];
        let t = Trace {
            h: &h,
            coherence: &coh,
            valid: &valid,
            delay_s: 0.0,
        };
        assert_eq!(
            average(&f, &[], AverageMethod::Power, DelayReference::Fixed(0.0)),
            Err(AverageError::NoTraces)
        );
        assert_eq!(
            average(&f, &[t], AverageMethod::Power, DelayReference::Trace(1)),
            Err(AverageError::ReferenceOutOfRange { index: 1, count: 1 })
        );
        assert_eq!(
            average(
                &f[1..],
                &[t],
                AverageMethod::Power,
                DelayReference::Trace(0)
            ),
            Err(AverageError::LengthMismatch { trace: 0 })
        );
    }
}
