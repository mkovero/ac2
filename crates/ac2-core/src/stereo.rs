//! Stereo pair meters (docs/design/programme-meter.md §4, §5): the zero-lag correlator and
//! the goniometer's point pick with its auto gain. Both read the signal only; the frame
//! layout and the drawing are the daemon's and the scene's.

use crate::spectrum::power_dbfs;

/// One side of a pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// A correlator reading. `quiet` names the first side (left checked first) whose level is
/// below the gate; then `r` is `None`: a dead channel must not read 0 and look like wide
/// stereo. Otherwise `r` is the normalised correlation in [−1, 1].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Correlation {
    pub r: Option<f32>,
    pub quiet: Option<Side>,
}

/// Normalised cross-correlation at zero lag over an exponential window (identical to
/// `tools/refgen/sets/stereo.py`): per sample `s += α·(v − s)` for `v ∈ {l·r, l², r²}`,
/// `α = 1 − e^(−1/(fs·τ))`, and `r = <lr> / sqrt(<l²>·<r²>)` clamped to [−1, 1]. Zero lag
/// only: an inter-channel delay lowers it honestly and no lag is searched.
#[derive(Debug, Clone)]
pub struct Correlator {
    sample_rate: f64,
    alpha: f64,
    lr: f64,
    ll: f64,
    rr: f64,
}

impl Correlator {
    pub fn new(sample_rate: f64, window_s: f64) -> Self {
        let mut c = Self {
            sample_rate,
            alpha: 0.0,
            lr: 0.0,
            ll: 0.0,
            rr: 0.0,
        };
        c.set_window(window_s);
        c
    }

    /// Changes the window (τ in seconds) without resetting the sums.
    pub fn set_window(&mut self, window_s: f64) {
        self.alpha = 1.0 - (-1.0 / (self.sample_rate * window_s)).exp();
    }

    /// Accumulates a block; both slices are read up to the shorter length.
    pub fn process(&mut self, l: &[f32], r: &[f32]) {
        for (&a, &b) in l.iter().zip(r) {
            let (a, b) = (f64::from(a), f64::from(b));
            self.lr += self.alpha * (a * b - self.lr);
            self.ll += self.alpha * (a * a - self.ll);
            self.rr += self.alpha * (b * b - self.rr);
        }
    }

    /// The reading, gated per side at `gate_dbfs` (RMS level, 0 dBFS = full-scale sine).
    pub fn reading(&self, gate_dbfs: f64) -> Correlation {
        let quiet = if power_dbfs(self.ll) < gate_dbfs {
            Some(Side::Left)
        } else if power_dbfs(self.rr) < gate_dbfs {
            Some(Side::Right)
        } else {
            None
        };
        let r = match quiet {
            Some(_) => None,
            None => Some((self.lr / (self.ll * self.rr).sqrt()).clamp(-1.0, 1.0) as f32),
        };
        Correlation { r, quiet }
    }
}

/// Points per goniometer frame.
pub const GONIO_POINTS: usize = 256;

/// The auto-gain floor: −60 dBFS, so the window is 30 dB or more above the quietest signal.
pub const GONIO_GAIN_FLOOR: f64 = 1e-3;

/// The intensity window below the gain, in dB.
pub const GONIO_WINDOW_DB: f32 = 30.0;

/// Draws [`GONIO_POINTS`] sample pairs per block for the goniometer, rotated 45° so that
/// mono stands upright and left leans left (`x = (r − l)/√2`, `y = (l + r)/√2`), divided
/// by the auto gain, clamped and quantised to `i8`.
///
/// Auto gain: a peak follower of the pair magnitude `√(l² + r²)` (the rotation keeps it, so
/// every point lies within radius 127 whatever the balance) with instant attack and an
/// exponential decay of τ = 1 s, floored at [`GONIO_GAIN_FLOOR`]; the figure shows shape,
/// never level. The follower runs on the squared magnitude, one sqrt per block.
///
/// The pick is not a fixed stride (a stride that divides a tone's period collapses it to a
/// few dots): the block is split into [`GONIO_POINTS`] bins and one pair is drawn from a
/// random position in each, from the picker's own xorshift64* generator. Bin `i` covers
/// `[⌊i·n/256⌋, ⌊(i+1)·n/256⌋)`; in a block shorter than 256 samples a bin that would be
/// empty takes the single sample at `⌊i·n/256⌋`, so a short block repeats samples rather
/// than leaving points unset.
#[derive(Debug, Clone)]
pub struct GonioPicker {
    decay_sq: f64,
    gain_sq: f64,
    rng: u64,
}

impl GonioPicker {
    /// Decay time constant of the auto gain.
    pub const GAIN_TAU_S: f64 = 1.0;

    pub fn new(sample_rate: f64, seed: u64) -> Self {
        Self {
            // The follower holds the squared magnitude, so it decays at twice the rate.
            decay_sq: (-2.0 / (sample_rate * Self::GAIN_TAU_S)).exp(),
            gain_sq: GONIO_GAIN_FLOOR * GONIO_GAIN_FLOOR,
            // xorshift needs a non-zero state.
            rng: seed | 1,
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// The auto gain (linear pair magnitude) after the last block.
    pub fn gain(&self) -> f64 {
        self.gain_sq.sqrt()
    }

    /// Runs the auto gain over the block, fills `out` with the picked points and returns
    /// the gain in dBFS (a full-scale mono signal reads +3.01: its pair magnitude is √2).
    /// Both slices are read up to the shorter length; an empty block leaves the gain as it
    /// is and fills `out` with the origin.
    pub fn pick(&mut self, l: &[f32], r: &[f32], out: &mut [[i8; 2]; GONIO_POINTS]) -> f32 {
        let n = l.len().min(r.len());
        for (&a, &b) in l.iter().zip(r) {
            let m = f64::from(a * a + b * b);
            self.gain_sq = if m > self.gain_sq {
                m
            } else {
                self.gain_sq * self.decay_sq
            };
        }
        self.gain_sq = self.gain_sq.max(GONIO_GAIN_FLOOR * GONIO_GAIN_FLOOR);
        let gain_db = (10.0 * self.gain_sq.log10()) as f32;
        if n == 0 {
            out.fill([0, 0]);
            return gain_db;
        }
        // 127/(g·√2): the rotation and the gain in one factor per block.
        let scale = 127.0 / (self.gain() * std::f64::consts::SQRT_2);
        for (i, point) in out.iter_mut().enumerate() {
            let lo = i * n / GONIO_POINTS;
            let hi = ((i + 1) * n / GONIO_POINTS).max(lo + 1).min(n);
            let k = lo + (self.next_u64() % (hi - lo) as u64) as usize;
            let (a, b) = (f64::from(l[k]), f64::from(r[k]));
            *point = [quantise((b - a) * scale), quantise((a + b) * scale)];
        }
        gain_db
    }
}

fn quantise(v: f64) -> i8 {
    v.round().clamp(-127.0, 127.0) as i8
}

/// Landing intensity of a picked point in 0…1: its level within the auto-gain window,
/// 1 at the gain (radius 127) and 0 at [`GONIO_WINDOW_DB`] below it. The rotation keeps the
/// pair's magnitude, so the quantised point alone carries the level and the wire needs no
/// third coordinate; the scene calls this on the points it receives.
pub fn landing_intensity(point: [i8; 2]) -> f32 {
    let (x, y) = (f32::from(point[0]), f32::from(point[1]));
    let radius = (x * x + y * y).sqrt();
    if radius < 0.5 {
        return 0.0;
    }
    (1.0 + 20.0 * (radius / 127.0).log10() / GONIO_WINDOW_DB).clamp(0.0, 1.0)
}
