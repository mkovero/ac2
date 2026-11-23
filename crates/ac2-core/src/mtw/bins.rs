//! Each stage's own bins, as the averages hold them: for a display that draws the analysis
//! at its native resolution rather than gathered into grid columns.
//!
//! A stage's bins are drawn where that stage alone serves the grid: from the top of the
//! blend below it (crossover `b`'s `hi`, first bin `⌈hi / Δf⌉`) up to the start of the blend
//! above it (crossover `b − 1`'s `lo`, last bin `⌊lo / Δf⌋`); the full-rate stage up to its
//! served band, the deepest from bin 1 (DC carries no transfer). Inside a blend two stages'
//! estimates are mixed, which no single stage's bins show, so the grid is drawn there.

use std::ops::Range;

use num_complex::Complex64;

use super::{Layout, Mtw};

/// The bins of one stage drawn as its own, and what places them in time and frequency.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawnBins {
    /// Stage index, 0 = full rate.
    pub stage: usize,
    /// Decimation factor from the full rate.
    pub factor: usize,
    /// Stage sample rate, Hz.
    pub rate_hz: f64,
    /// Bin spacing, Hz: bin `k` is centred at `k · bin_hz`.
    pub bin_hz: f64,
    /// Hop between block starts in full-rate samples: how far apart the stage's results come.
    pub hop: u64,
    /// First drawn bin.
    pub first: usize,
    /// Drawn bins, `first ..= first + count − 1`.
    pub count: usize,
}

impl DrawnBins {
    /// The drawn bins as a range of bin indices.
    pub fn range(&self) -> Range<usize> {
        self.first..self.first + self.count
    }
}

impl Layout {
    /// Per stage, shallowest first, the bins drawn as its own (module doc).
    pub fn drawn_bins(&self) -> Vec<DrawnBins> {
        let deepest = self.stages.len().saturating_sub(1);
        self.stages
            .iter()
            .enumerate()
            .map(|(b, s)| {
                let first = if b == deepest {
                    1
                } else {
                    (self.crossovers[b].hi_hz / s.bin_hz).ceil() as usize
                };
                let top = if b == 0 {
                    s.served_hi_hz
                } else {
                    self.crossovers[b - 1].lo_hz
                };
                let last = (top / s.bin_hz).floor() as usize;
                DrawnBins {
                    stage: b,
                    factor: s.factor,
                    rate_hz: s.rate_hz,
                    bin_hz: s.bin_hz,
                    hop: (s.hop * s.factor) as u64,
                    first,
                    count: (last + 1).saturating_sub(first),
                }
            })
            .collect()
    }
}

impl Mtw {
    /// Blocks held in stage `stage`'s average (FIFO: held; exponential: received); 0 for a
    /// stage that does not exist.
    pub fn stage_held(&self, stage: usize) -> u64 {
        self.stages.get(stage).map_or(0, |s| s.est.held())
    }

    /// One past the newest measurement input index stage `stage`'s newest averaged block
    /// depends on; `None` before its first block (or after its averages were reset).
    pub fn stage_last_block_end(&self, stage: usize) -> Option<u64> {
        self.stage_block_end(self.stages.get(stage)?)
    }

    /// H1 = Gxy / Gxx and γ² = |Gxy|² / (Gxx·Gyy) (clamped to 1) of stage `stage`'s bins
    /// `bins`, into `h1` and `coh` (cleared first, one entry per bin). NaN in both where the
    /// reference has no energy; γ² is 0 where only the measurement has none. Every entry is
    /// NaN before the stage's first block. Returns the blocks the average holds.
    pub fn stage_bins_into(
        &self,
        stage: usize,
        bins: Range<usize>,
        h1: &mut Vec<Complex64>,
        coh: &mut Vec<f64>,
    ) -> u64 {
        h1.clear();
        coh.clear();
        let nan = f64::NAN;
        let n = bins.len();
        let Some(s) = self.stages.get(stage) else {
            h1.resize(n, Complex64::new(nan, nan));
            coh.resize(n, nan);
            return 0;
        };
        let (sums, weight) = s.est.sums();
        let held = s.est.held();
        let bins = bins.start.min(sums.xx.len())..bins.end.min(sums.xx.len());
        if held == 0 || weight <= 0.0 || bins.len() != n {
            h1.resize(n, Complex64::new(nan, nan));
            coh.resize(n, nan);
            return 0;
        }
        for k in bins {
            let (xx, yy, xy) = (sums.xx[k], sums.yy[k], sums.xy[k]);
            if xx > 0.0 && xx.is_finite() {
                h1.push(xy / xx);
                // As in a column: a bin without measurement energy holds no coherent power,
                // and rounding can push |Gxy|² a hair above Gxx·Gyy.
                coh.push(if yy > 0.0 {
                    (xy.norm_sqr() / (xx * yy)).min(1.0)
                } else {
                    0.0
                });
            } else {
                h1.push(Complex64::new(nan, nan));
                coh.push(nan);
            }
        }
        held
    }
}
