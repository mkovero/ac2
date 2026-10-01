//! Small deterministic PRNG for test signals and the fake device. Not cryptographic; chosen
//! because it is seedable, allocation-free and identical on every platform.

/// SplitMix64 generator.
#[derive(Debug, Clone)]
pub(crate) struct Rng {
    state: u64,
    spare: Option<f64>,
}

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self {
            state: seed,
            spare: None,
        }
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub(crate) fn uniform(&mut self) -> f64 {
        // 53 random mantissa bits.
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Standard normal (Box–Muller; the second value of each pair is kept for the next call).
    pub(crate) fn gaussian(&mut self) -> f64 {
        if let Some(z) = self.spare.take() {
            return z;
        }
        // 1 - u is in (0, 1], so the logarithm is finite.
        let r = (-2.0 * (1.0 - self.uniform()).ln()).sqrt();
        let theta = std::f64::consts::TAU * self.uniform();
        self.spare = Some(r * theta.sin());
        r * theta.cos()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaussian_has_unit_variance_and_zero_mean() {
        let mut r = Rng::new(7);
        let n = 200_000;
        let (mut sum, mut sq) = (0.0, 0.0);
        for _ in 0..n {
            let g = r.gaussian();
            sum += g;
            sq += g * g;
        }
        let mean = sum / n as f64;
        let var = sq / n as f64 - mean * mean;
        assert!(mean.abs() < 0.01, "mean {mean}");
        assert!((var - 1.0).abs() < 0.02, "var {var}");
    }

    #[test]
    fn same_seed_same_sequence() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }
}
