//! Single-channel FFT spectrum: amplitude, PSD and band power (design Q4).
//!
//! Every frame is reduced to the one-sided *folded power* F_k = c_k·|X_k|² (c_k = 2 for
//! interior bins, 1 at DC and Nyquist). Averaging and peak hold act on F_k, i.e. on power,
//! and every displayed quantity is a fixed scaling of it:
//!
//! | quantity | formula |
//! |---|---|
//! | amplitude (FS RMS) | √F_k / S₁ |
//! | amplitude dBFS | 20·lg(√2 · amplitude) — a full-scale sine reads 0 dBFS |
//! | PSD (FS²/Hz) | F_k / (fs·S₂) |
//! | band power (FS²) | Σ_k w_k · PSD_k · Δf, w_k = fraction of bin k inside the band |
//! | band power dBFS | 10·lg(2 · power) |
//!
//! with S₁ = Σw, S₂ = Σw² of the periodic window and Δf = fs/N. Band power over bins that tile
//! [0, fs/2] completely equals the windowed signal's mean square Σ(w·x)²/S₂ (Parseval), and a
//! bin that straddles a band edge is shared between the two bands in proportion to overlap,
//! so no power is created or lost by banding.

use crate::rta::BandFraction;
use crate::window::{Window, WindowGains};
use num_complex::Complex64;
use realfft::{RealFftPlanner, RealToComplex};
use std::f64::consts::SQRT_2;
use std::fmt;
use std::ops::Range;
use std::sync::Arc;

/// Level in dBFS of a mean square or band power `p` (FS²): 0 dBFS = mean square of a
/// full-scale sine (½ FS²).
pub fn power_dbfs(p: f64) -> f64 {
    10.0 * (2.0 * p).log10()
}

/// Level in dBFS of an RMS value (FS): 0 dBFS = RMS of a full-scale sine.
pub fn rms_dbfs(rms: f64) -> f64 {
    20.0 * (rms * SQRT_2).log10()
}

/// How successive frames are combined (always on power).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Averaging {
    /// Each frame replaces the previous one.
    Off,
    /// Arithmetic mean of the last `frames` frames.
    Fifo {
        /// Number of frames in the window (≥ 1).
        frames: usize,
    },
    /// Exponential average with the given time constant. Until enough frames have arrived it
    /// is the plain mean of all frames so far, so it starts without a ramp from zero.
    Exponential {
        /// Time constant in seconds (> 0).
        time_constant_s: f64,
    },
}

/// Peak hold applied to the averaged spectrum, per bin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PeakHold {
    /// How long a new peak is held before it starts to decay (s); infinite holds forever.
    pub hold_s: f64,
    /// Decay rate after the hold time (dB/s, ≥ 0); 0 holds forever.
    pub decay_db_per_s: f64,
}

/// Analyzer configuration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpectrumConfig {
    /// Sample rate in Hz.
    pub fs: f64,
    /// FFT length N.
    pub n: usize,
    /// Samples between successive frames (fixed hop; hop < N overlaps).
    pub hop: usize,
    /// Analysis window.
    pub window: Window,
    /// Frame averaging.
    pub averaging: Averaging,
    /// Optional peak hold on the averaged spectrum.
    pub peak_hold: Option<PeakHold>,
}

/// Invalid analyzer configuration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpectrumError {
    /// FFT length below 2.
    FftLength(usize),
    /// Hop of zero.
    Hop(usize),
    /// Sample rate not positive and finite.
    SampleRate(f64),
    /// FIFO of zero frames or a time constant that is not positive and finite.
    Averaging,
    /// Negative/NaN hold time or decay rate.
    PeakHold,
}

impl fmt::Display for SpectrumError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpectrumError::FftLength(n) => write!(f, "FFT length {n} is below 2"),
            SpectrumError::Hop(h) => write!(f, "hop {h} must be at least 1"),
            SpectrumError::SampleRate(fs) => write!(f, "invalid sample rate {fs} Hz"),
            SpectrumError::Averaging => write!(f, "invalid averaging parameters"),
            SpectrumError::PeakHold => write!(f, "invalid peak-hold parameters"),
        }
    }
}

impl std::error::Error for SpectrumError {}

/// Read-only view of a one-sided power spectrum with its normalisation.
#[derive(Debug, Clone, Copy)]
pub struct PowerSpectrum<'a> {
    folded: &'a [f64],
    fs: f64,
    n: usize,
    gains: WindowGains,
}

impl<'a> PowerSpectrum<'a> {
    /// Number of one-sided bins, N/2 + 1 (floor).
    pub fn bins(&self) -> usize {
        self.folded.len()
    }

    /// Centre frequency of bin `k`.
    pub fn freq(&self, k: usize) -> f64 {
        k as f64 * self.fs / self.n as f64
    }

    /// Bin spacing Δf = fs/N.
    pub fn bin_width(&self) -> f64 {
        self.fs / self.n as f64
    }

    /// Folded power F_k = c_k·|X_k|² (unnormalised).
    pub fn folded(&self) -> &'a [f64] {
        self.folded
    }

    /// Amplitude of bin `k` in FS RMS: a bin-centred sine reads its RMS.
    pub fn amplitude_rms(&self, k: usize) -> f64 {
        self.folded[k].sqrt() / self.gains.sum
    }

    /// Amplitude of bin `k` in dBFS (tone).
    pub fn amplitude_dbfs(&self, k: usize) -> f64 {
        rms_dbfs(self.amplitude_rms(k))
    }

    /// One-sided PSD of bin `k` in FS²/Hz.
    pub fn psd(&self, k: usize) -> f64 {
        self.folded[k] / (self.fs * self.gains.sum_sq)
    }

    /// PSD of bin `k` in dB re 1 FS²/Hz.
    pub fn psd_db(&self, k: usize) -> f64 {
        10.0 * self.psd(k).log10()
    }

    /// Total power Σ_k PSD_k·Δf in FS² (the windowed mean square, by Parseval).
    pub fn total_power(&self) -> f64 {
        self.folded.iter().sum::<f64>() / (self.n as f64 * self.gains.sum_sq)
    }

    /// Band power in FS² for each band of `banding`; NaN for bands whose status is not
    /// [`BandStatus::Valid`].
    ///
    /// # Panics
    /// If `banding` was built for another N or rate, or `out.len() != banding.len()`.
    pub fn band_powers(&self, banding: &FftBanding, out: &mut [f64]) {
        assert!(
            banding.n == self.n && banding.fs == self.fs,
            "banding built for N={} fs={}, spectrum has N={} fs={}",
            banding.n,
            banding.fs,
            self.n,
            self.fs
        );
        assert_eq!(out.len(), banding.bands.len(), "output length");
        let scale = 1.0 / (self.n as f64 * self.gains.sum_sq);
        for (o, band) in out.iter_mut().zip(&banding.bands) {
            *o = match band.status {
                BandStatus::Valid => {
                    let w = &banding.weights[band.weights.clone()];
                    let f = &self.folded[band.first_bin..band.first_bin + w.len()];
                    w.iter().zip(f).map(|(w, f)| w * f).sum::<f64>() * scale
                }
                BandStatus::InsufficientResolution | BandStatus::AboveNyquist => f64::NAN,
            };
        }
    }
}

/// Why a band has (or lacks) a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BandStatus {
    /// Band power is computed.
    Valid,
    /// The band is narrower than two bins; shown as "insufficient resolution", never
    /// interpolated.
    InsufficientResolution,
    /// The band extends above Nyquist.
    AboveNyquist,
}

/// One band of an FFT banding.
#[derive(Debug, Clone, PartialEq)]
pub struct FftBand {
    /// Centre frequency in Hz (IEC exact mid-band, or geometric mean of the edges).
    pub centre_hz: f64,
    /// Lower edge in Hz.
    pub lower_hz: f64,
    /// Upper edge in Hz.
    pub upper_hz: f64,
    /// Whether the band has a value.
    pub status: BandStatus,
    first_bin: usize,
    weights: Range<usize>,
}

/// Fractional-bin weights mapping an N-point spectrum at rate fs onto frequency bands.
///
/// Bin k represents the interval [(k − ½)Δf, (k + ½)Δf] clipped to [0, fs/2] (so the DC and
/// Nyquist bins have half width but full weight, which keeps Parseval exact). Its weight in
/// a band is the fraction of that interval inside the band.
#[derive(Debug, Clone, PartialEq)]
pub struct FftBanding {
    n: usize,
    fs: f64,
    bands: Vec<FftBand>,
    weights: Vec<f64>,
}

impl FftBanding {
    /// IEC 61260-1 base-10 bands of `fraction` with exact mid-band frequency in
    /// `[f_lo, f_hi]`, for an `n`-point FFT at rate `fs`.
    pub fn iec(fraction: BandFraction, f_lo: f64, f_hi: f64, n: usize, fs: f64) -> Self {
        let specs = fraction.centres(f_lo, f_hi).into_iter().map(|fm| {
            let (lo, hi) = fraction.edges(fm);
            (fm, lo, hi)
        });
        Self::build(specs, n, fs)
    }

    /// Contiguous bands between successive `edges` (ascending, Hz). Centres are geometric
    /// means of the edges (arithmetic for a band starting at 0 Hz).
    ///
    /// # Panics
    /// If fewer than two edges are given or they are not strictly increasing from ≥ 0.
    pub fn from_edges(edges: &[f64], n: usize, fs: f64) -> Self {
        assert!(edges.len() >= 2, "need at least two edges");
        assert!(
            edges[0] >= 0.0 && edges.windows(2).all(|w| w[1] > w[0]),
            "edges must be ascending from >= 0"
        );
        let specs = edges.windows(2).map(|w| {
            let c = if w[0] > 0.0 {
                (w[0] * w[1]).sqrt()
            } else {
                w[1] / 2.0
            };
            (c, w[0], w[1])
        });
        Self::build(specs, n, fs)
    }

    fn build(specs: impl Iterator<Item = (f64, f64, f64)>, n: usize, fs: f64) -> Self {
        assert!(n >= 2 && fs > 0.0, "invalid FFT size {n} or rate {fs}");
        let df = fs / n as f64;
        let nyq = fs / 2.0;
        let bins = n / 2 + 1;
        let mut bands = Vec::new();
        let mut weights = Vec::new();
        for (centre, lo, hi) in specs {
            let status = if hi > nyq * (1.0 + 1e-12) {
                BandStatus::AboveNyquist
            } else if hi - lo < 2.0 * df {
                BandStatus::InsufficientResolution
            } else {
                BandStatus::Valid
            };
            let start = weights.len();
            let mut first_bin = 0;
            if status == BandStatus::Valid {
                let k0 = ((lo / df - 0.5).floor().max(0.0)) as usize;
                let k1 = (((hi / df + 0.5).ceil()) as usize).min(bins - 1);
                first_bin = k0;
                for k in k0..=k1 {
                    let b_lo = ((k as f64 - 0.5) * df).max(0.0);
                    let b_hi = ((k as f64 + 0.5) * df).min(nyq);
                    let overlap = (b_hi.min(hi) - b_lo.max(lo)).max(0.0);
                    weights.push(overlap / (b_hi - b_lo));
                }
            }
            bands.push(FftBand {
                centre_hz: centre,
                lower_hz: lo,
                upper_hz: hi,
                status,
                first_bin,
                weights: start..weights.len(),
            });
        }
        Self {
            n,
            fs,
            bands,
            weights,
        }
    }

    /// The bands, low to high.
    pub fn bands(&self) -> &[FftBand] {
        &self.bands
    }

    /// Number of bands.
    pub fn len(&self) -> usize {
        self.bands.len()
    }

    /// True if there are no bands.
    pub fn is_empty(&self) -> bool {
        self.bands.is_empty()
    }

    /// FFT length this banding is for.
    pub fn n(&self) -> usize {
        self.n
    }

    /// Sample rate this banding is for.
    pub fn fs(&self) -> f64 {
        self.fs
    }
}

#[derive(Clone)]
struct Fft(Arc<dyn RealToComplex<f64>>);

impl fmt::Debug for Fft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RealToComplex(len={})", self.0.len())
    }
}

/// Streaming single-channel spectrum analyzer: push blocks of any size; a frame of N samples
/// is analysed every `hop` samples once N samples have arrived.
///
/// Pushing does not allocate.
#[derive(Debug, Clone)]
pub struct SpectrumAnalyzer {
    cfg: SpectrumConfig,
    window: Vec<f64>,
    gains: WindowGains,
    fold: Vec<f64>,
    fft: Fft,
    fft_in: Vec<f64>,
    fft_out: Vec<Complex64>,
    scratch: Vec<Complex64>,
    ring: Vec<f64>,
    write: usize,
    total: u64,
    next_frame_at: u64,
    frame: Vec<f64>,
    avg: Vec<f64>,
    frames_averaged: u64,
    fifo: Vec<f64>,
    fifo_sum: Vec<f64>,
    fifo_len: usize,
    fifo_pos: usize,
    peak: Vec<f64>,
    peak_age_s: Vec<f64>,
}

impl SpectrumAnalyzer {
    /// Creates an analyzer. All buffers are allocated here.
    pub fn new(cfg: SpectrumConfig) -> Result<Self, SpectrumError> {
        if cfg.n < 2 {
            return Err(SpectrumError::FftLength(cfg.n));
        }
        if cfg.hop == 0 {
            return Err(SpectrumError::Hop(cfg.hop));
        }
        if !(cfg.fs.is_finite() && cfg.fs > 0.0) {
            return Err(SpectrumError::SampleRate(cfg.fs));
        }
        let fifo_frames = match cfg.averaging {
            Averaging::Off => 0,
            Averaging::Fifo { frames } if frames >= 1 => frames,
            Averaging::Exponential { time_constant_s }
                if time_constant_s.is_finite() && time_constant_s > 0.0 =>
            {
                0
            }
            Averaging::Fifo { .. } | Averaging::Exponential { .. } => {
                return Err(SpectrumError::Averaging);
            }
        };
        if let Some(p) = cfg.peak_hold
            && !(p.hold_s >= 0.0 && p.decay_db_per_s >= 0.0 && p.decay_db_per_s.is_finite())
        {
            return Err(SpectrumError::PeakHold);
        }
        let n = cfg.n;
        let bins = n / 2 + 1;
        let window = cfg.window.coefficients(n);
        let gains = WindowGains::of(&window);
        let mut fold = vec![2.0; bins];
        fold[0] = 1.0;
        if n.is_multiple_of(2) {
            fold[bins - 1] = 1.0;
        }
        let fft = RealFftPlanner::<f64>::new().plan_fft_forward(n);
        let fft_in = fft.make_input_vec();
        let fft_out = fft.make_output_vec();
        let scratch = fft.make_scratch_vec();
        let peak_len = if cfg.peak_hold.is_some() { bins } else { 0 };
        Ok(Self {
            window,
            gains,
            fold,
            fft: Fft(fft),
            fft_in,
            fft_out,
            scratch,
            ring: vec![0.0; n],
            write: 0,
            total: 0,
            next_frame_at: n as u64,
            frame: vec![0.0; bins],
            avg: vec![0.0; bins],
            frames_averaged: 0,
            fifo: vec![0.0; fifo_frames * bins],
            fifo_sum: vec![0.0; if fifo_frames > 0 { bins } else { 0 }],
            fifo_len: 0,
            fifo_pos: 0,
            peak: vec![0.0; peak_len],
            peak_age_s: vec![0.0; peak_len],
            cfg,
        })
    }

    /// Configuration.
    pub fn config(&self) -> &SpectrumConfig {
        &self.cfg
    }

    /// Window gains S₁, S₂.
    pub fn gains(&self) -> WindowGains {
        self.gains
    }

    /// Number of one-sided bins.
    pub fn bins(&self) -> usize {
        self.avg.len()
    }

    /// Frames analysed since the averaging was last reset.
    pub fn frames(&self) -> u64 {
        self.frames_averaged
    }

    /// Pushes samples; returns how many frames were analysed during this call.
    pub fn push(&mut self, mut block: &[f64]) -> usize {
        let mut frames = 0;
        while !block.is_empty() {
            let until = (self.next_frame_at - self.total) as usize;
            let take = until.min(block.len());
            self.write_ring(&block[..take]);
            self.total += take as u64;
            block = &block[take..];
            if self.total == self.next_frame_at {
                self.analyse_frame();
                self.next_frame_at += self.cfg.hop as u64;
                frames += 1;
            }
        }
        frames
    }

    fn write_ring(&mut self, mut x: &[f64]) {
        let n = self.ring.len();
        while !x.is_empty() {
            let take = (n - self.write).min(x.len());
            self.ring[self.write..self.write + take].copy_from_slice(&x[..take]);
            self.write = (self.write + take) % n;
            x = &x[take..];
        }
    }

    fn analyse_frame(&mut self) {
        // The ring is full; `write` points at the oldest sample.
        let n = self.ring.len();
        let (newer, older) = self.ring.split_at(self.write);
        for (j, &v) in older.iter().chain(newer).enumerate() {
            self.fft_in[j] = v * self.window[j];
        }
        debug_assert_eq!(older.len() + newer.len(), n);
        // realfft only fails on wrong buffer lengths, which are fixed at construction.
        if self
            .fft
            .0
            .process_with_scratch(&mut self.fft_in, &mut self.fft_out, &mut self.scratch)
            .is_err()
        {
            unreachable!("FFT buffers are sized by the planner");
        }
        for ((p, x), c) in self.frame.iter_mut().zip(&self.fft_out).zip(&self.fold) {
            *p = c * x.norm_sqr();
        }
        self.accumulate();
    }

    fn accumulate(&mut self) {
        self.frames_averaged += 1;
        let bins = self.frame.len();
        match self.cfg.averaging {
            Averaging::Off => self.avg.copy_from_slice(&self.frame),
            Averaging::Exponential { time_constant_s } => {
                let dt = self.cfg.hop as f64 / self.cfg.fs;
                let alpha =
                    (1.0 - (-dt / time_constant_s).exp()).max(1.0 / self.frames_averaged as f64);
                for (a, f) in self.avg.iter_mut().zip(&self.frame) {
                    *a += alpha * (f - *a);
                }
            }
            Averaging::Fifo { frames } => {
                let slot = &mut self.fifo[self.fifo_pos * bins..(self.fifo_pos + 1) * bins];
                if self.fifo_len == frames {
                    for (s, old) in self.fifo_sum.iter_mut().zip(slot.iter()) {
                        *s -= old;
                    }
                } else {
                    self.fifo_len += 1;
                }
                slot.copy_from_slice(&self.frame);
                for (s, new) in self.fifo_sum.iter_mut().zip(&self.frame) {
                    *s += new;
                }
                self.fifo_pos = (self.fifo_pos + 1) % frames;
                if self.fifo_pos == 0 {
                    // Re-sum from the stored frames once per cycle so subtract/add rounding
                    // cannot drift without bound.
                    self.fifo_sum.fill(0.0);
                    for frame in self.fifo.chunks_exact(bins).take(self.fifo_len) {
                        for (s, v) in self.fifo_sum.iter_mut().zip(frame) {
                            *s += v;
                        }
                    }
                }
                let inv = 1.0 / self.fifo_len as f64;
                for (a, s) in self.avg.iter_mut().zip(&self.fifo_sum) {
                    *a = s * inv;
                }
            }
        }
        if let Some(ph) = self.cfg.peak_hold {
            let dt = self.cfg.hop as f64 / self.cfg.fs;
            let decay = 10f64.powf(-ph.decay_db_per_s * dt / 10.0);
            let first = self.frames_averaged == 1;
            for ((p, age), &a) in self
                .peak
                .iter_mut()
                .zip(&mut self.peak_age_s)
                .zip(&self.avg)
            {
                if first || a >= *p {
                    *p = a;
                    *age = 0.0;
                } else {
                    *age += dt;
                    if *age > ph.hold_s {
                        *p = (*p * decay).max(a);
                    }
                }
            }
        }
    }

    fn view<'a>(&self, folded: &'a [f64]) -> PowerSpectrum<'a> {
        PowerSpectrum {
            folded,
            fs: self.cfg.fs,
            n: self.cfg.n,
            gains: self.gains,
        }
    }

    /// The averaged spectrum; `None` before the first frame.
    pub fn average(&self) -> Option<PowerSpectrum<'_>> {
        (self.frames_averaged > 0).then(|| self.view(&self.avg))
    }

    /// The most recent single frame; `None` before the first frame.
    pub fn latest(&self) -> Option<PowerSpectrum<'_>> {
        (self.frames_averaged > 0).then(|| self.view(&self.frame))
    }

    /// The peak-hold spectrum; `None` without peak hold or before the first frame.
    pub fn peak(&self) -> Option<PowerSpectrum<'_>> {
        (self.cfg.peak_hold.is_some() && self.frames_averaged > 0).then(|| self.view(&self.peak))
    }

    /// Restarts averaging and peak hold; input history is kept, so the next frame comes at
    /// the regular hop.
    pub fn reset_average(&mut self) {
        self.frames_averaged = 0;
        self.fifo_len = 0;
        self.fifo_pos = 0;
        self.fifo_sum.fill(0.0);
        self.avg.fill(0.0);
        self.peak.fill(0.0);
        self.peak_age_s.fill(0.0);
    }

    /// Restarts peak hold from the current average.
    pub fn reset_peak(&mut self) {
        self.peak.copy_from_slice(&self.avg);
        self.peak_age_s.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_testkit::golden::GoldenSet;
    use std::f64::consts::TAU;

    fn analyzer(fs: f64, n: usize, window: Window, averaging: Averaging) -> SpectrumAnalyzer {
        SpectrumAnalyzer::new(SpectrumConfig {
            fs,
            n,
            hop: n,
            window,
            averaging,
            peak_hold: None,
        })
        .expect("config")
    }

    /// Deterministic Gaussian noise (xorshift64* + Box–Muller); statistics only, no golden.
    struct Noise(u64);
    impl Noise {
        fn uniform(&mut self) -> f64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            ((self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        }
        fn gauss(&mut self) -> f64 {
            let (u1, u2) = (self.uniform(), self.uniform());
            (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()
        }
    }

    #[test]
    fn tone_noise_matches_golden() {
        let g = GoldenSet::load("spectrum_hann_tone_noise").expect("golden");
        let x = g.f64("x").expect("x");
        let n = x.len();
        let mut a = analyzer(48_000.0, n, Window::Hann, Averaging::Off);
        assert_eq!(a.push(&x), 1);
        let s = a.average().expect("frame");
        let f: Vec<f64> = (0..s.bins()).map(|k| s.freq(k)).collect();
        let amp: Vec<f64> = (0..s.bins()).map(|k| s.amplitude_rms(k)).collect();
        let amp_db: Vec<f64> = (0..s.bins()).map(|k| s.amplitude_dbfs(k)).collect();
        let psd: Vec<f64> = (0..s.bins()).map(|k| s.psd(k)).collect();
        let psd_db: Vec<f64> = (0..s.bins()).map(|k| s.psd_db(k)).collect();
        g.assert_f64("freq_hz", &f);
        g.assert_f64("amplitude_rms", &amp);
        g.assert_f64("amplitude_dbfs", &amp_db);
        g.assert_f64("psd", &psd);
        g.assert_f64("psd_db", &psd_db);
    }

    /// Off-bin tone at a half-bin offset: the spectrum matches the scipy/analytic golden and
    /// the peak reads exactly the window's scalloping loss below the tone level.
    #[test]
    fn half_bin_tone_reads_scalloping_loss() {
        let g = GoldenSet::load("spectrum_offbin_windows").expect("golden");
        let x = g.f64("x").expect("x");
        let fs = g.scalar("fs_hz").expect("fs");
        for (win, key) in [
            (Window::Hann, "hann"),
            (Window::BlackmanHarris4, "blackmanharris4"),
            (Window::FlatTop, "flattop_hft95"),
            (Window::Rectangular, "rectangular"),
        ] {
            let mut a = analyzer(fs, x.len(), win, Averaging::Off);
            a.push(&x);
            let s = a.average().expect("frame");
            let db: Vec<f64> = (0..s.bins()).map(|k| s.amplitude_dbfs(k)).collect();
            g.assert_f64(&format!("{key}_amplitude_dbfs"), &db);
            let peak = db.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let loss = -peak;
            let expected = g.scalar(&format!("{key}_scalloping_db")).expect("loss");
            assert!(
                (loss - expected).abs() < 1e-9,
                "{key}: {loss} vs {expected}"
            );
            assert!(
                (loss - win.max_scalloping_loss_db()).abs() < 0.01,
                "{key}: stated {} vs measured {loss}",
                win.max_scalloping_loss_db()
            );
        }
    }

    #[test]
    fn fft_banding_matches_golden_and_parseval() {
        let g = GoldenSet::load("fft_banding_noise").expect("golden");
        let x = g.f64("x").expect("x");
        let fs = g.scalar("fs_hz").expect("fs");
        let n = x.len();
        let mut a = analyzer(fs, n, Window::Hann, Averaging::Off);
        a.push(&x);
        let s = a.average().expect("frame");
        for (fr, key) in [
            (BandFraction::Octave, "oct"),
            (BandFraction::Third, "third"),
        ] {
            let banding = FftBanding::iec(fr, 15.0, 21_000.0, n, fs);
            let centres: Vec<f64> = banding.bands().iter().map(|b| b.centre_hz).collect();
            g.assert_f64(&format!("{key}_centre_hz"), &centres);
            let status: Vec<f64> = banding
                .bands()
                .iter()
                .map(|b| match b.status {
                    BandStatus::Valid => 0.0,
                    BandStatus::InsufficientResolution => 1.0,
                    BandStatus::AboveNyquist => 2.0,
                })
                .collect();
            g.assert_f64(&format!("{key}_status"), &status);
            let mut p = vec![0.0; banding.len()];
            s.band_powers(&banding, &mut p);
            let p0: Vec<f64> = p
                .iter()
                .map(|v| if v.is_nan() { 0.0 } else { *v })
                .collect();
            g.assert_f64(&format!("{key}_band_power"), &p0);
        }
        // Parseval: bands tiling [0, fs/2] sum to the windowed mean square.
        let mut edges = vec![0.0];
        let third = FftBanding::iec(BandFraction::Third, 100.0, 20_000.0, n, fs);
        edges.extend(third.bands().iter().map(|b| b.lower_hz));
        edges.push(third.bands().last().expect("band").upper_hz);
        edges.push(fs / 2.0);
        let tiling = FftBanding::from_edges(&edges, n, fs);
        assert!(tiling.bands().iter().all(|b| b.status == BandStatus::Valid));
        let mut p = vec![0.0; tiling.len()];
        s.band_powers(&tiling, &mut p);
        let total: f64 = p.iter().sum();
        let expected = g.scalar("windowed_mean_square").expect("ms");
        assert!(
            (total / expected - 1.0).abs() < 1e-9,
            "{total} vs {expected}"
        );
        assert!((s.total_power() / expected - 1.0).abs() < 1e-12);
    }

    #[test]
    fn narrow_and_out_of_range_bands_are_nan_with_reason() {
        let banding = FftBanding::iec(BandFraction::Third, 19.0, 21_000.0, 4096, 44_100.0);
        let b = banding.bands();
        assert_eq!(b[0].status, BandStatus::InsufficientResolution);
        assert_eq!(b.last().expect("band").status, BandStatus::AboveNyquist);
        let x = vec![0.1; 4096];
        let mut a = analyzer(44_100.0, 4096, Window::Hann, Averaging::Off);
        a.push(&x);
        let mut p = vec![0.0; banding.len()];
        a.average().expect("frame").band_powers(&banding, &mut p);
        assert!(p[0].is_nan() && p[p.len() - 1].is_nan());
        assert!(p[10].is_finite());
    }

    /// White noise: band power is independent of N (statistical tolerance), and equals
    /// σ²·B/(fs/2).
    #[test]
    fn white_noise_band_power_independent_of_n() {
        let fs = 48_000.0;
        let sigma = 0.1;
        let total = 1 << 22;
        let mut rng = Noise(0x0AC2_5EED);
        let x: Vec<f64> = (0..total).map(|_| sigma * rng.gauss()).collect();
        for n in [1024, 4096, 16_384, 65_536] {
            let mut a = analyzer(fs, n, Window::Hann, Averaging::Fifo { frames: total / n });
            a.push(&x);
            let banding = FftBanding::iec(BandFraction::Octave, 250.0, 8000.0, n, fs);
            let mut p = vec![0.0; banding.len()];
            a.average().expect("avg").band_powers(&banding, &mut p);
            for (b, p) in banding.bands().iter().zip(&p) {
                let bw = b.upper_hz - b.lower_hz;
                let expect = sigma * sigma * bw / (fs / 2.0);
                let err_db = 10.0 * (p / expect).log10();
                // Relative standard deviation of a band-power estimate ≈ 1/√(B·T_eff); with
                // non-overlapped Hann frames T_eff is about half the record. Allow 4σ.
                let t_eff = 0.5 * total as f64 / fs;
                let tol_db = 10.0 * (1.0 + 4.0 / (bw * t_eff).sqrt()).log10();
                assert!(
                    err_db.abs() < tol_db,
                    "N={n} band {:.0}: {err_db:.3} dB (tol {tol_db:.3})",
                    b.centre_hz
                );
            }
        }
    }

    /// Periodic pink noise (1/f power, random phases, period 2²⁰): band power is
    /// independent of N and matches the components' power in the band.
    #[test]
    fn pink_noise_band_power_independent_of_n() {
        let fs = 48_000.0;
        let len = 1usize << 20;
        let mut rng = Noise(0x0AC2_D1CE);
        let mut planner = RealFftPlanner::<f64>::new();
        let inv = planner.plan_fft_inverse(len);
        let mut spec = inv.make_input_vec();
        let df = fs / len as f64;
        // Component k has amplitude a_k ∝ 1/√f_k, so its power a_k²/2 ∝ 1/f_k.
        let comp_power = |k: usize| 1e-3 / (k as f64 * df);
        for (k, c) in spec.iter_mut().enumerate().take(len / 2).skip(1) {
            let amp = (2.0 * comp_power(k)).sqrt();
            *c = Complex64::from_polar(amp * len as f64 / 2.0, TAU * rng.uniform());
        }
        let mut x = inv.make_output_vec();
        inv.process(&mut spec, &mut x).expect("ifft");
        // inverse realfft is unnormalised: x[n] = Σ_k a_k cos(...) with the scaling above.
        for v in &mut x {
            *v /= len as f64;
        }
        for n in [1024, 4096, 16_384, 65_536] {
            let mut a = analyzer(fs, n, Window::Hann, Averaging::Fifo { frames: len / n });
            a.push(&x);
            let banding = FftBanding::iec(BandFraction::Third, 500.0, 8000.0, n, fs);
            let mut p = vec![0.0; banding.len()];
            a.average().expect("avg").band_powers(&banding, &mut p);
            for (b, p) in banding.bands().iter().zip(&p) {
                let k0 = (b.lower_hz / df).ceil() as usize;
                let k1 = (b.upper_hz / df).floor() as usize;
                let expect: f64 = (k0..=k1).map(comp_power).sum();
                let err_db = 10.0 * (p / expect).log10();
                assert!(
                    err_db.abs() < 0.2,
                    "N={n} band {:.0}: {err_db:.3} dB",
                    b.centre_hz
                );
            }
        }
    }

    #[test]
    fn streaming_hop_and_block_independence() {
        let fs = 48_000.0;
        let n = 1024;
        let hop = 256;
        let mut rng = Noise(7);
        let x: Vec<f64> = (0..10_000).map(|_| rng.gauss()).collect();
        let cfg = SpectrumConfig {
            fs,
            n,
            hop,
            window: Window::Hann,
            averaging: Averaging::Fifo { frames: 8 },
            peak_hold: None,
        };
        let mut one = SpectrumAnalyzer::new(cfg).expect("cfg");
        let mut many = SpectrumAnalyzer::new(cfg).expect("cfg");
        let frames = one.push(&x);
        assert_eq!(frames, (x.len() - n) / hop + 1);
        let mut count = 0;
        for chunk in x.chunks(97) {
            count += many.push(chunk);
        }
        assert_eq!(count, frames);
        let (a, b) = (one.average().expect("a"), many.average().expect("b"));
        assert_eq!(a.folded(), b.folded());
        // FIFO of 8 equals the mean of the last 8 single frames computed directly.
        let mut direct = vec![0.0; n / 2 + 1];
        for f in 0..8 {
            let end = n + (frames - 1 - f) * hop;
            let mut single = analyzer(fs, n, Window::Hann, Averaging::Off);
            single.push(&x[end - n..end]);
            for (d, v) in direct.iter_mut().zip(single.average().expect("s").folded()) {
                *d += v / 8.0;
            }
        }
        for (d, v) in direct.iter().zip(a.folded()) {
            assert!((d - v).abs() <= 1e-9 * d.abs().max(1e-12), "{d} vs {v}");
        }
    }

    #[test]
    fn exponential_average_converges_to_mean() {
        let fs = 48_000.0;
        let n = 512;
        let mut rng = Noise(11);
        let x: Vec<f64> = (0..n * 2000).map(|_| 0.5 * rng.gauss()).collect();
        let mut a = analyzer(
            fs,
            n,
            Window::Hann,
            Averaging::Exponential {
                time_constant_s: 2.0,
            },
        );
        a.push(&x);
        let s = a.average().expect("avg");
        // White noise PSD σ²/(fs/2); 2 s time constant ≈ 375 frames → ~5 % per bin, so
        // compare the in-band mean.
        let mean: f64 = (10..200).map(|k| s.psd(k)).sum::<f64>() / 190.0;
        let expect = 0.25 / (fs / 2.0);
        assert!((mean / expect - 1.0).abs() < 0.02, "{mean} vs {expect}");
        // First frame is not ramped from zero.
        let mut b = analyzer(
            fs,
            n,
            Window::Hann,
            Averaging::Exponential {
                time_constant_s: 10.0,
            },
        );
        b.push(&x[..n]);
        let mut c = analyzer(fs, n, Window::Hann, Averaging::Off);
        c.push(&x[..n]);
        assert_eq!(
            b.average().expect("b").folded(),
            c.average().expect("c").folded()
        );
    }

    #[test]
    fn peak_hold_holds_then_decays() {
        let fs = 48_000.0;
        let n = 1024;
        let hold_s = 0.5;
        let decay = 20.0;
        let mut a = SpectrumAnalyzer::new(SpectrumConfig {
            fs,
            n,
            hop: n,
            window: Window::Hann,
            averaging: Averaging::Off,
            peak_hold: Some(PeakHold {
                hold_s,
                decay_db_per_s: decay,
            }),
        })
        .expect("cfg");
        let k = 64;
        let f = k as f64 * fs / n as f64;
        let loud: Vec<f64> = (0..n)
            .map(|i| 0.5 * (TAU * f * i as f64 / fs).sin())
            .collect();
        let quiet: Vec<f64> = (0..n)
            .map(|i| 0.005 * (TAU * f * i as f64 / fs).sin())
            .collect();
        a.push(&loud);
        let peak0 = a.peak().expect("peak").amplitude_dbfs(k);
        assert!((peak0 - rms_dbfs(0.5 / SQRT_2)).abs() < 1e-9);
        let dt = n as f64 / fs;
        let frames_held = (hold_s / dt).floor() as usize;
        for _ in 0..frames_held {
            a.push(&quiet);
        }
        assert_eq!(a.peak().expect("p").amplitude_dbfs(k), peak0);
        let extra = 10;
        for _ in 0..extra {
            a.push(&quiet);
        }
        let dropped = peak0 - a.peak().expect("p").amplitude_dbfs(k);
        assert!(
            (dropped - decay * dt * extra as f64).abs() < 1e-9,
            "{dropped}"
        );
        // Decay never goes below the live average.
        for _ in 0..1000 {
            a.push(&quiet);
        }
        let p = a.peak().expect("p").amplitude_dbfs(k);
        let live = a.average().expect("a").amplitude_dbfs(k);
        assert!((p - live).abs() < 1e-9);
    }

    #[test]
    fn dc_and_nyquist_not_doubled() {
        let n = 64;
        let x: Vec<f64> = (0..n)
            .map(|i| 0.25 + if i % 2 == 0 { 0.5 } else { -0.5 })
            .collect();
        let mut a = analyzer(1000.0, n, Window::Rectangular, Averaging::Off);
        a.push(&x);
        let s = a.average().expect("s");
        assert!((s.amplitude_rms(0) - 0.25).abs() < 1e-12);
        assert!((s.amplitude_rms(n / 2) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn invalid_configs_rejected() {
        let base = SpectrumConfig {
            fs: 48_000.0,
            n: 1024,
            hop: 512,
            window: Window::Hann,
            averaging: Averaging::Off,
            peak_hold: None,
        };
        assert!(matches!(
            SpectrumAnalyzer::new(SpectrumConfig { n: 1, ..base }),
            Err(SpectrumError::FftLength(1))
        ));
        assert!(matches!(
            SpectrumAnalyzer::new(SpectrumConfig { hop: 0, ..base }),
            Err(SpectrumError::Hop(0))
        ));
        assert!(matches!(
            SpectrumAnalyzer::new(SpectrumConfig {
                averaging: Averaging::Fifo { frames: 0 },
                ..base
            }),
            Err(SpectrumError::Averaging)
        ));
        assert!(matches!(
            SpectrumAnalyzer::new(SpectrumConfig {
                peak_hold: Some(PeakHold {
                    hold_s: -1.0,
                    decay_db_per_s: 1.0
                }),
                ..base
            }),
            Err(SpectrumError::PeakHold)
        ));
    }
}
