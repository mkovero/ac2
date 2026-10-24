//! BS.1770-5 Annex 1 K-weighting: a high-shelf pre-filter (the acoustic effect of the head)
//! followed by the RLB high pass, at any supported rate.
//!
//! The annex prints the two biquads for 48 kHz only and asks other rates for the same
//! response. Both stages are bilinear-transform designs, so they are re-derived per rate
//! from the analogue parameters that reproduce Tables 1 and 2 at 48 kHz to about 1e-15.
//! The coefficients and the state are f64: at 192 kHz the RLB poles lie about 1e-3 from the
//! unit circle, where f32 coefficient rounding moves the corner audibly and f32 state
//! accumulates noise above the signal of a quiet programme.

use std::f64::consts::PI;

/// Pre-filter centre frequency, gain and Q fitted to Table 1.
const PRE_F0: f64 = 1681.974450955533;
const PRE_GAIN_DB: f64 = 3.999843853973347;
const PRE_Q: f64 = 0.7071752369554196;
/// Shelf mid-band gain exponent: `Vb = Vh^PRE_VB_EXP` places the shelf's half-gain point
/// where Table 1 has it.
const PRE_VB_EXP: f64 = 0.4996667741545416;
/// RLB corner and Q fitted to Table 2.
const RLB_F0: f64 = 38.13547087602444;
const RLB_Q: f64 = 0.5003270373238773;

/// The rate the annex's tables are printed for.
pub const TABLE_RATE: f64 = 48000.0;

/// BS.1770-5 Table 1 (pre-filter, 48 kHz): `b0, b1, b2, a1, a2` with `a0 = 1`.
pub const TABLE_1: [f64; 5] = [
    1.53512485958697,
    -2.69169618940638,
    1.19839281085285,
    -1.69065929318241,
    0.73248077421585,
];
/// BS.1770-5 Table 2 (RLB high pass, 48 kHz): `b0, b1, b2, a1, a2` with `a0 = 1`.
pub const TABLE_2: [f64; 5] = [1.0, -2.0, 1.0, -1.99004745483398, 0.99007225036621];

/// One biquad's coefficients, `a0` normalised to 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coefficients {
    /// Numerator b₀, b₁, b₂.
    pub b: [f64; 3],
    /// Denominator a₁, a₂.
    pub a: [f64; 2],
}

impl Coefficients {
    /// Magnitude response in dB at `f` Hz for rate `fs`. Written in `φ = sin²(ω/2)`:
    /// the RLB poles sit so close to z = 1 that the plain `cos ω` form cancels away the
    /// digits that matter at low frequencies.
    pub fn magnitude_db(&self, f: f64, fs: f64) -> f64 {
        let phi = (PI * f / fs).sin().powi(2);
        let power = |c: [f64; 3]| {
            let s = c[0] + c[1] + c[2];
            s * s - 4.0 * (c[0] * c[1] + 4.0 * c[0] * c[2] + c[1] * c[2]) * phi
                + 16.0 * c[0] * c[2] * phi * phi
        };
        let num = power(self.b);
        let den = power([1.0, self.a[0], self.a[1]]);
        10.0 * (num / den).log10()
    }
}

/// The two K-weighting stages for a rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KWeighting {
    pub pre: Coefficients,
    pub rlb: Coefficients,
}

fn rlb_a0(fs: f64) -> f64 {
    let k = (PI * RLB_F0 / fs).tan();
    1.0 + k / RLB_Q + k * k
}

impl KWeighting {
    /// The cascade for `fs` Hz. Table 2's numerator is the unnormalised `[1, −2, 1]`,
    /// which gives the RLB stage a +0.043 dB passband gain at 48 kHz; scaling it by
    /// `a0(48 kHz)/a0(fs)` keeps that gain at every rate, so the response (and the −0.691
    /// constant that cancels it at 997 Hz) carries over unchanged.
    pub fn new(fs: f64) -> Self {
        let k = (PI * PRE_F0 / fs).tan();
        let vh = 10f64.powf(PRE_GAIN_DB / 20.0);
        let vb = vh.powf(PRE_VB_EXP);
        let a0 = 1.0 + k / PRE_Q + k * k;
        let pre = Coefficients {
            b: [
                (vh + vb * k / PRE_Q + k * k) / a0,
                2.0 * (k * k - vh) / a0,
                (vh - vb * k / PRE_Q + k * k) / a0,
            ],
            a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / PRE_Q + k * k) / a0],
        };
        let k = (PI * RLB_F0 / fs).tan();
        let a0 = 1.0 + k / RLB_Q + k * k;
        let g = rlb_a0(TABLE_RATE) / a0;
        let rlb = Coefficients {
            b: [g, -2.0 * g, g],
            a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / RLB_Q + k * k) / a0],
        };
        Self { pre, rlb }
    }

    /// Cascade magnitude in dB at `f` Hz for rate `fs`.
    pub fn magnitude_db(&self, f: f64, fs: f64) -> f64 {
        self.pre.magnitude_db(f, fs) + self.rlb.magnitude_db(f, fs)
    }
}

/// Below this a filter state is flushed to zero: a digital-silence tail would otherwise
/// decay through subnormals, which cost up to a hundred times a normal operation on x86.
const FLUSH: f64 = 1e-30;

/// One channel's K-weighting state: two transposed direct-form II sections.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct ChannelFilter {
    s: [[f64; 2]; 2],
}

impl ChannelFilter {
    #[inline]
    pub(super) fn process(&mut self, k: &KWeighting, x: f64) -> f64 {
        let mut v = x;
        for (st, c) in self.s.iter_mut().zip([&k.pre, &k.rlb]) {
            let y = c.b[0] * v + st[0];
            st[0] = c.b[1] * v - c.a[0] * y + st[1];
            st[1] = c.b[2] * v - c.a[1] * y;
            v = y;
        }
        v
    }

    pub(super) fn flush_denormals(&mut self) {
        for v in self.s.iter_mut().flatten() {
            if v.abs() < FLUSH {
                *v = 0.0;
            }
        }
    }

    pub(super) fn clear(&mut self) {
        self.s = [[0.0; 2]; 2];
    }
}

#[cfg(test)]
impl ChannelFilter {
    pub(super) fn is_zero(&self) -> bool {
        self.s.iter().flatten().all(|&v| v == 0.0)
    }
}
