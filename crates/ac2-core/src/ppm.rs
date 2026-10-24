//! Programme meters (docs/design/programme-meter.md §3): IEC 60268-10 quasi-peak detectors
//! (Type I, Type II), the IEC 60268-17 VU mean, and the BS.1770-5 Annex 2 true peak.
//! Over detection and max hold are the daemon's; this module only reads the signal.
//!
//! Every reading is linear (full scale = 1). The quasi-peak and true-peak readings are peaks:
//! `20·log10` of them is dBFS / dBTP ([`dbtp`], [`crate::protection::peak_dbfs`]). The VU
//! reading is scaled so a steady sine reads its RMS: convert with
//! [`crate::spectrum::rms_dbfs`], the convention of the `rms` meter field, and a full-scale
//! sine reads 0 dBFS in every ballistic.

use crate::protection::peak_dbfs;

/// The dynamics of one quasi-peak type. The standards specify a type by a table of 5 kHz
/// tone-burst lengths and the reading each gives relative to a steady tone, not by one
/// integration time; a single first-order attack cannot hold both ends of a column, so the
/// attack is two first-order charges in parallel.
///
/// `tools/refgen/sets/ppm.py --fit` fits `a, τ₁, τ₂` to the table by least squares in dB
/// over the rows; the constants are frozen there and here, and `ppm.rs` is tested against
/// the generator's vectors (parity) and against the table (acceptance) separately.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuasiPeakSpec {
    /// Weight of the fast charge in the reading, `a·y₁ + (1 − a)·y₂`.
    pub a: f64,
    /// Time constant of the fast charge.
    pub tau1_s: f64,
    /// Time constant of the slow charge.
    pub tau2_s: f64,
    /// Return rate, linear in dB.
    pub release_db_per_s: f64,
}

impl QuasiPeakSpec {
    /// IEC 60268-10 Type I (Nordic, DIN scales). Table (burst → reading re steady tone):
    /// 100 ms 0 dB, 10 ms −1, 5 ms −2, 3 ms −4, 0.4 ms −15; return 20 dB in 1.7 s.
    /// Fit residuals per row: +0.00, +0.24, −0.30, +0.15, −0.01 dB.
    pub const TYPE1: QuasiPeakSpec = QuasiPeakSpec {
        a: 0.360601,
        tau1_s: 0.000794,
        tau2_s: 0.004907,
        release_db_per_s: 20.0 / 1.7,
    };

    /// IEC 60268-10 Type II (BBC, EBU scales). Table: 100 ms 0 dB, 10 ms −2, 5 ms −4,
    /// 1.5 ms −9, 0.5 ms −17; return 24 dB in 2.8 s.
    /// Fit residuals per row: −0.00, −0.06, +0.11, −0.14, +0.08 dB.
    pub const TYPE2: QuasiPeakSpec = QuasiPeakSpec {
        a: 0.433480,
        tau1_s: 0.001484,
        tau2_s: 0.010061,
        release_db_per_s: 24.0 / 2.8,
    };
}

/// One channel's quasi-peak detector. Per sample, identical to `tools/refgen/sets/ppm.py`:
///
/// ```text
/// h   = max(|x|, h·rel)                       peak-held rectifier, released at the return rate
/// yᵢ  = yᵢ + αᵢ·(h − yᵢ)    if h > yᵢ         two first-order charges toward the held peak
///     = yᵢ·rel              otherwise          otherwise the return
/// out = a·y₁ + (1 − a)·y₂
/// αᵢ  = 1 − e^(−1/(fs·τᵢ)),  rel = 10^(−release_db_per_s / (20·fs))
/// ```
///
/// The rectified sine falls to zero twice a period; charging the states straight from `|x|`
/// makes a steady 5 kHz tone read 0.2 dB under its peak and leaves the Type I fit
/// degenerate. A real meter's rectifier bridges the troughs with a reservoir, so does the
/// hold, and a steady tone at 1 kHz or above reads its peak within 0.01 dB.
#[derive(Debug, Clone)]
pub struct QuasiPeak {
    a: f64,
    alpha1: f64,
    alpha2: f64,
    release: f64,
    held: f64,
    y1: f64,
    y2: f64,
}

impl QuasiPeak {
    pub fn new(sample_rate: f64, spec: &QuasiPeakSpec) -> Self {
        Self {
            a: spec.a,
            alpha1: 1.0 - (-1.0 / (sample_rate * spec.tau1_s)).exp(),
            alpha2: 1.0 - (-1.0 / (sample_rate * spec.tau2_s)).exp(),
            release: 10f64.powf(-spec.release_db_per_s / (20.0 * sample_rate)),
            held: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    /// Runs the block; returns the maximum reading within it (linear, full scale = 1).
    pub fn process(&mut self, x: &[f32]) -> f32 {
        let mut max = 0.0f64;
        for &v in x {
            let u = f64::from(v).abs();
            self.held = if u > self.held {
                u
            } else {
                self.held * self.release
            };
            let h = self.held;
            self.y1 = if h > self.y1 {
                self.y1 + self.alpha1 * (h - self.y1)
            } else {
                self.y1 * self.release
            };
            self.y2 = if h > self.y2 {
                self.y2 + self.alpha2 * (h - self.y2)
            } else {
                self.y2 * self.release
            };
            max = max.max(self.output());
        }
        max as f32
    }

    /// The reading after the last sample processed.
    pub fn reading(&self) -> f32 {
        self.output() as f32
    }

    fn output(&self) -> f64 {
        self.a * self.y1 + (1.0 - self.a) * self.y2
    }
}

/// IEC 60268-17 VU: a first-order mean of `|x|` reaching 99 % of a steady tone in 300 ms
/// (τ = 0.3 s / ln 100), scaled by π/(2√2) so a steady sine reads its RMS (the mean of
/// `|sin|` is 2/π of the peak).
#[derive(Debug, Clone)]
pub struct Vu {
    alpha: f64,
    mean: f64,
}

impl Vu {
    /// Settling time to 99 % of a steady tone.
    pub const SETTLE_S: f64 = 0.3;

    pub fn new(sample_rate: f64) -> Self {
        let tau = Self::SETTLE_S / 100f64.ln();
        Self {
            alpha: 1.0 - (-1.0 / (sample_rate * tau)).exp(),
            mean: 0.0,
        }
    }

    /// Runs the block; returns the reading after its last sample.
    pub fn process(&mut self, x: &[f32]) -> f32 {
        for &v in x {
            self.mean += self.alpha * (f64::from(v).abs() - self.mean);
        }
        self.reading()
    }

    pub fn reading(&self) -> f32 {
        (self.mean * std::f64::consts::PI / (2.0 * std::f64::consts::SQRT_2)) as f32
    }
}

/// Taps per phase of the Annex 2 interpolator (order 48, 4 phases).
pub const TRUE_PEAK_HISTORY: usize = 12;

/// BS.1770-5 Annex 2 order-48 4-phase interpolating FIR, `[phase][tap]`: the annex prints
/// the phases as columns; interpolated sample `4n + p` is `Σₘ PHASES[p][m]·x[n − m]`.
/// The 48 taps sum to the 4× upsampling gain (within the filter's ripple), so the output
/// needs no compensation. The literals are the annex's
/// (exact multiples of 2⁻¹³, so f32 holds them exactly), kept as printed to be checked
/// against it.
#[allow(clippy::excessive_precision)]
pub const TRUE_PEAK_PHASES: [[f32; TRUE_PEAK_HISTORY]; 4] = [
    [
        0.001_708_984_375,
        0.010_986_328_125,
        -0.019_653_320_312_5,
        0.033_203_125,
        -0.059_448_242_187_5,
        0.137_329_101_562_5,
        0.972_167_968_75,
        -0.102_294_921_875,
        0.047_607_421_875,
        -0.026_611_328_125,
        0.014_892_578_125,
        -0.008_300_781_25,
    ],
    [
        -0.029_174_804_687_5,
        0.029_296_875,
        -0.051_757_812_5,
        0.089_111_328_125,
        -0.166_503_906_25,
        0.465_087_890_625,
        0.779_785_156_25,
        -0.200_317_382_812_5,
        0.101_562_5,
        -0.058_227_539_062_5,
        0.033_081_054_687_5,
        -0.018_920_898_437_5,
    ],
    [
        -0.018_920_898_437_5,
        0.033_081_054_687_5,
        -0.058_227_539_062_5,
        0.101_562_5,
        -0.200_317_382_812_5,
        0.779_785_156_25,
        0.465_087_890_625,
        -0.166_503_906_25,
        0.089_111_328_125,
        -0.051_757_812_5,
        0.029_296_875,
        -0.029_174_804_687_5,
    ],
    [
        -0.008_300_781_25,
        0.014_892_578_125,
        -0.026_611_328_125,
        0.047_607_421_875,
        -0.102_294_921_875,
        0.972_167_968_75,
        0.137_329_101_562_5,
        -0.059_448_242_187_5,
        0.033_203_125,
        -0.019_653_320_312_5,
        0.010_986_328_125,
        0.001_708_984_375,
    ],
];

/// One channel's true-peak detector: the Annex 2 4× interpolator at every sample rate (at
/// 96 kHz that is more than the annex asks, which buys one code path and one set of
/// vectors). The reading is the largest `|interpolated sample|` of the block, all four
/// phases, with the last twelve input samples carried across blocks.
#[derive(Debug, Clone)]
pub struct TruePeak {
    history: [f32; TRUE_PEAK_HISTORY],
    pos: usize,
}

impl Default for TruePeak {
    fn default() -> Self {
        Self::new()
    }
}

impl TruePeak {
    pub fn new() -> Self {
        Self {
            history: [0.0; TRUE_PEAK_HISTORY],
            pos: 0,
        }
    }

    /// Runs the block; returns its true peak (linear, full scale = 1, may exceed 1).
    pub fn process(&mut self, x: &[f32]) -> f32 {
        let mut max = 0.0f32;
        for &v in x {
            // history[pos] is the newest sample x[n]; walking backwards gives x[n − m].
            self.history[self.pos] = v;
            for phase in &TRUE_PEAK_PHASES {
                let mut acc = 0.0f32;
                let mut i = self.pos;
                for &tap in phase {
                    acc += tap * self.history[i];
                    i = if i == 0 { TRUE_PEAK_HISTORY - 1 } else { i - 1 };
                }
                max = max.max(acc.abs());
            }
            self.pos = (self.pos + 1) % TRUE_PEAK_HISTORY;
        }
        max
    }
}

/// dB true peak: `20·log10` of a [`TruePeak`] reading, relative to full scale.
pub fn dbtp(linear: f32) -> f32 {
    peak_dbfs(f64::from(linear)) as f32
}
