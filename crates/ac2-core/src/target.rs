//! Generated target curves: a straight tilt about a pivot with an optional low-frequency
//! shelf, and the perceived-balance shift between two loudness levels ([`iso226`]).
//! Magnitudes in dB at the frequencies asked for; no grid of their own.

use crate::eq::{Pass, Realisation, Section, ShelfFreq, ShelfOrder, ShelfSlope, response};

pub mod iso226;
#[cfg(test)]
mod tests;

/// A house tilt: `slope · log₂(f / pivot)` dB, a straight line on a log-frequency axis, 0 dB
/// at the pivot. Octaves (not decades) because tilts are specified and read per octave.
pub fn tilt_db(f: f64, slope_db_per_oct: f64, pivot_hz: f64) -> f64 {
    slope_db_per_oct * (f / pivot_hz).log2()
}

/// A low-frequency shelf of `gain_db` whose gain is half at `corner_hz` (in dB): the
/// second-order cookbook low shelf at slope S = 1, evaluated on its analog prototype.
///
/// That shape because it is the steepest second-order shelf without overshoot (monotonic,
/// so the target never asks for a bump or dip the operator did not specify), its 12 dB/oct
/// transition is gentle enough to be smooth across the two octaves around the corner, and
/// it is exactly the shelf a processor's (and the EQ what-if's) S = 1 low shelf makes, so
/// the target is reachable with one filter. The analog prototype: a target names a
/// response, not a sample rate. `None` when the EQ model refuses the gain
/// ([`crate::eq::MAX_GAIN_DB`]) or the corner is not a positive frequency.
pub fn lf_shelf_db(freqs: &[f64], gain_db: f64, corner_hz: f64) -> Option<Vec<f64>> {
    if !(corner_hz.is_finite() && corner_hz > 0.0) {
        return None;
    }
    if gain_db == 0.0 {
        return Some(vec![0.0; freqs.len()]);
    }
    let shelf = Section::Shelf {
        pass: Pass::Low,
        f: corner_hz,
        gain_db,
        order: ShelfOrder::Second(ShelfSlope::S(1.0)),
        f_at: ShelfFreq::Midpoint,
    };
    let h = response(&[shelf], Realisation::Analog, freqs).ok()?;
    Some(h.iter().map(|z| 20.0 * z.norm().log10()).collect())
}
