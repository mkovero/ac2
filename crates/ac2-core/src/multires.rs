//! Multi-resolution single-channel spectrum: long windows at low frequencies, short ones at
//! high frequencies, every band at the full sample rate (`docs/design/multires-spectrum.md`).
//!
//! # Ladder
//! Six bands, window lengths `N_max, N_max/2, …, N_max/32`, where `N_max` is the shortest
//! power of two whose bin is no wider than [`LF_BIN_HZ`] (65 536 points at 96 kHz, 32 768 at
//! 48 kHz): the lowest frequencies keep the detail of a 65 536-point spectrum at 96 kHz while
//! the top band's window is about 21 ms at any common rate. No band is decimated, so every
//! band frames the same sample stream and a band's spectrum is of the `N_b` samples ending
//! exactly where its frame ends.
//!
//! Band placement follows the MTW ladder's rule ([`crate::mtw::layout`]): a window serves a
//! frequency only where one of its bins fits a column at the design density
//! ([`LADDER_DESIGN_PPO`]), `f ≥ κ · Δf_b`. Band `b` is the sole estimate at its *anchor*
//! `A_b = 2 · κ · Δf_b`, which is also where the next shorter band (bins twice as wide) reaches
//! its own validity edge, so between two anchors both bands resolve the design grid.
//!
//! # Level and blend
//! A column reports **tone level**, as the fixed spectrum does: the folded power of each bin
//! scaled by `2 / S₁²`, so a sine reads its level in every band whatever the window length.
//! Within a band a column takes the highest bin whose centre it covers (the bin under the
//! column centre when it covers none), as the fixed spectrum's display columns do.
//!
//! A tone-scaled bin reads a noise as PSD · ENBW · Δf_b: halving the window doubles the noise
//! reading. Between anchors `A_b < f < 2A_b` the column blends the two bands in power with the
//! shorter band's weight `w = f / A_b − 1`:
//!
//! `P(f) = (1 − w) · P_b + w · P_{b+1}`.
//!
//! A sine reads the same in both bands, so the blend keeps its level. A noise reads
//! `P_b · (1 + w) = P_b · f / A_b`, i.e. a noise bandwidth `B(f) = ENBW · f / (2κ)` that grows
//! in exact proportion to frequency from `A_0` to `A_5` (constant relative bandwidth: about
//! 1/64 octave for Hann), with no step anywhere: pink noise reads flat there, white noise
//! rises 3 dB per octave. Below `A_0` and above `A_5` one band serves and the noise bandwidth
//! is that band's constant `ENBW · Δf_b`.
//!
//! # Density
//! With [`SpectrumLevel::Density`] a column reports power spectral density instead: within
//! a band it takes the **mean** of the bins it covers (the maximum of several bins of a
//! noise reads above their mean), blends the bands with the same weights and divides by
//! the column's noise bandwidth [`MultiResLadder::noise_bandwidth_hz`],
//!
//! `D(f) = ((1 − w) · P̄_b + w · P̄_{b+1}) / B(f)`, `B(f) = (1 − w) · B_b + w · B_{b+1}`.
//!
//! A noise of density `S` gives `P̄_b = S · B_b` in every band, so `D = S` exactly, through
//! the crossovers as well as at the anchors: the same reading as a fixed spectrum's
//! density. A sine reads `a² / B(f)`, its level less `10·lg B(f)`.
//!
//! # Time
//! Each band is a [`SpectrumAnalyzer`] with its own hop and averaging, so a column updates
//! when a band behind it produces a frame: the low columns at the long window's hop, the high
//! ones at the short window's. A column is never extrapolated: before every band behind it has
//! a frame it has no value.

use crate::grid::LogGrid;
use crate::mtw::layout::{LADDER_DESIGN_PPO, validity_factor};
use crate::spectrum::{Averaging, SpectrumAnalyzer, SpectrumConfig, SpectrumError, SpectrumLevel};
use crate::window::Window;

/// Widest bin of the longest window, Hz: that of a 65 536-point FFT at 96 kHz.
pub const LF_BIN_HZ: f64 = 96_000.0 / 65_536.0;
/// Number of bands; each window is half the one before.
pub const BANDS: usize = 6;
/// Lowest column of the output grid, in octaves from 1 kHz (2⁻⁷ kHz ≈ 7.8 Hz).
pub const GRID_LOWEST_OCTAVE: i32 = -7;

/// One band of the ladder.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MultiResBand {
    /// Window (FFT) length, samples.
    pub n: usize,
    /// Hop between frames, samples.
    pub hop: usize,
    /// Bin spacing `fs / n`, Hz.
    pub bin_hz: f64,
    /// Where this band is the only estimate: `2 · κ(design) · bin_hz`, Hz.
    pub anchor_hz: f64,
}

impl MultiResBand {
    /// Window length, s.
    pub fn window_s(&self, fs: f64) -> f64 {
        self.n as f64 / fs
    }

    /// Hop, s.
    pub fn hop_s(&self, fs: f64) -> f64 {
        self.hop as f64 / fs
    }
}

/// The bands for a sample rate.
#[derive(Debug, Clone, PartialEq)]
pub struct MultiResLadder {
    /// Sample rate, Hz.
    pub fs: f64,
    /// Bands, longest window first.
    pub bands: Vec<MultiResBand>,
}

/// A band's share of a column.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandWeight {
    /// Band index.
    pub band: usize,
    /// Weight in the column's power (the weights of a column sum to 1).
    pub weight: f64,
}

impl MultiResLadder {
    /// The ladder at `fs`, each band framed every `hop(n)` samples.
    pub fn new(fs: f64, hop: impl Fn(usize) -> usize) -> Result<Self, SpectrumError> {
        if !(fs.is_finite() && fs > 0.0) {
            return Err(SpectrumError::SampleRate(fs));
        }
        let n_max = Self::longest_window(fs);
        let kappa = validity_factor(LADDER_DESIGN_PPO);
        let bands = (0..BANDS)
            .map(|b| {
                let n = n_max >> b;
                let bin_hz = fs / n as f64;
                MultiResBand {
                    n,
                    hop: hop(n),
                    bin_hz,
                    anchor_hz: 2.0 * kappa * bin_hz,
                }
            })
            .collect::<Vec<_>>();
        if let Some(b) = bands.iter().find(|b| b.hop == 0 || b.n < 2) {
            return Err(if b.hop == 0 {
                SpectrumError::Hop(b.hop)
            } else {
                SpectrumError::FftLength(b.n)
            });
        }
        Ok(Self { fs, bands })
    }

    /// The longest window at `fs`: the shortest power of two whose bin is at most
    /// [`LF_BIN_HZ`] wide (at least `2^BANDS`, so the shortest window has 2 samples).
    pub fn longest_window(fs: f64) -> usize {
        let need = fs / LF_BIN_HZ * (1.0 - 1e-12);
        (need.ceil().max(1.0) as usize)
            .next_power_of_two()
            .max(1 << BANDS)
    }

    /// The output grid at `ppo` columns per octave: from 2⁻⁷ kHz up to the last column
    /// whose upper edge is at or below Nyquist.
    pub fn grid(fs: f64, ppo: u32) -> LogGrid {
        let p = f64::from(ppo.max(1));
        let k_max = (p * (fs / 2.0 / 1000.0).log2() - 0.5).floor() as i32;
        LogGrid {
            ppo: ppo.max(1),
            k_min: GRID_LOWEST_OCTAVE * ppo.max(1) as i32,
            k_max,
        }
    }

    /// The bands behind a column centred at `f` and their weights: one band below the first
    /// anchor and above the last, two between anchors (shorter band weight `f / A_b − 1`).
    pub fn weights(&self, f: f64) -> [Option<BandWeight>; 2] {
        let last = self.bands.len() - 1;
        let one = |band| [Some(BandWeight { band, weight: 1.0 }), None];
        if f <= self.bands[0].anchor_hz {
            return one(0);
        }
        if f >= self.bands[last].anchor_hz {
            return one(last);
        }
        let b = self
            .bands
            .iter()
            .rposition(|band| band.anchor_hz < f)
            .unwrap_or(0);
        let w = (f / self.bands[b].anchor_hz - 1.0).clamp(0.0, 1.0);
        [
            Some(BandWeight {
                band: b,
                weight: 1.0 - w,
            }),
            Some(BandWeight {
                band: b + 1,
                weight: w,
            }),
        ]
    }

    /// The longest window that contributes to a column at `f`: what sets how fast the column
    /// follows a change.
    pub fn slowest_band(&self, f: f64) -> usize {
        self.weights(f)
            .iter()
            .flatten()
            .filter(|w| w.weight > 0.0)
            .map(|w| w.band)
            .min()
            .unwrap_or(0)
    }

    /// Resolution cell of a column at `f`: the bin of the widest-binned band contributing.
    pub fn cell_hz(&self, f: f64) -> f64 {
        let band = self
            .weights(f)
            .iter()
            .flatten()
            .filter(|w| w.weight > 0.0)
            .map(|w| w.band)
            .max()
            .unwrap_or(0);
        self.bands[band].bin_hz
    }

    /// Ranges of `grid` whose column is narrower than the cell behind it
    /// ([`crate::resolution::unresolved_ranges`]): below half the first anchor at the design
    /// density, and more on a finer grid.
    pub fn unresolved_ranges(&self, grid: &LogGrid) -> Vec<(f64, f64)> {
        crate::resolution::unresolved_ranges(grid, |f| Some(self.cell_hz(f)))
    }

    /// Noise bandwidth of a column at `f` for a window of `enbw_bins`, Hz: the weighted
    /// bandwidth of the bands behind it. A tone-level reading of a noise of density `S` is
    /// `S · B`.
    pub fn noise_bandwidth_hz(&self, f: f64, enbw_bins: f64) -> f64 {
        self.weights(f)
            .iter()
            .flatten()
            .map(|w| w.weight * enbw_bins * self.bands[w.band].bin_hz)
            .sum()
    }
}

/// Analyzer configuration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MultiResConfig {
    /// Window of every band.
    pub window: Window,
    /// Averaging of every band, each on its own frames.
    pub averaging: Averaging,
    /// Output grid.
    pub grid: LogGrid,
    /// Tone level or density per column.
    pub level: SpectrumLevel,
}

/// The bins one band contributes to one column.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Tap {
    band: usize,
    /// Bins `first .. end` of the band.
    first: usize,
    end: usize,
    /// The band's weight in the column (over the column's noise bandwidth, density).
    weight: f64,
}

/// Streaming multi-resolution analyzer. Pushing does not allocate.
#[derive(Debug, Clone)]
pub struct MultiResAnalyzer {
    ladder: MultiResLadder,
    cfg: MultiResConfig,
    bands: Vec<SpectrumAnalyzer>,
    /// Column `c` reads `taps[tap_first[c] .. tap_first[c + 1]]`.
    taps: Vec<Tap>,
    tap_first: Vec<usize>,
    /// Per band: folded power → tone power in the caller's scale, per bin.
    gain: Vec<Vec<f64>>,
}

impl MultiResAnalyzer {
    /// Creates an analyzer for `ladder`. All buffers are allocated here.
    pub fn new(ladder: MultiResLadder, cfg: MultiResConfig) -> Result<Self, SpectrumError> {
        let bands = ladder
            .bands
            .iter()
            .map(|b| {
                SpectrumAnalyzer::new(SpectrumConfig {
                    fs: ladder.fs,
                    n: b.n,
                    hop: b.hop,
                    window: cfg.window,
                    averaging: cfg.averaging,
                    peak_hold: None,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut taps = Vec::new();
        let mut tap_first = Vec::with_capacity(cfg.grid.len() + 1);
        let enbw = bands
            .first()
            .map_or(1.0, |a: &SpectrumAnalyzer| a.gains().enbw_bins());
        for c in 0..cfg.grid.len() {
            tap_first.push(taps.len());
            let f = cfg.grid.frequency(c);
            let (lo, hi) = cfg.grid.edges(c);
            let per = match cfg.level {
                SpectrumLevel::Tone => 1.0,
                SpectrumLevel::Density => 1.0 / ladder.noise_bandwidth_hz(f, enbw),
            };
            for w in ladder.weights(f).into_iter().flatten() {
                let band = &ladder.bands[w.band];
                let bins = band.n / 2 + 1;
                let first = ((lo / band.bin_hz).ceil().max(0.0) as usize).min(bins);
                let end = (((hi / band.bin_hz).ceil().max(0.0)) as usize).min(bins);
                let (first, end) = if end > first {
                    (first, end)
                } else {
                    // The column is narrower than a bin: it reads the bin it lies in.
                    let k = ((f / band.bin_hz).round() as usize).min(bins - 1);
                    (k, k + 1)
                };
                taps.push(Tap {
                    band: w.band,
                    first,
                    end,
                    weight: w.weight * per,
                });
            }
        }
        tap_first.push(taps.len());
        let gain = bands
            .iter()
            .map(|a| vec![a.tone_power_factor(); a.bins()])
            .collect();
        Ok(Self {
            ladder,
            cfg,
            bands,
            taps,
            tap_first,
            gain,
        })
    }

    /// The ladder.
    pub fn ladder(&self) -> &MultiResLadder {
        &self.ladder
    }

    /// Configuration.
    pub fn config(&self) -> &MultiResConfig {
        &self.cfg
    }

    /// Number of output columns.
    pub fn columns(&self) -> usize {
        self.cfg.grid.len()
    }

    /// Scales every bin by `gain(f)` on top of the tone scaling (a calibration and a mic
    /// curve correction, as a power factor at the bin's frequency).
    pub fn set_gain(&mut self, gain: impl Fn(f64) -> f64) {
        for ((g, a), band) in self
            .gain
            .iter_mut()
            .zip(&self.bands)
            .zip(&self.ladder.bands)
        {
            let tone = a.tone_power_factor();
            for (k, v) in g.iter_mut().enumerate() {
                *v = tone * gain(k as f64 * band.bin_hz);
            }
        }
    }

    /// Pushes samples into every band; returns a mask of the bands that analysed at least one
    /// frame during this call (bit `b` = band `b`).
    pub fn push(&mut self, block: &[f64]) -> u32 {
        let mut mask = 0;
        for (b, a) in self.bands.iter_mut().enumerate() {
            if a.push(block) > 0 {
                mask |= 1 << b;
            }
        }
        mask
    }

    /// Samples pushed after the end of band `b`'s newest frame; `None` before its first.
    pub fn samples_since_frame(&self, b: usize) -> Option<u64> {
        self.bands[b].samples_since_frame()
    }

    /// Samples pushed after the end of the newest frame of any band.
    pub fn samples_since_newest(&self) -> Option<u64> {
        self.bands
            .iter()
            .filter_map(SpectrumAnalyzer::samples_since_frame)
            .min()
    }

    /// Frames band `b` has averaged since the last reset.
    pub fn frames(&self, b: usize) -> u64 {
        self.bands[b].frames()
    }

    /// Restarts every band's averaging; the framing goes on.
    pub fn reset_average(&mut self) {
        for a in &mut self.bands {
            a.reset_average();
        }
    }

    /// Tone power or density per column (in the scale [`Self::set_gain`] set) from the
    /// averaged spectra; NaN where a band behind the column has no frame yet. Returns
    /// `false` when no band has a frame.
    ///
    /// # Panics
    /// If `out.len()` is not [`Self::columns`].
    pub fn column_power(&self, out: &mut [f64]) -> bool {
        assert_eq!(out.len(), self.columns(), "one value per column");
        if self.bands.iter().all(|a| a.average().is_none()) {
            return false;
        }
        for (c, o) in out.iter_mut().enumerate() {
            let mut p = 0.0;
            for t in &self.taps[self.tap_first[c]..self.tap_first[c + 1]] {
                let Some(s) = self.bands[t.band].average() else {
                    p = f64::NAN;
                    break;
                };
                let g = &self.gain[t.band];
                let bins = s.folded()[t.first..t.end]
                    .iter()
                    .zip(&g[t.first..t.end])
                    .map(|(f, g)| f * g);
                let v = match self.cfg.level {
                    SpectrumLevel::Tone => bins.fold(f64::NEG_INFINITY, f64::max),
                    SpectrumLevel::Density => bins.sum::<f64>() / (t.end - t.first) as f64,
                };
                p += t.weight * v;
            }
            *o = p;
        }
        true
    }
}

#[cfg(test)]
#[path = "multires_tests.rs"]
mod tests;
