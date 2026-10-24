//! Fixed-memory gating for integrated loudness (BS.1770-5) and loudness range (Tech 3342).
//!
//! A show runs for hours, so neither gate may keep every block. Each keeps 0.01 LU bins from
//! −70 to +10 LUFS with each bin's count and exact f64 energy sum, plus an underflow bin
//! (what the absolute gate drops) and an overflow bin (f32 input can exceed 0 dBFS). The
//! relative threshold comes from the exact energy sums; only the blocks in the bin the
//! threshold falls in are approximated, and they lie within 0.01 LU of a gate where the exact
//! result itself jumps for a 0.01 LU change of the input.

use super::lufs;

/// Lower edge of the binned range, LUFS: the absolute gate of both measures.
pub const HISTOGRAM_FLOOR: f64 = -70.0;
/// Bin width, LU.
pub const BIN_LU: f64 = 0.01;
/// Bins between −70 and +10 LUFS.
pub const BINS: usize = 8000;

const INTEGRATED_REL_LU: f64 = -10.0;
const RANGE_REL_LU: f64 = -20.0;
const PERCENTILE_LOW: u64 = 10;
const PERCENTILE_HIGH: u64 = 95;

#[derive(Debug, Clone, Copy, Default)]
struct Bin {
    count: u64,
    /// Sum of the blocks' channel-weighted mean squares.
    energy: f64,
}

impl Bin {
    fn add(&mut self, z: f64) {
        self.count += 1;
        self.energy += z;
    }
}

/// Which side of −70 LUFS the absolute gate keeps: BS.1770 keeps `l > −70`, Tech 3342
/// keeps `l >= −70`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gate {
    Above,
    AtOrAbove,
}

#[derive(Debug, Clone)]
struct Histogram {
    gate: Gate,
    bins: Box<[Bin]>,
    under: Bin,
    over: Bin,
}

impl Histogram {
    fn new(gate: Gate) -> Self {
        Self {
            gate,
            bins: vec![Bin::default(); BINS].into_boxed_slice(),
            under: Bin::default(),
            over: Bin::default(),
        }
    }

    fn clear(&mut self) {
        self.bins.fill(Bin::default());
        self.under = Bin::default();
        self.over = Bin::default();
    }

    /// Bin index of a loudness inside the binned range.
    fn index(l: f64) -> usize {
        ((l - HISTOGRAM_FLOOR) * 100.0).floor() as usize
    }

    fn lower_edge(i: usize) -> f64 {
        HISTOGRAM_FLOOR + BIN_LU * i as f64
    }

    /// Adds a block of mean square `z` (loudness `l = −0.691 + 10 log₁₀ z`).
    fn add(&mut self, z: f64) {
        if !z.is_finite() {
            return;
        }
        let l = lufs(z);
        let dropped = match self.gate {
            Gate::Above => l <= HISTOGRAM_FLOOR,
            Gate::AtOrAbove => l < HISTOGRAM_FLOOR,
        };
        if dropped {
            self.under.add(z);
            return;
        }
        let i = Self::index(l);
        if i >= BINS {
            self.over.add(z);
        } else {
            self.bins[i].add(z);
        }
    }

    /// Count and energy of every block the absolute gate keeps.
    fn absolute(&self) -> (u64, f64) {
        self.bins
            .iter()
            .chain(std::iter::once(&self.over))
            .fold((0, 0.0), |(c, e), b| (c + b.count, e + b.energy))
    }

    /// Relative threshold `offset` LU under the energy mean of the absolute-gated blocks;
    /// `None` when none passed.
    fn threshold(&self, offset: f64) -> Option<f64> {
        let (c, e) = self.absolute();
        (c > 0).then(|| lufs(e / c as f64) + offset)
    }
}

/// Integrated loudness from 400 ms gating blocks (BS.1770-5 Annex 1, both gates `>`).
#[derive(Debug, Clone)]
pub(super) struct IntegratedGate(Histogram);

impl IntegratedGate {
    pub(super) fn new() -> Self {
        Self(Histogram::new(Gate::Above))
    }

    pub(super) fn clear(&mut self) {
        self.0.clear();
    }

    pub(super) fn add(&mut self, z: f64) {
        self.0.add(z);
    }

    /// Integrated loudness, `None` until a block passes the absolute gate.
    pub(super) fn value(&self) -> Option<f64> {
        let h = &self.0;
        let gamma = h.threshold(INTEGRATED_REL_LU)?;
        let (count, energy) = if gamma <= HISTOGRAM_FLOOR {
            // The absolute gate governs: every block it kept counts.
            let (c, e) = h.absolute();
            (c as f64, e)
        } else {
            let mut c = h.over.count as f64;
            let mut e = h.over.energy;
            let g = Histogram::index(gamma);
            if g < BINS {
                // Blocks strictly above Γ count; in Γ's bin, the share above Γ assuming the
                // blocks spread evenly over the bin.
                let frac = (Histogram::lower_edge(g + 1) - gamma) / BIN_LU;
                c += h.bins[g].count as f64 * frac;
                e += h.bins[g].energy * frac;
                for b in &h.bins[g + 1..] {
                    c += b.count as f64;
                    e += b.energy;
                }
            }
            (c, e)
        };
        (count > 0.0).then(|| lufs(energy / count))
    }
}

/// Loudness range from the short-term values on the gating grid (Tech 3342, both gates
/// `>=`, percentiles 10 and 95).
#[derive(Debug, Clone)]
pub(super) struct RangeGate(Histogram);

impl RangeGate {
    pub(super) fn new() -> Self {
        Self(Histogram::new(Gate::AtOrAbove))
    }

    pub(super) fn clear(&mut self) {
        self.0.clear();
    }

    pub(super) fn add(&mut self, z: f64) {
        self.0.add(z);
    }

    /// LRA in LU, `None` until a value passes the absolute gate.
    pub(super) fn value(&self) -> Option<f64> {
        let h = &self.0;
        let gamma = h.threshold(RANGE_REL_LU)?;
        // First regular bin that counts (whole bins only, so the gated count is an integer);
        // BINS when only the overflow bin does.
        let first = if gamma < HISTOGRAM_FLOOR {
            0
        } else {
            let g = Histogram::index(gamma);
            if g >= BINS {
                BINS
            } else if Histogram::lower_edge(g) >= gamma {
                g
            } else {
                g + 1
            }
        };
        let n = h.bins[first..].iter().map(|b| b.count).sum::<u64>() + h.over.count;
        if n == 0 {
            return None;
        }
        let low = self.percentile(first, n, PERCENTILE_LOW);
        let high = self.percentile(first, n, PERCENTILE_HIGH);
        Some(high - low)
    }

    /// The gated value at zero-based rank `((n − 1)·p + 50) / 100` (Tech 3342's MATLAB
    /// `round((n − 1)·p/100 + 1)`, one-based): the centre of the bin holding it, or the
    /// overflow bin's energy mean.
    fn percentile(&self, first: usize, n: u64, p: u64) -> f64 {
        let h = &self.0;
        let k = ((n - 1) * p + 50) / 100;
        let mut cum = 0u64;
        for (i, b) in h.bins.iter().enumerate().skip(first) {
            cum += b.count;
            if cum > k {
                return Histogram::lower_edge(i) + 0.5 * BIN_LU;
            }
        }
        lufs(h.over.energy / h.over.count as f64)
    }
}
