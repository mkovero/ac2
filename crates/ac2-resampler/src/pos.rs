//! Positions in input-sample time and the step between output samples, both in 64.64 fixed
//! point.
//!
//! An `f64` absolute sample index loses sub-sample resolution as it grows: its spacing is
//! 2⁻²⁰ ≈ 1e-6 samples from 2³² samples on (12.4 hours at 96 kHz) and a whole sample from 2⁵³
//! on (about 3000 years). Fixed point keeps 2⁻⁶⁴ of a sample at any index, and adding a step
//! is integer addition, so stepping a position a million times lands exactly where one
//! multiplication puts it.

use crate::Error;

/// 2⁶⁴ as `f64`: the scale between a fraction and its fixed-point bits.
const ONE_F64: f64 = 18_446_744_073_709_551_616.0;

/// 2⁻⁵³: a 53-bit integer times this is a fraction in [0, 1) with every bit kept.
const TWO_POW_MINUS_53: f64 = 1.0 / 9_007_199_254_740_992.0;

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

    /// The fraction past [`index`](Self::index), in [0, 1), truncated to 53 bits.
    pub fn fraction(self) -> f64 {
        // Dividing all 64 bits by 2⁶⁴ would round the largest fractions up to exactly 1.0;
        // keeping the top 53 truncates instead, and 53 bits are exact in an f64.
        (self.frac_bits() >> 11) as f64 * TWO_POW_MINUS_53
    }

    /// The fraction as 64-bit fixed point (units of 2⁻⁶⁴ sample).
    pub const fn frac_bits(self) -> u64 {
        self.0 as u64
    }

    /// The position `n` steps on, exactly; `None` past the last representable position
    /// (index 2⁶⁴ − 1).
    pub fn advanced(self, step: Step, n: u64) -> Option<Self> {
        step.0
            .checked_mul(u128::from(n))
            .and_then(|d| self.0.checked_add(d))
            .map(Self)
    }

    /// `self − other` in input samples: the exact 64.64 difference rounded once to the
    /// nearest `f64` (exact while it fits in 53 significant bits, e.g. any difference under
    /// 2⁻¹¹ sample, or under 2⁴² samples with no bits below 2⁻¹¹).
    pub fn diff(self, other: Self) -> f64 {
        // u128 → f64 rounds to nearest; dividing by 2⁶⁴ is then exact.
        if self.0 >= other.0 {
            (self.0 - other.0) as f64 / ONE_F64
        } else {
            -((other.0 - self.0) as f64 / ONE_F64)
        }
    }

    /// The position `samples` input samples later (earlier when negative), the shift rounded
    /// to the nearest 2⁻⁶⁴ sample. `None` when the result would fall before index 0 or past
    /// the last representable position, or `samples` is not finite.
    pub fn shifted(self, samples: f64) -> Option<Self> {
        // Scaling by 2⁶⁴ is exact. From 2⁻¹¹ sample on the scaled value is already an integer
        // (53 significant bits reach down to 2⁻⁶⁴ at most), so `round` only acts on shifts
        // smaller than that.
        let bits = (samples.abs() * ONE_F64).round();
        if bits.is_nan() || bits >= ONE_F64 * ONE_F64 {
            return None;
        }
        let bits = bits as u128;
        if samples >= 0.0 {
            self.0.checked_add(bits)
        } else {
            self.0.checked_sub(bits)
        }
        .map(Self)
    }

    /// One step on. Only for positions [`Resampler::available`](crate::Resampler::available)
    /// has bounded: those are below index 2⁶⁴ − 2 (the input count minus a lookahead of at
    /// least 1), and a step is under 2 samples, so the sum stays below 2¹²⁸.
    pub(crate) const fn stepped(self, step: Step) -> Self {
        Self(self.0 + step.0)
    }

    pub(crate) const fn raw(self) -> u128 {
        self.0
    }

    pub(crate) const fn from_raw(raw: u128) -> Self {
        Self(raw)
    }
}

/// Input samples per output sample, within 1 ± [`Step::MAX_DEVIATION`], as 64.64 fixed
/// point.
///
/// The crate refuses a step outside that range and never clamps one: a clock servo clamps its
/// own correction to ±[`Step::MAX_DEVIATION`] before building the step, where the loop's gain
/// and limits are decided.
///
/// Resolution: [`Step::new`] holds its `f64` exactly, so it resolves what an `f64` near 1
/// does, 2⁻⁵² (2.2e-16) above 1 and 2⁻⁵³ below — over 10⁹ outputs a ratio rounded to that
/// grid can place the last one up to about 1e-7 samples from where the exact ratio would.
/// [`Step::from_deviation`] takes the deviation from 1 instead and resolves 2⁻⁶⁴ (5.4e-20).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Step(u128);

impl Step {
    /// The farthest a step may be from 1. Crystals disagree by parts per million; a thousand
    /// times that is a wrong sample rate, not drift, and the kernel's passband is only
    /// characterised for ratios this close to 1.
    pub const MAX_DEVIATION: f64 = 1e-3;

    /// One input sample per output sample.
    pub const ONE: Self = Self(1 << 64);

    /// `step` input samples per output sample, exactly; refused outside
    /// 1 ± [`Self::MAX_DEVIATION`].
    pub fn new(step: f64) -> Result<Self, Error> {
        // Bounds as the caller would write them (`1.0 - 1e-3`), not `|step − 1|`, which
        // rounds 0.999 to just over 1e-3 away.
        if !(1.0 - Self::MAX_DEVIATION..=1.0 + Self::MAX_DEVIATION).contains(&step) {
            return Err(Error::StepOutOfRange { step });
        }
        // Near 1 an f64's spacing is 2⁻⁵² or 2⁻⁵³, both multiples of 2⁻⁶⁴, and scaling by a
        // power of two is exact: the product is an integer.
        Ok(Self((step * ONE_F64) as u128))
    }

    /// A step of `1 + deviation`, the deviation rounded to the nearest 2⁻⁶⁴ rather than the
    /// sum to an `f64`'s 2⁻⁵²; refused outside ±[`Self::MAX_DEVIATION`].
    pub fn from_deviation(deviation: f64) -> Result<Self, Error> {
        if !(-Self::MAX_DEVIATION..=Self::MAX_DEVIATION).contains(&deviation) {
            return Err(Error::DeviationOutOfRange { deviation });
        }
        // |deviation|·2⁶⁴ ≤ 1.9e16, far inside u128, and the scaling itself is exact.
        let bits = (deviation.abs() * ONE_F64).round() as u128;
        Ok(Self(if deviation >= 0.0 {
            Self::ONE.0 + bits
        } else {
            Self::ONE.0 - bits
        }))
    }

    /// The step as the nearest `f64` (exact for any step built by [`Step::new`]).
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
    fn deviation_resolves_below_f64_spacing_near_1() {
        // 2⁻⁶⁰ is far below 1's f64 spacing: `Step::new(1.0 + 2⁻⁶⁰)` would see exactly 1.
        let tiny = 1.0 / (1u128 << 60) as f64;
        assert_eq!(Step::from_deviation(tiny).unwrap().0, (1 << 64) + 16);
        assert_eq!(Step::from_deviation(-tiny).unwrap().0, (1 << 64) - 16);
        assert_eq!(Step::from_deviation(0.0).unwrap(), Step::ONE);
        // A deviation whose bits reach below 2⁻⁶⁴ rounds to the nearest: 1e-6·2⁶⁴ is
        // 18446744073709.55…
        assert_eq!(
            Step::from_deviation(1e-6).unwrap().0,
            (1 << 64) + 18_446_744_073_710
        );
        // Both bounds are in range, as `Step::new`'s are.
        for d in [1e-3, -1e-3] {
            let s = Step::from_deviation(d).unwrap();
            assert!((s.get() - (1.0 + d)).abs() < 1e-15);
        }
        for d in [1.0001e-3, -1.0001e-3, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                Step::from_deviation(d),
                Err(Error::DeviationOutOfRange { .. })
            ));
        }
    }

    #[test]
    fn fraction_stays_below_1_for_every_bit_pattern() {
        let p = Pos(u128::from(u64::MAX));
        assert_eq!(p.frac_bits(), u64::MAX);
        assert!(p.fraction() < 1.0);
        assert_eq!(p.fraction(), 1.0 - TWO_POW_MINUS_53);
        assert_eq!(Pos::new(3, 0.75).unwrap().fraction(), 0.75);
    }

    #[test]
    fn pos_accumulates_exactly_at_large_index() {
        let start = Pos::new(96_000 * 86_400 * 3650, 0.25).unwrap();
        let step = Step::new(1.0 + 3e-6).unwrap();
        let mut p = start;
        for _ in 0..100_000 {
            p = p.advanced(step, 1).unwrap();
        }
        assert_eq!(Some(p), start.advanced(step, 100_000));
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

    #[test]
    fn advanced_refuses_overflow() {
        let last = Pos::from_index(u64::MAX);
        assert_eq!(last.advanced(Step::ONE, 0), Some(last));
        assert_eq!(last.advanced(Step::ONE, 1), None);
        assert_eq!(Pos::from_index(0).advanced(Step::ONE, u64::MAX), Some(last));
        let fast = Step::new(1.0 + 1e-3).unwrap();
        assert_eq!(Pos::from_index(0).advanced(fast, u64::MAX), None);
    }

    #[test]
    fn diff_is_signed_and_exact_at_large_index() {
        let a = Pos::new(1 << 40, 0.25).unwrap();
        let b = Pos::new((1 << 40) - 3, 0.75).unwrap();
        assert_eq!(a.diff(b), 2.5);
        assert_eq!(b.diff(a), -2.5);
        assert_eq!(a.diff(a), 0.0);
        // One unit of 2⁻⁶⁴ at an index where an f64 sample time spaces 2⁻¹²: still resolved.
        let c = Pos(a.0 + 1);
        assert_eq!(c.diff(a), 1.0 / ONE_F64);
        assert_eq!(
            Pos::from_index(u64::MAX).diff(Pos::from_index(0)),
            2f64.powi(64)
        );
    }

    #[test]
    fn shifted_is_exact_and_refuses_leaving_the_range() {
        let a = Pos::new(1 << 40, 0.25).unwrap();
        assert_eq!(
            a.shifted(-2.5),
            Some(Pos::new((1 << 40) - 3, 0.75).unwrap())
        );
        assert_eq!(a.shifted(2.5).unwrap().diff(a), 2.5);
        assert_eq!(a.shifted(0.0), Some(a));
        assert_eq!(a.shifted(-0.0), Some(a));
        // A shift below 2⁻¹¹ sample keeps its bits down to 2⁻⁶⁴.
        let tiny = 3.0 / ONE_F64;
        assert_eq!(a.shifted(tiny), Some(Pos(a.0 + 3)));
        assert_eq!(a.shifted(-tiny), Some(Pos(a.0 - 3)));
        assert_eq!(Pos::from_index(0).shifted(-tiny), None);
        assert_eq!(Pos::from_index(u64::MAX).shifted(1.0), None);
        for s in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300, -1e300] {
            assert_eq!(a.shifted(s), None);
        }
    }
}
