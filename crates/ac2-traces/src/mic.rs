//! Mic curves applied to stored traces after capture (`docs/design/q7-calibration.md` §9).
//!
//! The curve is a display edit, like smoothing: the stored columns stay as measured, and
//! the correction (the curve normalised to 0 dB at `f_norm`) is subtracted from the
//! magnitude when the trace is served, at the same places the live jobs subtract it — per
//! log-grid column for transfer and sweep traces, per bin for spectra, as the band power
//! average for RTA bands. Phase and coherence are never touched (decision 7c).

pub use ac2_core::mic_curve::Correction;
use ac2_core::mic_curve::MicCurve;
use ac2_proto::GridDef;
use ac2_proto::model::{BandFraction, DistortionCurve, MicState, SweepData, TraceKind, TraceMeta};

use crate::columns::{StoredTrace, frequencies};

/// Why a mic curve cannot be applied to (or removed from) a trace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MicCurveError {
    /// Targets are specified curves, not measured through a mic.
    Target,
    /// The trace's columns carry a curve from its capture already.
    InColumns {
        /// That curve's name.
        curve: String,
    },
    /// Locked traces keep their curve.
    Locked,
}

impl std::fmt::Display for MicCurveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Target => {
                f.write_str("a target is a specified curve; mic curves apply to measured traces")
            }
            Self::InColumns { curve } => write!(
                f,
                "captured with mic curve {curve:?} applied: it is in the columns already, \
                 a second curve would correct twice"
            ),
            Self::Locked => f.write_str("the trace is locked; unlock it to change its curve"),
        }
    }
}

impl std::error::Error for MicCurveError {}

/// Whether the mic curve of `meta` may change.
pub fn check(meta: &TraceMeta) -> Result<(), MicCurveError> {
    if meta.kind == TraceKind::Target {
        return Err(MicCurveError::Target);
    }
    if let Some(curve) = meta.mic.as_ref().and_then(|m| m.curve.as_ref()) {
        return Err(MicCurveError::InColumns {
            curve: curve.label.clone(),
        });
    }
    if meta.edit.locked {
        return Err(MicCurveError::Locked);
    }
    Ok(())
}

/// The correction of the curve through `points` (Hz, dB), normalised to 0 dB at `f_norm`.
pub fn correction(points: &[[f64; 2]], f_norm: f64) -> Result<Correction, String> {
    let pts: Vec<(f64, f64)> = points.iter().map(|p| (p[0], p[1])).collect();
    MicCurve::from_points(&pts)
        .map(|c| c.normalised(f_norm))
        .map_err(|e| e.to_string())
}

/// The curve's points (Hz, dB), as a session stores them.
pub fn points(k: &Correction) -> Vec<[f64; 2]> {
    let c = k.curve();
    c.freqs()
        .iter()
        .zip(c.gains())
        .map(|(f, g)| [*f, *g])
        .collect()
}

fn core_fraction(f: BandFraction) -> ac2_core::rta::BandFraction {
    use ac2_core::rta::BandFraction as C;
    match f {
        BandFraction::Octave => C::Octave,
        BandFraction::Third => C::Third,
        BandFraction::Sixth => C::Sixth,
        BandFraction::Twelfth => C::Twelfth,
        BandFraction::TwentyFourth => C::TwentyFourth,
    }
}

/// dB the correction subtracts in each column of `grid`: at the column frequency on log
/// and linear grids; on IEC bands the power average over the band, which is what a band
/// power of pink noise through the mic needs and what the live RTA subtracts.
pub fn column_db(grid: &GridDef, k: &Correction) -> Vec<f64> {
    match grid {
        GridDef::IecBands { fraction, centres } => centres
            .iter()
            .map(|c| {
                let (lo, hi) = core_fraction(*fraction).edges(c.0);
                k.band_db(lo, hi)
            })
            .collect(),
        _ => frequencies(grid).iter().map(|f| k.db(*f)).collect(),
    }
}

/// Subtracts the correction from `mag_db` (on `grid`); gaps stay gaps.
pub fn correct(grid: &GridDef, mag_db: &mut [f32], k: &Correction) {
    for (v, d) in mag_db.iter_mut().zip(column_db(grid, k)) {
        *v = (f64::from(*v) - d) as f32;
    }
}

/// A sweep's distortion through a corrected mic. Order `n` at fundamental `f` is the
/// ratio of what the mic picked up at `n·f` to what it picked up at `f`, so its true
/// level is the measured one − (c(n·f) − c(f)); the floor (noise around `n·f`, re the
/// same fundamental) shifts the same way. THD is re-summed from the corrected orders as
/// the analysis sums them: the power sum of the orders measured in that column.
pub fn correct_sweep(grid: &GridDef, s: &SweepData, k: &Correction) -> SweepData {
    let f = frequencies(grid);
    let mut out = s.clone();
    for h in &mut out.harmonics {
        let n = f64::from(h.order);
        for (i, fi) in f.iter().enumerate() {
            let d = k.db(n * fi) - k.db(*fi);
            for v in [h.curve.level_db.get_mut(i), h.curve.floor_db.get_mut(i)]
                .into_iter()
                .flatten()
            {
                *v = (f64::from(*v) - d) as f32;
            }
        }
    }
    let sum = |pick: &dyn Fn(&DistortionCurve) -> Option<f32>| {
        let p: Vec<f64> = out
            .harmonics
            .iter()
            .filter_map(|h| pick(&h.curve))
            .filter(|v| !v.is_nan())
            .map(|v| 10f64.powf(f64::from(v) / 10.0))
            .collect();
        (!p.is_empty()).then(|| (10.0 * p.iter().sum::<f64>().log10()) as f32)
    };
    for i in 0..f.len() {
        if s.thd.level_db.get(i).is_some_and(|v| !v.is_nan())
            && let Some(v) = sum(&|c| c.level_db.get(i).copied())
        {
            out.thd.level_db[i] = v;
        }
        if s.thd.floor_db.get(i).is_some_and(|v| !v.is_nan())
            && let Some(v) = sum(&|c| c.floor_db.get(i).copied())
        {
            out.thd.floor_db[i] = v;
        }
    }
    out
}

/// The trace with its applied curve moved into the columns, as if it had been captured
/// with it: what averages and A−B combine, so a derived trace holds corrected columns and
/// says so in its `mic`.
pub fn bake(t: &StoredTrace) -> StoredTrace {
    let (Some(k), Some(mc)) = (&t.mic_curve, &t.meta.mic_curve) else {
        return t.clone();
    };
    let mut b = t.clone();
    correct(&t.grid, &mut b.columns.mag_db, k);
    b.sweep = t.display_sweep();
    b.meta.mic = Some(MicState {
        name: mc.mic.clone(),
        curve: Some(mc.curve.clone()),
    });
    b.meta.mic_curve = None;
    b.mic_curve = None;
    b
}
