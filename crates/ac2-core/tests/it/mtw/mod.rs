//! MTW engine: golden comparisons against scipy-built references and loopback tests with
//! synthetic signals whose answers are analytic.

use ac2_core::grid::LogGrid;
use ac2_core::mtw::{
    Averaging, ColumnSource, ConfigError, DepthPolicy, Ladder, Layout, MIN_FAST_LF_BLOCKS, Mtw,
    MtwConfig, MtwFrame, PairDecimator, SampleGate, StageAveraging, StageBins, Validity,
};
use ac2_testkit::golden::GoldenSet;
use num_complex::Complex64;
use std::f64::consts::PI;

mod fast_lf;
mod golden;
mod layout;
mod loopback;
mod stream;

// ---------------------------------------------------------------------------------------
// Signal helpers
// ---------------------------------------------------------------------------------------

/// Deterministic Gaussian noise (splitmix64 + Box–Muller).
struct Noise(u64);

impl Noise {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    fn gauss(&mut self) -> f64 {
        let (u1, u2) = (self.uniform(), self.uniform());
        (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos()
    }
    fn vec(&mut self, n: usize, rms: f64) -> Vec<f64> {
        (0..n).map(|_| rms * self.gauss()).collect()
    }
}

/// RBJ cookbook peaking EQ.
#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 3],
}

impl Biquad {
    fn peaking(fs: f64, f0: f64, q: f64, gain_db: f64) -> Self {
        let a_lin = 10f64.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * f0 / fs;
        let alpha = w0.sin() / (2.0 * q);
        let a0 = 1.0 + alpha / a_lin;
        Self {
            b: [
                (1.0 + alpha * a_lin) / a0,
                -2.0 * w0.cos() / a0,
                (1.0 - alpha * a_lin) / a0,
            ],
            a: [1.0, -2.0 * w0.cos() / a0, (1.0 - alpha / a_lin) / a0],
        }
    }
    fn filter(&self, x: &[f64]) -> Vec<f64> {
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        x.iter()
            .map(|&v| {
                let y = self.b[0] * v + self.b[1] * x1 + self.b[2] * x2
                    - self.a[1] * y1
                    - self.a[2] * y2;
                (x2, x1, y2, y1) = (x1, v, y1, y);
                y
            })
            .collect()
    }
    fn response(&self, fs: f64, f: f64) -> Complex64 {
        let z1 = Complex64::from_polar(1.0, -2.0 * PI * f / fs);
        let z2 = z1 * z1;
        (self.b[0] + self.b[1] * z1 + self.b[2] * z2)
            / (self.a[0] + self.a[1] * z1 + self.a[2] * z2)
    }
}

/// `y[n] = s[n − d]` (zero before the start), for either sign of `d`, over `[0, len)`, where
/// `s` is indexed from `-lead`.
fn delayed(s: &[f64], lead: usize, d: i64, len: usize) -> Vec<f64> {
    (0..len as i64)
        .map(|n| {
            let i = n - d + lead as i64;
            if i >= 0 && (i as usize) < s.len() {
                s[i as usize]
            } else {
                0.0
            }
        })
        .collect()
}

fn grid48() -> LogGrid {
    LogGrid::covering(48, 20.0, 20_000.0)
}

fn engine(sr: f64, averaging: Averaging, delay: i64) -> Mtw {
    engine_with(sr, averaging, DepthPolicy::EqualConfidence, delay)
}

fn engine_with(sr: f64, averaging: Averaging, depth: DepthPolicy, delay: i64) -> Mtw {
    Mtw::new(MtwConfig {
        sample_rate_hz: sr,
        ladder: Ladder::Standard,
        averaging,
        depth,
        grid: grid48(),
        delay_samples: delay as f64,
    })
    .expect("engine")
}

/// Full-rate samples needed for every stage to fill its FIFO.
fn samples_to_fill(m: &Mtw) -> usize {
    m.layout()
        .stages
        .iter()
        .zip(m.stage_averaging())
        .map(|(s, a)| {
            let blocks = match a {
                StageAveraging::Fifo { blocks } => blocks,
                StageAveraging::Exponential { .. } => 1,
            };
            (s.nfft + (blocks - 1) * s.hop) * s.factor + s.filter_len()
        })
        .max()
        .expect("stages")
}

/// Push in irregular chunks (the result must not depend on chunking).
fn push_chunked(m: &mut Mtw, start: u64, x: &[f64], y: &[f64]) {
    let sizes = [997usize, 64, 4096, 1, 333, 12_000, 256];
    let mut i = 0;
    let mut c = 0;
    while i < x.len() {
        let e = (i + sizes[c % sizes.len()]).min(x.len());
        m.push(start + i as u64, &x[i..e], &y[i..e], SampleGate::Accept)
            .expect("push");
        i = e;
        c += 1;
    }
}

fn bins_freqs(m: &Mtw, b: &StageBins) -> Vec<f64> {
    let df = m.layout().stages[b.stage].bin_hz;
    (b.lo..b.hi).map(|k| k as f64 * df).collect()
}

/// Mean of `h` over a column's bins, blended as the engine blends.
fn expected_column(m: &Mtw, src: &ColumnSource, h: impl Fn(f64) -> Complex64) -> Complex64 {
    let mean = |b: &StageBins| {
        let f = bins_freqs(m, b);
        f.iter().map(|&f| h(f)).sum::<Complex64>() / f.len() as f64
    };
    match src {
        ColumnSource::Stage(b) => mean(b),
        ColumnSource::Blend {
            deep,
            shallow,
            shallow_weight: w,
        } => mean(deep) * (1.0 - w) + mean(shallow) * *w,
        ColumnSource::None => Complex64::new(f64::NAN, f64::NAN),
    }
}

fn valid(f: &MtwFrame) -> impl Iterator<Item = usize> + '_ {
    (0..f.freq_hz.len()).filter(|&i| f.columns[i].validity == Validity::Valid)
}

fn wrap_deg(d: f64) -> f64 {
    (d + 180.0).rem_euclid(360.0) - 180.0
}

fn same_frame(a: &MtwFrame, b: &MtwFrame) -> bool {
    a.columns == b.columns
        && a.last_block_end == b.last_block_end
        && a.h1
            .iter()
            .zip(&b.h1)
            .all(|(p, q)| (p.re.to_bits(), p.im.to_bits()) == (q.re.to_bits(), q.im.to_bits()))
        && a.coherence
            .iter()
            .zip(&b.coherence)
            .all(|(p, q)| p.to_bits() == q.to_bits())
}
