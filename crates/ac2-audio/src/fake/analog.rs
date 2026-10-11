use super::config::FakeReverb;

/// Half-width of the drift interpolator, in samples.
pub(super) const SINC_HALF: usize = 16;
/// Fractional-delay phases of the drift interpolator. 1024 phases quantise the read
/// position to 1/2048 sample, far below anything a timing monitor resolves.
pub(super) const SINC_PHASES: usize = 1024;

/// Blackman-windowed sinc table, `SINC_PHASES + 1` rows of `2 * SINC_HALF` taps; row `p`
/// interpolates at fraction `p / SINC_PHASES` between taps `SINC_HALF - 1` and `SINC_HALF`.
pub(super) fn sinc_table() -> Box<[f32]> {
    let taps = 2 * SINC_HALF;
    let mut t = vec![0.0f32; (SINC_PHASES + 1) * taps];
    for p in 0..=SINC_PHASES {
        let frac = p as f64 / SINC_PHASES as f64;
        let row = &mut t[p * taps..(p + 1) * taps];
        let mut sum = 0.0;
        for (k, v) in row.iter_mut().enumerate() {
            let x = k as f64 - (SINC_HALF as f64 - 1.0) - frac;
            let sinc = if x == 0.0 {
                1.0
            } else {
                (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
            };
            let u = (x + SINC_HALF as f64) / (2.0 * SINC_HALF as f64);
            let w = 0.42 - 0.5 * (std::f64::consts::TAU * u).cos()
                + 0.08 * (2.0 * std::f64::consts::TAU * u).cos();
            *v = (sinc * w) as f32;
            sum += sinc * w;
        }
        // Unity DC gain in every phase, so drift does not modulate the level.
        for v in row.iter_mut() {
            *v = (f64::from(*v) / sum) as f32;
        }
    }
    t.into_boxed_slice()
}

/// One delay line of the reverberator: a feedback comb (`gain` = loop gain) or a
/// Schroeder all-pass (`gain` = its coefficient).
struct DelayLine {
    buf: Box<[f32]>,
    pos: usize,
    gain: f32,
}

impl DelayLine {
    fn new(len: usize, gain: f32) -> Self {
        Self {
            buf: vec![0.0; len.max(1)].into_boxed_slice(),
            pos: 0,
            gain,
        }
    }

    #[inline]
    fn comb(&mut self, x: f32) -> f32 {
        let y = self.buf[self.pos];
        self.buf[self.pos] = x + self.gain * y;
        self.pos = (self.pos + 1) % self.buf.len();
        y
    }

    #[inline]
    fn allpass(&mut self, x: f32) -> f32 {
        let v = self.buf[self.pos];
        let y = v - self.gain * x;
        self.buf[self.pos] = x + self.gain * y;
        self.pos = (self.pos + 1) % self.buf.len();
        y
    }
}

/// [`FakeReverb`] at a sample rate.
pub(super) struct Reverb {
    combs: Box<[DelayLine]>,
    allpasses: Box<[DelayLine]>,
    level: f32,
}

impl Reverb {
    /// Comb and all-pass lengths at 44.1 kHz (mutually prime-ish, as in the classic
    /// designs): echoes from different combs rarely coincide.
    const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
    const ALLPASSES: [usize; 4] = [556, 441, 341, 225];

    pub(super) fn new(r: FakeReverb, fs: f64) -> Self {
        let scale = fs / 44_100.0;
        let len = |d: usize| ((d as f64 * scale).round() as usize).max(1);
        Self {
            combs: Self::COMBS
                .iter()
                .map(|&d| {
                    let n = len(d);
                    // Energy −60 dB (amplitude 10⁻³) after t60: g^(t60·fs/n) = 10⁻³.
                    let g = 10f64.powf(-3.0 * n as f64 / (f64::from(r.t60_s) * fs));
                    DelayLine::new(n, g as f32)
                })
                .collect(),
            allpasses: Self::ALLPASSES
                .iter()
                .map(|&d| DelayLine::new(len(d), 0.5))
                .collect(),
            level: r.level / Self::COMBS.len() as f32,
        }
    }

    #[inline]
    pub(super) fn tick(&mut self, x: f32) -> f32 {
        let mut y = 0.0;
        for c in self.combs.iter_mut() {
            y += c.comb(x);
        }
        for a in self.allpasses.iter_mut() {
            y = a.allpass(y);
        }
        y * self.level
    }
}

/// `x + c2·x² + c3·x³ + …` by Horner's rule.
pub(super) fn shape(poly: &[f64], x: f32) -> f32 {
    if poly.is_empty() {
        return x;
    }
    let x = f64::from(x);
    let mut acc = 0.0;
    for c in poly.iter().rev() {
        acc = acc * x + c;
    }
    (x + acc * x * x) as f32
}
