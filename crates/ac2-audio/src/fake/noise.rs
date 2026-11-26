//! [`TrueTimeNoise`]: one continuous band-limited noise signal of true time, so fake devices
//! on different clocks, in one process or several, sample the same signal.

use std::sync::OnceLock;

/// Taps of the interpolating kernel, in grid samples.
const TAPS: usize = 64;
/// Fractional phases of the kernel table; adjacent phases are interpolated linearly, which
/// leaves the kernel's error far below its stopband.
const PHASES: usize = 1024;
/// Kaiser window shape: about 86 dB of stopband.
const BETA: f64 = 8.6;
/// Cutoff, as a share of the grid rate (the nominal sample rate). A 64-tap Kaiser kernel at
/// β 8.6 needs about ±0.043 of the rate to fall from its passband into its stopband, so
/// this cutoff keeps the passband flat to 0.39 of the rate and the stopband (over 80 dB down)
/// from 0.48: nothing aliases when a device off its nominal rate samples it.
const CUTOFF: f64 = 0.435;
/// The signal's seed: fixed, so it is one signal everywhere.
const SEED: u64 = 0x7472_7565_7469_6d65;
/// Level: −20 dBFS rms.
const LEVEL_RMS: f64 = 0.1;
/// Grid values kept, a power of two above the kernel's span plus a block's advance at any
/// rate, tagged by their grid index so a jump in time (a lost stretch, another delay) only
/// misses.
const CACHE: usize = 256;

/// The kernel table: `PHASES + 1` rows of `TAPS`; row `p` weighs grid samples `i − 31 ..=
/// i + 32` for a position `p / PHASES` past grid sample `i`, scaled so that grid values of
/// unit variance give an output of unit variance. The same in grid units at every rate.
fn table() -> &'static [f32] {
    static TABLE: OnceLock<Box<[f32]>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let half = (TAPS / 2) as f64;
        let i0 = |x: f64| {
            // Bessel I₀ by its series, converged well before 40 terms at β 8.6.
            let (mut sum, mut term) = (1.0, 1.0);
            for k in 1..40 {
                term *= (x / (2.0 * k as f64)).powi(2);
                sum += term;
            }
            sum
        };
        let kernel = |x: f64| {
            let u = x / half;
            let w = i0(BETA * (1.0 - u * u).max(0.0).sqrt()) / i0(BETA);
            let a = 2.0 * CUTOFF * x;
            let sinc = if a == 0.0 {
                1.0
            } else {
                (std::f64::consts::PI * a).sin() / (std::f64::consts::PI * a)
            };
            2.0 * CUTOFF * sinc * w
        };
        let mut t = vec![0.0f64; (PHASES + 1) * TAPS];
        let mut power = 0.0;
        for p in 0..=PHASES {
            let frac = p as f64 / PHASES as f64;
            for k in 0..TAPS {
                let v = kernel(k as f64 - (half - 1.0) - frac);
                t[p * TAPS + k] = v;
                power += v * v;
            }
        }
        // Grid values are independent, so the output's variance is the row's power; it
        // barely varies with the phase, and its mean sets the level.
        let scale = ((PHASES + 1) as f64 / power).sqrt();
        t.iter().map(|&v| (v * scale) as f32).collect()
    })
}

/// White noise band-limited to [`CUTOFF`] of the rate at −20 dBFS rms, as a function of
/// continuous time: independent values on a grid at the nominal rate from a counter hash of
/// the grid index, interpolated by a Kaiser-windowed sinc. Band-limited, so it can be read at
/// any instant: a device off its nominal rate samples it as an interface samples a signal.
///
/// Times are ns since the Unix epoch on the true-time axis, the clock the fake's simulated
/// system clocks keep before their stated offsets.
#[derive(Clone)]
pub struct TrueTimeNoise {
    rate: u32,
    table: &'static [f32],
    cache: Box<[(u64, f32)]>,
}

impl std::fmt::Debug for TrueTimeNoise {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrueTimeNoise")
            .field("rate", &self.rate)
            .finish_non_exhaustive()
    }
}

impl TrueTimeNoise {
    /// The signal on the grid of `sample_rate` (non-zero), the nominal rate of the devices
    /// that sample it.
    pub fn new(sample_rate: u32) -> Self {
        Self {
            rate: sample_rate.max(1),
            table: table(),
            cache: vec![(u64::MAX, 0.0); CACHE].into_boxed_slice(),
        }
    }

    /// The signal at `ns + frac_ns` (`frac_ns` in `[0, 1)`), ns since the Unix epoch.
    pub fn at(&mut self, ns: u64, frac_ns: f64) -> f32 {
        let (i, f) = self.grid(ns, frac_ns);
        self.at_grid(i, f)
    }

    /// The grid position of `ns + frac_ns`: a grid index and an offset from it (not
    /// reduced: `[0, 1 + rate / 10⁹)`). Exact in the index whatever the time's size.
    pub(super) fn grid(&self, ns: u64, frac_ns: f64) -> (u64, f64) {
        let g = u128::from(ns) * u128::from(self.rate);
        let (q, r) = (g / 1_000_000_000, g % 1_000_000_000);
        let f = (r as f64 + frac_ns * f64::from(self.rate)) / 1e9;
        (q as u64, f)
    }

    /// The signal at grid position `index + offset` (`offset` ≥ 0, any size).
    #[inline]
    pub(super) fn at_grid(&mut self, index: u64, offset: f64) -> f32 {
        let whole = offset.floor();
        let i = index.wrapping_add(whole as u64);
        let x = (offset - whole) * PHASES as f64;
        let p = (x as usize).min(PHASES - 1);
        let w = (x - p as f64) as f32;
        let (a, b) = (p * TAPS, (p + 1) * TAPS);
        let first = i.wrapping_sub(TAPS as u64 / 2 - 1);
        let mut acc = 0.0f32;
        for k in 0..TAPS {
            let c = self.table[a + k] + w * (self.table[b + k] - self.table[a + k]);
            acc += c * self.value(first.wrapping_add(k as u64));
        }
        acc
    }

    /// Grid value `i`: uniform, unit variance, scaled to the level; cached.
    #[inline]
    fn value(&mut self, i: u64) -> f32 {
        let slot = &mut self.cache[(i as usize) & (CACHE - 1)];
        if slot.0 != i {
            *slot = (i, hash_value(i));
        }
        slot.1
    }
}

/// Grid value `i`: splitmix64's finaliser on a counter, mapped to a uniform of unit variance
/// at the level.
fn hash_value(i: u64) -> f32 {
    let mut z = SEED.wrapping_add(i.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    let u = (z >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0;
    (u * 3f64.sqrt() * LEVEL_RMS) as f32
}
