//! Stage layout: decimation factors, FFT framing, crossovers and decimator designs.
//!
//! Crossovers sit at the shallower stage's *validity edge* for the ladder's design density:
//! the lowest frequency where one bin of that stage fills one 1/48-octave column,
//! `f = κ · Δf` with `κ = 1 / (2^(1/96) − 2^(−1/96))`. Below it the shallower stage cannot
//! fill every column, so the deeper stage serves. The 1/3-octave blend is placed *above*
//! the edge so no column is ever drawn from a stage that cannot resolve it. The design
//! density is a ladder constant, deliberately separate from whatever grid the frame is
//! drawn on: changing the display grid must not move the crossovers.

use super::fir::FirDesign;

/// FFT length of every ladder stage.
pub const NFFT: usize = 4096;
/// Target rates of the decimated stages; the factor is `round(sr / target)`.
pub const DECIMATED_TARGET_RATES_HZ: [f64; 2] = [12_000.0, 4_000.0];
/// Density (points per base-2 octave) the crossovers are designed for.
pub const LADDER_DESIGN_PPO: u32 = 48;
/// A stage serves frequencies up to this fraction of its own sample rate at most.
pub const SERVED_BAND_FRACTION: f64 = 0.45;
/// Width of the complex crossover blend, in octaves.
pub const BLEND_OCTAVES: f64 = 1.0 / 3.0;
/// Decimator design attenuation (dB). Kaiser's length formula can land a dB short, so the
/// design asks for 92 dB to guarantee at least 90 dB.
pub const DECIMATOR_ATTENUATION_DB: f64 = 92.0;

/// `κ(ppo)`: a column at `f` is at least one bin wide where `f ≥ κ · Δf`.
pub fn validity_factor(ppo: u32) -> f64 {
    let h = 0.5 / f64::from(ppo);
    1.0 / (2f64.powf(h) - 2f64.powf(-h))
}

/// Which ladder to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ladder {
    /// Full rate plus the ~12 kHz and ~4 kHz stages at NFFT 4096, overlap 50/75/87.5 %.
    Standard,
    /// One full-rate stage with the given FFT length and hop: fixed-FFT analysis, and a
    /// configuration that is directly comparable with a plain Welch estimate.
    Single {
        /// FFT length (even, ≥ 16).
        nfft: usize,
        /// Hop between block starts (1 ≤ hop ≤ nfft).
        hop: usize,
    },
}

/// Layout errors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LayoutError {
    /// Sample rate not positive and finite.
    InvalidSampleRate(f64),
    /// Single-stage FFT length or hop out of range.
    InvalidFraming {
        /// Requested FFT length.
        nfft: usize,
        /// Requested hop.
        hop: usize,
    },
    /// A stage would have to serve above `SERVED_BAND_FRACTION` of its rate. The three-stage
    /// ladder covers rates up to about 260 kHz; higher rates need an intermediate stage.
    StageOverReach {
        /// Stage index.
        stage: usize,
        /// Highest frequency the stage would have to serve.
        served_hz: f64,
        /// The stage's sample rate.
        rate_hz: f64,
    },
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutError::InvalidSampleRate(sr) => write!(f, "invalid sample rate {sr}"),
            LayoutError::InvalidFraming { nfft, hop } => {
                write!(f, "invalid framing nfft={nfft} hop={hop}")
            }
            LayoutError::StageOverReach {
                stage,
                served_hz,
                rate_hz,
            } => write!(
                f,
                "stage {stage} would serve {served_hz:.1} Hz at {rate_hz:.1} Hz \
                 (limit {SERVED_BAND_FRACTION} of rate)"
            ),
        }
    }
}

impl std::error::Error for LayoutError {}

/// One analysis stage.
#[derive(Debug, Clone, PartialEq)]
pub struct StageSpec {
    /// Position in the ladder, 0 = full rate.
    pub index: usize,
    /// Decimation factor from the full rate.
    pub factor: usize,
    /// Stage sample rate.
    pub rate_hz: f64,
    /// FFT length.
    pub nfft: usize,
    /// Hop between block starts, in stage samples.
    pub hop: usize,
    /// Bin spacing `rate / nfft`.
    pub bin_hz: f64,
    /// Highest frequency any column takes from this stage.
    pub served_hi_hz: f64,
    /// Anti-alias decimator, absent at full rate.
    pub decimator: Option<FirDesign>,
}

impl StageSpec {
    /// Fractional overlap of consecutive blocks.
    pub fn overlap(&self) -> f64 {
        1.0 - self.hop as f64 / self.nfft as f64
    }

    /// Window length in seconds.
    pub fn window_s(&self) -> f64 {
        self.nfft as f64 / self.rate_hz
    }

    /// Hop in seconds.
    pub fn hop_s(&self) -> f64 {
        self.hop as f64 / self.rate_hz
    }

    /// Lowest frequency at which this stage fills every column at the design density.
    pub fn validity_edge_hz(&self) -> f64 {
        validity_factor(LADDER_DESIGN_PPO) * self.bin_hz
    }

    /// Decimator length (1 at full rate).
    pub fn filter_len(&self) -> usize {
        self.decimator.as_ref().map_or(1, FirDesign::len)
    }

    /// Aligned full-rate pairs needed before this stage has completed `blocks` blocks
    /// (≥ 1). Decimated sample `j` depends on pairs `[j·M, j·M + L)`, and block `k` ends at
    /// stage sample `k·hop + nfft − 1`.
    pub fn pairs_for_blocks(&self, blocks: usize) -> u64 {
        let last = (self.nfft + blocks.saturating_sub(1) * self.hop - 1) as u64;
        last * self.factor as u64 + self.filter_len() as u64
    }

    /// Time from the start of the aligned stream to this stage's first complete block:
    /// the window plus the decimator's span. No averaging setting shortens it.
    pub fn first_block_s(&self) -> f64 {
        self.pairs_for_blocks(1) as f64 / (self.rate_hz * self.factor as f64)
    }
}

/// Hand-over between a shallower and the next deeper stage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crossover {
    /// Higher-rate stage.
    pub shallow: usize,
    /// Lower-rate stage.
    pub deep: usize,
    /// Blend start: the shallow stage's validity edge. Below it only `deep` serves.
    pub lo_hz: f64,
    /// Blend end (`lo · 2^(1/3)`). Above it only `shallow` serves.
    pub hi_hz: f64,
}

impl Crossover {
    /// Weight of the shallow stage at `f` (0 below the blend, 1 above), a raised cosine in
    /// log frequency.
    pub fn shallow_weight(&self, f: f64) -> f64 {
        if f <= self.lo_hz {
            0.0
        } else if f >= self.hi_hz {
            1.0
        } else {
            let t = (f / self.lo_hz).ln() / (self.hi_hz / self.lo_hz).ln();
            0.5 - 0.5 * (std::f64::consts::PI * t).cos()
        }
    }
}

/// Complete stage layout for a sample rate.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// Full sample rate.
    pub sample_rate_hz: f64,
    /// Stages, shallowest (full rate) first.
    pub stages: Vec<StageSpec>,
    /// `crossovers[b]` hands over between stage `b` and `b + 1`.
    pub crossovers: Vec<Crossover>,
}

impl Layout {
    /// Build the layout for `ladder` at `sample_rate_hz`.
    pub fn new(sample_rate_hz: f64, ladder: Ladder) -> Result<Self, LayoutError> {
        if !(sample_rate_hz.is_finite() && sample_rate_hz > 0.0) {
            return Err(LayoutError::InvalidSampleRate(sample_rate_hz));
        }
        match ladder {
            Ladder::Single { nfft, hop } => {
                if nfft < 16 || nfft % 2 != 0 || hop == 0 || hop > nfft {
                    return Err(LayoutError::InvalidFraming { nfft, hop });
                }
                Ok(Self {
                    sample_rate_hz,
                    stages: vec![StageSpec {
                        index: 0,
                        factor: 1,
                        rate_hz: sample_rate_hz,
                        nfft,
                        hop,
                        bin_hz: sample_rate_hz / nfft as f64,
                        served_hi_hz: SERVED_BAND_FRACTION * sample_rate_hz,
                        decimator: None,
                    }],
                    crossovers: Vec::new(),
                })
            }
            Ladder::Standard => Self::standard(sample_rate_hz),
        }
    }

    /// Resolution cell of a column centred at `f`: the bin spacing of the coarsest stage
    /// serving it (in a crossover blend, the shallow one); `None` above the served band.
    pub fn cell_hz(&self, f: f64) -> Option<f64> {
        if f > self.stages.first()?.served_hi_hz {
            return None;
        }
        let serving = self
            .crossovers
            .iter()
            .find(|c| f > c.lo_hz)
            .map_or(self.stages.len() - 1, |c| c.shallow);
        Some(self.stages[serving].bin_hz)
    }

    /// Ranges of `grid` where a column is narrower than the bin of the stage serving it
    /// ([`crate::resolution::unresolved_ranges`]). On a grid at the design density only the
    /// band below the deepest stage's validity edge; on a finer one also the band from each
    /// crossover up to `κ(ppo) / κ(design)` times it, where the shallower stage serves
    /// columns narrower than its bins.
    pub fn unresolved_ranges(&self, grid: &crate::grid::LogGrid) -> Vec<(f64, f64)> {
        crate::resolution::unresolved_ranges(grid, |f| self.cell_hz(f))
    }

    fn standard(sr: f64) -> Result<Self, LayoutError> {
        let mut factors = vec![1usize];
        for target in DECIMATED_TARGET_RATES_HZ {
            let m = (sr / target).round().max(1.0) as usize;
            if m > *factors.last().unwrap_or(&1) {
                factors.push(m);
            }
        }
        let kappa = validity_factor(LADDER_DESIGN_PPO);
        let half_col = 2f64.powf(0.5 / f64::from(LADDER_DESIGN_PPO));
        let blend = 2f64.powf(BLEND_OCTAVES);
        let mut stages = Vec::with_capacity(factors.len());
        let mut crossovers = Vec::new();
        for (b, &m) in factors.iter().enumerate() {
            let rate = sr / m as f64;
            // Overlap deepens 50 / 75 / 87.5 %: deeper stages have longer windows, and the
            // extra overlap keeps their update interval from growing with the window.
            let hop = NFFT >> (b + 1);
            let bin_hz = rate / NFFT as f64;
            let served_hi_hz = if b == 0 {
                SERVED_BAND_FRACTION * rate
            } else {
                let prev: &StageSpec = &stages[b - 1];
                let lo = kappa * prev.bin_hz;
                crossovers.push(Crossover {
                    shallow: b - 1,
                    deep: b,
                    lo_hz: lo,
                    hi_hz: lo * blend,
                });
                // The top blend column's upper edge.
                lo * blend * half_col
            };
            if served_hi_hz > SERVED_BAND_FRACTION * rate + 1e-9 {
                return Err(LayoutError::StageOverReach {
                    stage: b,
                    served_hz: served_hi_hz,
                    rate_hz: rate,
                });
            }
            let decimator = (m > 1).then(|| {
                // Stopband from `rate − guard`: whatever folds back lands at or above
                // `guard`, outside the served band with room for the analysis window's
                // main lobe. Aliases above the served band are harmless; that band is
                // served by a shallower stage.
                let guard = (1.25 * served_hi_hz).min(0.5 * rate);
                FirDesign::kaiser_lowpass(sr, served_hi_hz, rate - guard, DECIMATOR_ATTENUATION_DB)
            });
            stages.push(StageSpec {
                index: b,
                factor: m,
                rate_hz: rate,
                nfft: NFFT,
                hop,
                bin_hz,
                served_hi_hz,
                decimator,
            });
        }
        Ok(Self {
            sample_rate_hz: sr,
            stages,
            crossovers,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kappa_48() {
        assert!((validity_factor(48) - 69.2488).abs() < 1e-3);
    }

    #[test]
    fn factors_at_common_rates() {
        for (sr, want) in [
            (44_100.0, vec![1, 4, 11]),
            (48_000.0, vec![1, 4, 12]),
            (96_000.0, vec![1, 8, 24]),
            (192_000.0, vec![1, 16, 48]),
        ] {
            let l = Layout::new(sr, Ladder::Standard).expect("layout");
            let got: Vec<usize> = l.stages.iter().map(|s| s.factor).collect();
            assert_eq!(got, want, "{sr}");
            for s in &l.stages {
                assert!(s.served_hi_hz <= SERVED_BAND_FRACTION * s.rate_hz);
            }
            // The blend sits above the shallow stage's validity edge.
            for c in &l.crossovers {
                assert!(c.lo_hz >= l.stages[c.shallow].validity_edge_hz() - 1e-9);
            }
        }
    }

    #[test]
    fn crossovers_at_48k() {
        let l = Layout::new(48_000.0, Ladder::Standard).expect("layout");
        assert!((l.crossovers[0].lo_hz - 811.51).abs() < 0.01);
        assert!((l.crossovers[1].lo_hz - 202.88).abs() < 0.01);
        assert_eq!(
            l.stages.iter().map(|s| s.hop).collect::<Vec<_>>(),
            vec![2048, 1024, 512]
        );
    }

    fn ten_octaves(ppo: u32) -> crate::grid::LogGrid {
        let p = ppo as i32;
        crate::grid::LogGrid {
            ppo,
            k_min: -5 * p,
            k_max: 5 * p - 1,
        }
    }

    /// `want` within one column (`2^(1/ppo)`) of `got`.
    fn near(got: f64, want: f64, ppo: u32) -> bool {
        (got / want).log2().abs() <= 1.0 / f64::from(ppo)
    }

    #[test]
    fn unresolved_at_96k() {
        // Bins at 96 kHz: 96000/4096 = 23.44 Hz, /8 → 2.930 Hz, /24 → 0.9766 Hz. Crossovers
        // at κ(48)·bin: 1623 Hz and 202.9 Hz; the deepest stage resolves 1/48 from
        // κ(48)·0.9766 = 67.6 Hz and 1/96 from κ(96)·0.9766 = 135.3 Hz.
        let l = Layout::new(96_000.0, Ladder::Standard).expect("layout");
        let bins: Vec<f64> = l.stages.iter().map(|s| s.bin_hz).collect();
        let r48 = l.unresolved_ranges(&ten_octaves(48));
        assert_eq!(r48.len(), 1, "{r48:?}");
        assert!(r48[0].0 < 32.0);
        assert!(near(r48[0].1, 67.6, 48), "{r48:?}");
        assert!(near(r48[0].1, validity_factor(48) * bins[2], 48));

        let r96 = l.unresolved_ranges(&ten_octaves(96));
        assert_eq!(r96.len(), 3, "{r96:?}");
        let k96 = validity_factor(96);
        assert!(near(r96[0].1, k96 * bins[2], 96), "{r96:?}");
        assert!((k96 * bins[2] - 135.25).abs() < 0.05);
        // Just above each crossover the shallower stage's bins are wider than 1/96.
        assert!(near(r96[1].0, l.crossovers[1].lo_hz, 96), "{r96:?}");
        assert!(near(r96[1].1, k96 * bins[1], 96), "{r96:?}");
        assert!(near(r96[2].0, l.crossovers[0].lo_hz, 96), "{r96:?}");
        assert!(near(r96[2].1, k96 * bins[0], 96), "{r96:?}");
        assert!((k96 * bins[0] - 3246.0).abs() < 1.0);

        // Coarser grids: only the bottom, and at 1/12 nothing within ten octaves.
        let r24 = l.unresolved_ranges(&ten_octaves(24));
        assert_eq!(r24.len(), 1);
        assert!(near(r24[0].1, validity_factor(24) * bins[2], 24), "{r24:?}");
        assert!(l.unresolved_ranges(&ten_octaves(12)).is_empty());
        // Above the served band nothing is estimated, so nothing is marked.
        assert!(l.cell_hz(0.46 * 96_000.0).is_none());
    }

    #[test]
    fn unresolved_at_48k_starts_at_the_deepest_stage() {
        // 48 kHz: deepest stage 4 kHz, bin 0.9766 Hz, the same LF edge as at 96 kHz; the
        // full-rate stage's crossover moves down to 811.5 Hz.
        let l = Layout::new(48_000.0, Ladder::Standard).expect("layout");
        let r96 = l.unresolved_ranges(&ten_octaves(96));
        assert_eq!(r96.len(), 3, "{r96:?}");
        assert!(
            near(r96[2].0, 811.5, 96) && near(r96[2].1, 1623.0, 96),
            "{r96:?}"
        );
    }

    #[test]
    fn very_high_rate_is_refused_not_silently_wrong() {
        assert!(matches!(
            Layout::new(384_000.0, Ladder::Standard),
            Err(LayoutError::StageOverReach { stage: 1, .. })
        ));
    }

    #[test]
    fn low_rate_drops_duplicate_stages() {
        let l = Layout::new(8_000.0, Ladder::Standard).expect("layout");
        let got: Vec<usize> = l.stages.iter().map(|s| s.factor).collect();
        assert_eq!(got, vec![1, 2]);
    }
}
