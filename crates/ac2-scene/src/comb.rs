//! What a curve says about the comb two like landmarks would make: the cursor mark reads a
//! delay from the spacing of two peaks (or two notches) only when the curve resolves combs
//! that fine and repeats the spacing, since any two ripples of a room curve have a spacing.

use crate::axis::Range;
use crate::cursor_jump::{Landmark, landmark_at, landmarks};
use crate::spectrum::peaks::COMB_TOLERANCE;
use ac2_proto::model::SmoothingFraction;

/// The evidence for the comb two like landmarks space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Comb {
    /// A third landmark of the same kind one spacing beyond either end: the spacing repeats,
    /// as a comb's does, and names its delay.
    Confirmed,
    /// No third landmark of this kind one spacing beyond either end: two neighbours of
    /// uneven ripple, which say nothing about one delay.
    Uneven(Landmark),
    /// The smoothing averages away any comb finer than `below_hz` where the pair is: the
    /// teeth seen are not a comb's to read.
    Smoothed {
        fraction: SmoothingFraction,
        below_hz: f64,
    },
}

/// One end of the pair: the column its landmark is read at, and the frequency the
/// difference is read from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct End {
    pub col: usize,
    pub hz: f64,
}

/// The narrowest comb spacing a `1/b`-octave smoothing leaves readable at `hz`: twice its
/// bandwidth there, `hz · (2^(1/2b) − 2^(−1/2b))`.
///
/// Why twice: the kernel is Hann in log frequency, 1.5 bands wide for the noise of a
/// one-band average (`ac2_core::smoothing`). Its response to a ripple whose period spans
/// two bands keeps about 70 % of the swing; at 1.5 bands a half, at one band a sixth, and
/// at 0.75 bands nothing. Below two bands what is left of a comb's teeth is mostly the
/// kernel's own shape, so their spacing no longer stands for a delay.
pub fn smoothing_limit_hz(fraction: SmoothingFraction, hz: f64) -> f64 {
    let h = 1.0 / (2.0 * f64::from(fraction.b()));
    2.0 * hz * (2f64.powf(h) - 2f64.powf(-h))
}

/// The comb `mark` and `cursor` space on `values` (dB, column-aligned with ascending
/// `freqs`), its landmarks found as the cursor jumps find them (`shown`, `prominence_db`),
/// drawn under `smoothing`; `None` unless both ends are peaks or both notches on distinct
/// columns, as only like ends sit a whole spacing apart.
///
/// The smoothing is asked first: a comb it hides has no third tooth to find either.
pub fn evidence(
    freqs: &[f64],
    values: &[f64],
    shown: Range,
    prominence_db: f64,
    smoothing: Option<SmoothingFraction>,
    mark: End,
    cursor: End,
) -> Option<Comb> {
    if mark.col == cursor.col {
        return None;
    }
    let kind = landmark_at(freqs, values, shown, mark.col, prominence_db)?;
    if landmark_at(freqs, values, shown, cursor.col, prominence_db) != Some(kind) {
        return None;
    }
    let (lo, hi) = (mark.hz.min(cursor.hz), mark.hz.max(cursor.hz));
    let df = hi - lo;
    if !(df.is_finite() && df > 0.0) {
        return None;
    }
    // The bandwidth grows with frequency: the pair's upper end is where the smoothing is
    // widest, and both teeth must be resolved.
    if let Some(fraction) = smoothing {
        let below_hz = smoothing_limit_hz(fraction, hi);
        if df < below_hz {
            return Some(Comb::Smoothed { fraction, below_hz });
        }
    }
    let tol = COMB_TOLERANCE * df;
    let third = landmarks(freqs, values, shown, kind, prominence_db)
        .into_iter()
        .any(|x| (x - (hi + df)).abs() <= tol || (x - (lo - df)).abs() <= tol);
    Some(if third {
        Comb::Confirmed
    } else {
        Comb::Uneven(kind)
    })
}

#[cfg(test)]
mod tests;
