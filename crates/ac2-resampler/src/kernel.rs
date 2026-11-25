//! The interpolation kernel: a Kaiser-windowed sinc, tabulated at evenly spaced fractional
//! delays (phases) and interpolated linearly between adjacent phases.

use std::f64::consts::PI;

use crate::Error;

/// What a [`Kernel`] is built from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KernelSpec {
    /// Taps on each side of the output position: the kernel reads `2 · half_length` input
    /// samples per output sample.
    pub half_length: usize,
    /// Tabulated fractional delays per input sample; a power of two so a position's fraction
    /// bits split into phase and in-between weight by shifting.
    pub phases: usize,
    /// Kaiser window shape: larger trades a wider transition band for lower stopband leakage.
    pub beta: f64,
}

/// A tabulated fractional-delay kernel; share one between channels (it holds the table).
#[derive(Clone, Debug)]
pub struct Kernel {
    spec: KernelSpec,
    log2_phases: u32,
    /// `phases + 1` rows of `2 · half_length` coefficients. Row `p` delays by `p / phases`;
    /// tap `k` weighs input `index + k − (half_length − 1)`. The extra last row (delay 1) is
    /// row 0 moved one tap, so interpolating from the last phase needs no wrap.
    table: Vec<f64>,
}

impl Kernel {
    /// The largest table [`Kernel::new`] builds: 16 MiB, 18 times the measurement preset's,
    /// room for a longer or finer kernel. Each output reads two rows picked by its phase, in
    /// an order no cache predicts, so a table far past the caches is a slower kernel, not a
    /// better one; and the other limits alone would allow 1 GiB, which is an accident.
    pub const MAX_TABLE_BYTES: usize = 16 << 20;

    /// The preset the crate's quality targets are measured on; see the crate docs for the
    /// numbers behind each constant.
    pub fn measurement() -> Self {
        Self::new(KernelSpec {
            half_length: 28,
            phases: 2048,
            beta: 14.5,
        })
        .expect("the measurement preset is a valid spec")
    }

    /// Builds the table, refused with [`Error::KernelSpec`] for a spec outside its limits and
    /// [`Error::TableTooLarge`] for a table over [`Self::MAX_TABLE_BYTES`].
    pub fn new(spec: KernelSpec) -> Result<Self, Error> {
        if spec.half_length == 0 || spec.half_length > 1024 {
            return Err(Error::KernelSpec(spec));
        }
        if !spec.phases.is_power_of_two() || spec.phases < 2 || spec.phases > 1 << 16 {
            return Err(Error::KernelSpec(spec));
        }
        if !(spec.beta >= 0.0 && spec.beta <= 50.0) {
            return Err(Error::KernelSpec(spec));
        }
        let n = spec.half_length;
        let taps = 2 * n;
        // Within the limits above this is at most 1 GiB, so it cannot overflow.
        let bytes = (spec.phases + 1) * taps * size_of::<f64>();
        if bytes > Self::MAX_TABLE_BYTES {
            return Err(Error::TableTooLarge { spec, bytes });
        }
        let i0_beta = bessel_i0(spec.beta);
        let mut table = vec![0.0; (spec.phases + 1) * taps];
        for (p, row) in table.chunks_exact_mut(taps).enumerate() {
            if p == 0 || p == spec.phases {
                // The sinc is exactly zero at every non-zero integer: an integer delay is a
                // unit impulse, so an output on an input sample reproduces it exactly.
                row[if p == 0 { n - 1 } else { n }] = 1.0;
                continue;
            }
            let frac = p as f64 / spec.phases as f64;
            for (k, c) in row.iter_mut().enumerate() {
                let t = frac + (n - 1) as f64 - k as f64;
                let r = t / n as f64;
                let w = bessel_i0(spec.beta * (1.0 - r * r).max(0.0).sqrt()) / i0_beta;
                *c = sinc(t) * w;
            }
            // Unit gain at DC for every phase, so a fraction never modulates a constant level.
            let sum: f64 = row.iter().sum();
            row.iter_mut().for_each(|c| *c /= sum);
        }
        Ok(Self {
            spec,
            log2_phases: spec.phases.trailing_zeros(),
            table,
        })
    }

    pub fn spec(&self) -> KernelSpec {
        self.spec
    }

    /// Input samples read per output sample.
    pub fn taps(&self) -> usize {
        2 * self.spec.half_length
    }

    /// Input samples needed after an output position's index: an output at `index + frac`
    /// needs input up to `index + lookahead()`. This is the latency the kernel adds.
    pub fn lookahead(&self) -> u64 {
        self.spec.half_length as u64
    }

    /// Input samples needed before an output position's index: an output at `index + frac`
    /// reads from `index − history()`.
    pub fn history(&self) -> u64 {
        self.spec.half_length as u64 - 1
    }

    /// One output sample at fraction bits `frac` (units of 2⁻⁶⁴) from `x`, the
    /// [`taps`](Self::taps) input samples starting [`history`](Self::history) before the
    /// position's index.
    #[inline]
    pub(crate) fn apply(&self, x: &[f32], frac: u64) -> f64 {
        let taps = self.taps();
        let phase = (frac >> (64 - self.log2_phases)) as usize;
        // The bits below the phase, as a weight in [0, 1) with 53 significant bits.
        let weight = ((frac << self.log2_phases) >> 11) as f64 * (1.0 / (1u64 << 53) as f64);
        let rows = &self.table[phase * taps..(phase + 2) * taps];
        let (r0, r1) = rows.split_at(taps);
        // Four partial sums per row: one running sum is a chain of dependent adds, so the
        // loop would wait on add latency rather than throughput.
        let (mut s0, mut s1) = ([0.0f64; LANES], [0.0f64; LANES]);
        let x = &x[..taps];
        let (xc, ac, bc) = (
            x.chunks_exact(LANES),
            r0.chunks_exact(LANES),
            r1.chunks_exact(LANES),
        );
        let (xr, ar, br) = (xc.remainder(), ac.remainder(), bc.remainder());
        for ((x, a), b) in xc.zip(ac).zip(bc) {
            for l in 0..LANES {
                let x = f64::from(x[l]);
                s0[l] += x * a[l];
                s1[l] += x * b[l];
            }
        }
        for ((&x, &a), &b) in xr.iter().zip(ar).zip(br) {
            let x = f64::from(x);
            s0[0] += x * a;
            s1[0] += x * b;
        }
        let s0: f64 = s0.iter().sum();
        let s1: f64 = s1.iter().sum();
        s0 + weight * (s1 - s0)
    }
}

/// Independent partial sums in the inner product.
const LANES: usize = 4;

/// sin(πt)/(πt).
fn sinc(t: f64) -> f64 {
    if t == 0.0 {
        1.0
    } else {
        (PI * t).sin() / (PI * t)
    }
}

/// Modified Bessel function of the first kind, order 0, by its power series; the terms fall
/// off factorially, so for the window's arguments (≤ 50) it converges in under 100 terms.
fn bessel_i0(x: f64) -> f64 {
    let q = x * x / 4.0;
    let (mut sum, mut term) = (1.0, 1.0);
    for k in 1..500 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn integer_phases_are_unit_impulses() {
        let k = Kernel::new(KernelSpec {
            half_length: 4,
            phases: 8,
            beta: 8.0,
        })
        .unwrap();
        let x: Vec<f32> = (1..=8).map(|v| v as f32 * 0.37).collect();
        assert_eq!(k.apply(&x, 0), f64::from(x[3]));
    }

    #[test]
    fn refuses_bad_specs() {
        for spec in [
            KernelSpec {
                half_length: 0,
                phases: 8,
                beta: 8.0,
            },
            KernelSpec {
                half_length: 4,
                phases: 12,
                beta: 8.0,
            },
            KernelSpec {
                half_length: 4,
                phases: 8,
                beta: f64::NAN,
            },
        ] {
            assert!(matches!(Kernel::new(spec), Err(Error::KernelSpec(_))));
        }
        // Within every per-field limit, but a 1 GiB table.
        let huge = KernelSpec {
            half_length: 1024,
            phases: 1 << 16,
            beta: 8.0,
        };
        assert_eq!(
            Kernel::new(huge).unwrap_err(),
            Error::TableTooLarge {
                spec: huge,
                bytes: 65_537 * 2048 * 8
            }
        );
        // 2¹⁵ + 1 rows of 64 taps is just over 16 MiB (the design table's 2¹⁴ phases build).
        let over = KernelSpec {
            half_length: 32,
            phases: 1 << 15,
            beta: 16.0,
        };
        assert!(matches!(
            Kernel::new(over),
            Err(Error::TableTooLarge { .. })
        ));
    }

    #[test]
    fn bessel_i0_matches_known_values() {
        assert!((bessel_i0(0.0) - 1.0).abs() < 1e-15);
        assert!((bessel_i0(1.0) - 1.266_065_877_752_008_4).abs() < 1e-14);
        assert!((bessel_i0(10.0) / 2_815.716_628_466_254 - 1.0).abs() < 1e-14);
    }
}
