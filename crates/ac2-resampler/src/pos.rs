//! Positions in input-sample time and the step between output samples, both in 64.64 fixed
//! point.
//!
//! An `f64` absolute sample index loses sub-sample resolution as it grows: at 96 kHz its
//! spacing passes 1e-6 samples after about 3 years and a whole sample after about 3000. Fixed
//! point keeps 2⁻⁶⁴ of a sample at any index, and adding a step is integer addition, so
//! stepping a position a million times lands exactly where one multiplication puts it.

use crate::Error;

/// 2⁶⁴ as `f64`: the scale between a fraction and its fixed-point bits.
const ONE_F64: f64 = 18_446_744_073_709_551_616.0;

/// A point in input-sample time: an integer sample index and a fraction in [0, 1), held as
/// 64.64 fixed point.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos(u128);

impl Pos {
    /// The input sample `index` itself (fraction 0).
    pub const fn from_index(index: u64) -> Self {
        Self((index as u128) << 64)
    }

    /// `index + frac`. The fraction is rounded down to a multiple of 2⁻⁶⁴.
    pub fn new(index: u64, frac: f64) -> Result<Self, Error> {
        if !(0.0..1.0).contains(&frac) {
            return Err(Error::FractionOutOfRange { frac });
        }
        // frac < 1, so frac·2⁶⁴ < 2⁶⁴ and the cast cannot saturate.
        Ok(Self(
            ((index as u128) << 64) | (frac * ONE_F64) as u64 as u128,
        ))
    }

    /// The input sample at or before this position.
    pub const fn index(self) -> u64 {
        (self.0 >> 64) as u64
    }

    /// The fraction past [`index`](Self::index), in [0, 1).
    pub fn fraction(self) -> f64 {
        self.frac_bits() as f64 / ONE_F64
    }

    /// The fraction as 64-bit fixed point (units of 2⁻⁶⁴ sample).
    pub const fn frac_bits(self) -> u64 {
        self.0 as u64
    }

    /// The position `n` steps on, exactly.
    pub const fn advanced(self, step: Step, n: u64) -> Self {
        Self(self.0 + step.0 * n as u128)
    }

    pub(crate) const fn raw(self) -> u128 {
        self.0
    }
}

/// Input samples per output sample, within 1 ± [`Step::MAX_DEVIATION`], as 64.64 fixed
/// point.
///
/// Built from an `f64` the step is exact: near 1 an `f64` has a spacing of 2⁻⁵² or 2⁻⁵³,
/// both multiples of 2⁻⁶⁴.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Step(u128);

impl Step {
    /// The farthest a step may be from 1. Crystals disagree by parts per million; a thousand
    /// times that is a wrong sample rate, not drift, and the kernel's passband is only
    /// characterised for ratios this close to 1.
    pub const MAX_DEVIATION: f64 = 1e-3;

    /// One input sample per output sample.
    pub const ONE: Self = Self(1 << 64);

    /// `step` input samples per output sample; refused outside 1 ± [`Self::MAX_DEVIATION`].
    pub fn new(step: f64) -> Result<Self, Error> {
        // Bounds as the caller would write them (`1.0 - 1e-3`), not `|step − 1|`, which
        // rounds 0.999 to just over 1e-3 away.
        if !(1.0 - Self::MAX_DEVIATION..=1.0 + Self::MAX_DEVIATION).contains(&step) {
            return Err(Error::StepOutOfRange { step });
        }
        // Scaling by a power of two is exact and the product is an integer (see type docs).
        Ok(Self((step * ONE_F64) as u128))
    }

    /// The step as `f64` (exact for any step built from an `f64`).
    pub fn get(self) -> f64 {
        self.0 as f64 / ONE_F64
    }

    pub(crate) const fn raw(self) -> u128 {
        self.0
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn step_round_trips_and_refuses_out_of_range() {
        for s in [1.0, 1.0 + 1e-6, 1.0 - 1e-3, 1.0 + 1e-3, 1.0 + f64::EPSILON] {
            assert_eq!(Step::new(s).unwrap().get(), s);
        }
        for s in [1.0011, 0.9989, f64::NAN, f64::INFINITY, 2.0] {
            assert!(matches!(Step::new(s), Err(Error::StepOutOfRange { .. })));
        }
    }

    #[test]
    fn pos_accumulates_exactly_at_large_index() {
        let start = Pos::new(96_000 * 86_400 * 3650, 0.25).unwrap();
        let step = Step::new(1.0 + 3e-6).unwrap();
        let mut p = start;
        for _ in 0..100_000 {
            p = p.advanced(step, 1);
        }
        assert_eq!(p, start.advanced(step, 100_000));
        assert_eq!(p.index(), start.index() + 100_000);
        // The f64 step is 1 + 3e-6 within 2⁻⁵³, times 1e5 steps.
        assert!((p.fraction() - (0.25 + 0.3)).abs() < 1e-10);
        assert!(matches!(
            Pos::new(0, 1.0),
            Err(Error::FractionOutOfRange { .. })
        ));
        assert!(matches!(
            Pos::new(0, -0.1),
            Err(Error::FractionOutOfRange { .. })
        ));
    }
}
