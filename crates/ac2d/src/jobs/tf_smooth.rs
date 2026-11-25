//! A transfer function's display smoothing, as every `tf` frame carries it: the running job
//! smooths its averages with it, and a stopped transfer's kept result is smoothed with it
//! again when its smoothing changes, so the frame it sends then is the one the job would
//! have sent from the same averages.

use ac2_core::grid::LogGrid;
use ac2_core::smoothing::{Smoother, SmoothingMode, TfColumns};
use ac2_proto::frame::{TfFrame, ValidityMask};
use num_complex::Complex64;

use crate::conv;

/// The kernel for `smoothing` on `grid`.
pub(crate) fn smoother(
    grid: LogGrid,
    smoothing: Option<ac2_proto::model::Smoothing>,
) -> Option<(Smoother, SmoothingMode)> {
    smoothing.map(|s| {
        let (f, m) = conv::smoothing(s);
        (Smoother::new(grid, f), m)
    })
}

/// `h` (with the mic curve's gain already on it) smoothed: magnitude dB and phase degrees
/// per column. Only columns whose validity is `NONE` take part (a masked, thinned or
/// settling column would bleed into its neighbours); a column left without support is NaN.
pub(crate) fn smoothed(
    (sm, mode): (&Smoother, SmoothingMode),
    h: &[Complex64],
    coherence: &[f64],
    validity: &[ValidityMask],
) -> (Vec<f32>, Vec<f32>) {
    let valid: Vec<bool> = validity.iter().map(|v| *v == ValidityMask::NONE).collect();
    let s = sm.smooth(
        TfColumns {
            h,
            coherence,
            valid: &valid,
        },
        mode,
    );
    s.h.iter()
        .zip(&s.valid)
        .map(|(h, ok)| {
            if *ok {
                (
                    (20.0 * h.norm().log10()) as f32,
                    h.arg().to_degrees() as f32,
                )
            } else {
                (f32::NAN, f32::NAN)
            }
        })
        .unzip()
}

/// `raw` (an unsmoothed `tf`, as a capture takes it) smoothed on `grid` by its own
/// `meta.smoothing`, as the job's frame from the same averages; unchanged without one.
pub(crate) fn resmooth(raw: &TfFrame, grid: LogGrid) -> TfFrame {
    let mut out = raw.clone();
    let Some((sm, mode)) = smoother(grid, raw.meta.smoothing) else {
        return out;
    };
    let h: Vec<Complex64> = raw
        .mag
        .iter()
        .zip(&raw.phase)
        .map(|(m, p)| {
            Complex64::from_polar(10f64.powf(f64::from(*m) / 20.0), f64::from(*p).to_radians())
        })
        .collect();
    let coherence: Vec<f64> = raw.coh.iter().map(|c| f64::from(*c)).collect();
    let (mag, phase) = smoothed((&sm, mode), &h, &coherence, &raw.validity);
    out.mag = mag;
    out.phase = phase;
    out
}
