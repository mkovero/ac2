//! The device model, in f64, with no allocation once built.
//!
//! `dut_out = post(poly(pre(dut_in))) + noise_d`, `ref_out = ref_in + noise_r`.
//!
//! Biquad convention (the contract with the suite, which designs the coefficients and computes
//! the analytic response from the same definition): coefficients are normalised so a0 = 1 and
//! given as `b0,b1,b2,a1,a2`, meaning
//!
//! ```text
//! y[n] = b0 x[n] + b1 x[n-1] + b2 x[n-2] − a1 y[n-1] − a2 y[n-2]
//! H(z) = (b0 + b1 z⁻¹ + b2 z⁻²) / (1 + a1 z⁻¹ + a2 z⁻²)
//! ```
//!
//! realised as Direct Form II transposed, whose two state values keep rounding noise low for
//! the high-Q, low-frequency sections the suite uses.
//!
//! The polynomial is `poly(u) = Σ_k c_k u^k`, evaluated by Horner's rule.
//!
//! Noise is Gaussian white noise (Box–Muller over a xorshift64* generator), independent per
//! channel. Its level is in dBFS where 0 dBFS is a full-scale sine of peak 1.0, so the RMS of
//! the noise is `10^(dB/20)/√2`.
//!
//! Every output sample is hard-limited to ±1.0. The suite keeps signals far below that; the
//! limit only keeps a mistaken coefficient set from sending a runaway signal downstream.

use std::f64::consts::TAU;

/// One second-order section, Direct Form II transposed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Biquad {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
    s1: f64,
    s2: f64,
}

impl Biquad {
    /// Coefficients in the `b0,b1,b2,a1,a2` convention (a0 = 1), zero state.
    pub fn new(b0: f64, b1: f64, b2: f64, a1: f64, a2: f64) -> Self {
        Self {
            b0,
            b1,
            b2,
            a1,
            a2,
            s1: 0.0,
            s2: 0.0,
        }
    }

    /// Both poles strictly inside the unit circle. For z² + a1 z + a2 that holds exactly
    /// inside the stability triangle |a2| < 1, |a1| < 1 + a2.
    pub fn is_stable(&self) -> bool {
        self.a2.abs() < 1.0 && self.a1.abs() < 1.0 + self.a2
    }

    #[inline]
    pub fn tick(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.s1;
        self.s1 = self.b1 * x - self.a1 * y + self.s2;
        self.s2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// `poly(u) = Σ_k c[k] u^k`.
#[inline]
pub fn poly(c: &[f64], u: f64) -> f64 {
    c.iter().rev().fold(0.0, |acc, &ck| acc * u + ck)
}

/// xorshift64* feeding a Box–Muller transform: deterministic, allocation-free, and with
/// ample period for any run length the suite uses.
#[derive(Debug, Clone)]
pub struct Noise {
    state: u64,
    rms: f64,
    spare: Option<f64>,
}

impl Noise {
    /// `rms` of 0 makes the generator produce exact zeros.
    pub fn new(seed: u64, rms: f64) -> Self {
        // xorshift has a fixed point at zero.
        let state = if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        };
        Self {
            state,
            rms,
            spare: None,
        }
    }

    /// Noise RMS for a level in dBFS relative to a full-scale sine (peak 1.0).
    pub fn rms_for_dbfs(db: f64) -> f64 {
        10f64.powf(db / 20.0) / std::f64::consts::SQRT_2
    }

    #[inline]
    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in (0, 1]: never 0, so the logarithm below stays finite.
    #[inline]
    fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 1.0) * (1.0 / (1u64 << 53) as f64)
    }

    #[inline]
    pub fn sample(&mut self) -> f64 {
        if self.rms == 0.0 {
            return 0.0;
        }
        if let Some(z) = self.spare.take() {
            return z * self.rms;
        }
        let r = (-2.0 * self.uniform().ln()).sqrt();
        let th = TAU * self.uniform();
        self.spare = Some(r * th.sin());
        r * th.cos() * self.rms
    }
}

/// Hard limit to ±1.0; a non-finite value (which only a broken model could make) becomes 0.
#[inline]
fn limit(y: f64) -> f32 {
    if y.is_finite() {
        y.clamp(-1.0, 1.0) as f32
    } else {
        0.0
    }
}

/// The whole device: both paths and their state.
#[derive(Debug, Clone)]
pub struct Device {
    pre: Vec<Biquad>,
    poly: Vec<f64>,
    post: Vec<Biquad>,
    noise_d: Noise,
    noise_r: Noise,
}

/// Fixed per-channel seeds: runs are repeatable and the two channels' noise is independent.
pub const SEED_DUT: u64 = 0x0D57_A11C_E5EE_D001;
pub const SEED_REF: u64 = 0x5EED_0F2E_F00D_CAFE;

impl Device {
    pub fn new(pre: Vec<Biquad>, poly: Vec<f64>, post: Vec<Biquad>, noise_rms: f64) -> Self {
        Self {
            pre,
            poly,
            post,
            noise_d: Noise::new(SEED_DUT, noise_rms),
            noise_r: Noise::new(SEED_REF, noise_rms),
        }
    }

    #[inline]
    pub fn dut_sample(&mut self, x: f64) -> f64 {
        let u = self.pre.iter_mut().fold(x, |v, b| b.tick(v));
        let p = poly(&self.poly, u);
        self.post.iter_mut().fold(p, |v, b| b.tick(v)) + self.noise_d.sample()
    }

    /// Processes one block of each path. Slices of a path must have equal length.
    pub fn process(
        &mut self,
        ref_in: &[f32],
        ref_out: &mut [f32],
        dut_in: &[f32],
        dut_out: &mut [f32],
    ) {
        for (o, &x) in ref_out.iter_mut().zip(ref_in) {
            *o = limit(f64::from(x) + self.noise_r.sample());
        }
        for (o, &x) in dut_out.iter_mut().zip(dut_in) {
            *o = limit(self.dut_sample(f64::from(x)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// RBJ cookbook low-pass, normalised to a0 = 1.
    fn rbj_lowpass(f0: f64, q: f64, fs: f64) -> Biquad {
        let w0 = 2.0 * PI * f0 / fs;
        let alpha = w0.sin() / (2.0 * q);
        let c = w0.cos();
        let a0 = 1.0 + alpha;
        Biquad::new(
            (1.0 - c) / 2.0 / a0,
            (1.0 - c) / a0,
            (1.0 - c) / 2.0 / a0,
            -2.0 * c / a0,
            (1.0 - alpha) / a0,
        )
    }

    /// H(e^{jω}) as (re, im).
    fn response(b: &Biquad, w: f64) -> (f64, f64) {
        let (c1, s1) = (w.cos(), -w.sin());
        let (c2, s2) = ((2.0 * w).cos(), -(2.0 * w).sin());
        let nr = b.b0 + b.b1 * c1 + b.b2 * c2;
        let ni = b.b1 * s1 + b.b2 * s2;
        let dr = 1.0 + b.a1 * c1 + b.a2 * c2;
        let di = b.a1 * s1 + b.a2 * s2;
        let d = dr * dr + di * di;
        ((nr * dr + ni * di) / d, (ni * dr - nr * di) / d)
    }

    /// Complex amplitude of bin `k` of `x` (one-sided: a sine of peak A reads A).
    fn dft_bin(x: &[f64], k: usize) -> (f64, f64) {
        let n = x.len() as f64;
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &v) in x.iter().enumerate() {
            let ph = TAU * (k as f64) * (i as f64) / n;
            re += v * ph.cos();
            im -= v * ph.sin();
        }
        (2.0 * re / n, 2.0 * im / n)
    }

    #[test]
    fn biquad_matches_closed_form() {
        let fs = 48000.0;
        let b = rbj_lowpass(1000.0, 2.0, fs);
        assert!(b.is_stable());
        // An integer number of periods of a bin-centred sine, after the transient has died.
        let n = 48000;
        let k = 1500; // 1500 Hz, above the corner where both gain and phase move
        let w = TAU * k as f64 / n as f64;
        let mut f = b;
        let settle = 20 * n;
        let mut out = Vec::with_capacity(n);
        for i in 0..settle + n {
            let y = f.tick((w * i as f64).cos());
            if i >= settle {
                out.push(y);
            }
        }
        let (re, im) = dft_bin(&out, k);
        let (hr, hi) = response(&b, w);
        let (mag, hmag) = (re.hypot(im), hr.hypot(hi));
        assert!(((mag - hmag) / hmag).abs() < 1e-9, "{mag} vs {hmag}");
        // settle is a whole number of periods, so the cosine's phase at the window start is 0.
        let dphi = (im.atan2(re) - hi.atan2(hr) + PI).rem_euclid(TAU) - PI;
        assert!(dphi.abs() < 1e-9, "phase error {dphi}");
    }

    #[test]
    fn stability_triangle() {
        assert!(Biquad::new(1.0, 0.0, 0.0, -1.9, 0.95).is_stable());
        assert!(!Biquad::new(1.0, 0.0, 0.0, 0.0, 1.0).is_stable()); // poles on |z| = 1
        assert!(!Biquad::new(1.0, 0.0, 0.0, -2.0, 1.0).is_stable()); // double pole at z = 1
        assert!(!Biquad::new(1.0, 0.0, 0.0, 1.5, 0.4).is_stable()); // real pole below −1
    }

    #[test]
    fn polynomial_harmonics_closed_form() {
        let (a, c2, c3) = (0.316, 0.02, 0.04);
        let mut dev = Device::new(vec![], vec![0.0, 1.0, c2, c3], vec![], 0.0);
        let n = 4096;
        let k = 37;
        let x: Vec<f64> = (0..n)
            .map(|i| a * (TAU * k as f64 * i as f64 / n as f64).sin())
            .collect();
        let y: Vec<f64> = x.iter().map(|&v| dev.dut_sample(v)).collect();
        let amp = |h: usize| {
            let (re, im) = dft_bin(&y, h * k);
            re.hypot(im)
        };
        // sin² = (1 − cos 2θ)/2, sin³ = (3 sin θ − sin 3θ)/4.
        let h1 = a * (1.0 + 3.0 * c3 * a * a / 4.0);
        let h2 = c2 * a * a / 2.0;
        let h3 = c3 * a * a * a / 4.0;
        for (got, want) in [(amp(1), h1), (amp(2), h2), (amp(3), h3)] {
            assert!(((got - want) / want).abs() < 1e-12, "{got} vs {want}");
        }
        assert!(amp(4) < 1e-14);
        // The DC term c2 A²/2.
        let dc = y.iter().sum::<f64>() / n as f64;
        assert!((dc - c2 * a * a / 2.0).abs() < 1e-14);
    }

    fn run_blocks(block: usize, input: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let pre = vec![rbj_lowpass(2000.0, 0.7, 48000.0)];
        let post = vec![
            rbj_lowpass(300.0, 4.0, 48000.0),
            rbj_lowpass(8000.0, 0.5, 48000.0),
        ];
        let mut dev = Device::new(
            pre,
            vec![0.0, 1.0, 0.1, 0.05],
            post,
            Noise::rms_for_dbfs(-80.0),
        );
        let mut r = vec![0.0; input.len()];
        let mut d = vec![0.0; input.len()];
        for ((ri, ro), (di, dout)) in input
            .chunks(block)
            .zip(r.chunks_mut(block))
            .zip(input.chunks(block).zip(d.chunks_mut(block)))
        {
            dev.process(ri, ro, di, dout);
        }
        (r, d)
    }

    #[test]
    fn block_size_independent() {
        let input: Vec<f32> = (0..5000)
            .map(|i| (0.3 * (i as f64 * 0.0731).sin() + 0.1 * (i as f64 * 0.41).cos()) as f32)
            .collect();
        let one = run_blocks(1, &input);
        assert_eq!(one, run_blocks(64, &input));
        assert_eq!(one, run_blocks(256, &input));
    }

    #[test]
    fn noise_level_and_independence() {
        let db = -40.0;
        let want = Noise::rms_for_dbfs(db);
        let mut d = Noise::new(SEED_DUT, want);
        let mut r = Noise::new(SEED_REF, want);
        let n = 1_000_000;
        let (mut sd, mut sr, mut sx) = (0.0, 0.0, 0.0);
        for _ in 0..n {
            let (a, b) = (d.sample(), r.sample());
            sd += a * a;
            sr += b * b;
            sx += a * b;
        }
        let (rd, rr) = ((sd / n as f64).sqrt(), (sr / n as f64).sqrt());
        assert!(((rd - want) / want).abs() < 0.02, "{rd} vs {want}");
        assert!(((rr - want) / want).abs() < 0.02, "{rr} vs {want}");
        let corr = sx / (sd * sr).sqrt();
        assert!(corr.abs() < 0.01, "correlation {corr}");
    }

    #[test]
    fn noise_off_is_exact_and_ref_is_identity() {
        let mut dev = Device::new(vec![], vec![0.0, 1.0], vec![], 0.0);
        let x: Vec<f32> = (0..100).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();
        let mut r = vec![0.0; 100];
        let mut d = vec![0.0; 100];
        dev.process(&x, &mut r, &x, &mut d);
        assert_eq!(r, x);
        assert_eq!(d, x);
    }

    #[test]
    fn output_is_limited() {
        let mut dev = Device::new(vec![], vec![0.0, 4.0], vec![], 0.0);
        let x = [0.5f32, -0.5, 0.125];
        let mut r = [0.0; 3];
        let mut d = [0.0; 3];
        dev.process(&x, &mut r, &x, &mut d);
        assert_eq!(d, [1.0, -1.0, 0.5]);
    }
}
