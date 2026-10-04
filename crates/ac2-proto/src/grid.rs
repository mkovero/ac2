//! Immutable column grids and their stable ids.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::model::BandFraction;
use crate::units::Hz;

/// Stable grid identifier: FNV-1a 64 of the grid's canonical bytes ([`GridDef::canonical_bytes`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GridId(pub u64);

impl fmt::Display for GridId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

/// A column grid. Frames reference grids by [`GridId`]; `grid.get` returns the definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum GridDef {
    /// Log-spaced columns `f_k = 1000 · 2^(k / ppo)` Hz for `k = k_min ..= k_max`.
    Log {
        /// Points per octave.
        ppo: u32,
        /// First index.
        k_min: i32,
        /// Last index (inclusive).
        k_max: i32,
    },
    /// IEC fractional-octave bands; columns are the exact mid-band frequencies listed.
    IecBands {
        /// Band fraction.
        fraction: BandFraction,
        /// Mid-band frequencies, ascending.
        centres: Vec<Hz>,
    },
    /// FFT bins `k · fs / n` for `k = 0 ..= n / 2`.
    Linear {
        /// Sample rate.
        fs: Hz,
        /// FFT length.
        n: u32,
    },
    /// The bins of an `n`-point FFT at `fs` gathered for display ([`BinColumns`]): one
    /// column per bin while a bin is at least `1/ppo` octave wide, then columns `1/ppo`
    /// octave wide, each holding the bins whose centres it covers.
    LogBins {
        /// Sample rate.
        fs: Hz,
        /// FFT length.
        n: u32,
        /// Columns per octave above the single-bin columns.
        ppo: u32,
    },
}

/// Columns of a [`GridDef::LogBins`] grid. Column `c` holds bins `first_bin[c] ..
/// first_bin[c + 1]` and spans `edges[c] .. edges[c + 1]` Hz; both have one entry more
/// than there are columns.
///
/// Bin `k` (centre `k · df`, `df = fs / n`) is a column of its own while `k <
/// ceil(1 / (2^(1/ppo) − 1))`: below that a `1/ppo`-octave column would be narrower than a
/// bin and could hold none. Above it the edges grow by `2^(1/ppo)` from the upper edge of
/// the last single bin; such a column is wider than a bin, so it holds at least one bin
/// centre. The last column ends half a bin above Nyquist.
#[derive(Debug, Clone, PartialEq)]
pub struct BinColumns {
    /// First bin of each column, then `n / 2 + 1`.
    pub first_bin: Vec<u32>,
    /// Lower edge of each column, then the upper edge of the last, Hz.
    pub edges: Vec<f64>,
    /// Centre frequency of each column, Hz: the bin's own frequency for a single-bin
    /// column, the geometric mean of the edges otherwise.
    pub centres: Vec<f64>,
}

impl BinColumns {
    /// Columns of an `n`-point FFT at `fs` at `ppo` columns per octave.
    pub fn new(fs: f64, n: u32, ppo: u32) -> Self {
        let mut first_bin = Vec::new();
        let mut edges = Vec::new();
        log_bins(fs, n, ppo, |bin, edge| {
            first_bin.push(bin);
            edges.push(edge);
        });
        let df = fs / f64::from(n.max(1));
        let centres = first_bin
            .windows(2)
            .zip(edges.windows(2))
            .map(|(b, e)| {
                if b[1] - b[0] == 1 {
                    f64::from(b[0]) * df
                } else {
                    (e[0] * e[1]).sqrt()
                }
            })
            .collect();
        Self {
            first_bin,
            edges,
            centres,
        }
    }

    /// Number of columns.
    pub fn len(&self) -> usize {
        self.centres.len()
    }

    /// True when there are no columns.
    pub fn is_empty(&self) -> bool {
        self.centres.is_empty()
    }
}

/// Walks the column boundaries of a [`GridDef::LogBins`] grid: `(first bin, lower edge Hz)`
/// of every column, then `(n / 2 + 1, upper edge of the last column)`.
fn log_bins(fs: f64, n: u32, ppo: u32, mut boundary: impl FnMut(u32, f64)) {
    if n == 0 || ppo == 0 || !(fs > 0.0 && fs.is_finite()) {
        boundary(0, 0.0);
        return;
    }
    let bins = n / 2 + 1;
    let df = fs / f64::from(n);
    let r = 2f64.powf(1.0 / f64::from(ppo));
    let single = ((1.0 / (r - 1.0)).ceil()).min(f64::from(bins)) as u32;
    for k in 0..single {
        boundary(k, (f64::from(k) - 0.5).max(0.0) * df);
    }
    let top = (f64::from(bins) - 0.5) * df;
    let mut bin = single;
    let mut edge = (f64::from(single) - 0.5).max(0.0) * df;
    while bin < bins {
        boundary(bin, edge);
        let mut next = edge * r;
        // Rounding could leave a column without a bin centre; it widens until it has one.
        let mut next_bin = (next / df).ceil() as u32;
        while next_bin <= bin {
            next *= r;
            next_bin = (next / df).ceil() as u32;
        }
        bin = next_bin.min(bins);
        edge = next;
    }
    boundary(bins, top);
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

impl GridDef {
    /// Number of columns.
    pub fn len(&self) -> usize {
        match self {
            Self::Log { k_min, k_max, .. } => {
                usize::try_from(i64::from(*k_max) - i64::from(*k_min) + 1).unwrap_or(0)
            }
            Self::IecBands { centres, .. } => centres.len(),
            Self::Linear { n, .. } => *n as usize / 2 + 1,
            Self::LogBins { fs, n, ppo } => {
                let mut boundaries = 0usize;
                log_bins(fs.0, *n, *ppo, |_, _| boundaries += 1);
                boundaries - 1
            }
        }
    }

    /// True when the grid has no columns.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Canonical byte encoding hashed into the id: a tag byte (1 log, 2 IEC bands,
    /// 3 linear, 4 log bins) followed by the fields little-endian; floats as IEEE-754 f64 bits.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut b = Vec::new();
        match self {
            Self::Log { ppo, k_min, k_max } => {
                b.push(1);
                b.extend_from_slice(&ppo.to_le_bytes());
                b.extend_from_slice(&k_min.to_le_bytes());
                b.extend_from_slice(&k_max.to_le_bytes());
            }
            Self::IecBands { fraction, centres } => {
                b.push(2);
                b.extend_from_slice(&fraction.b().to_le_bytes());
                let n = u32::try_from(centres.len()).unwrap_or(u32::MAX);
                b.extend_from_slice(&n.to_le_bytes());
                for c in centres {
                    b.extend_from_slice(&c.0.to_bits().to_le_bytes());
                }
            }
            Self::Linear { fs, n } => {
                b.push(3);
                b.extend_from_slice(&fs.0.to_bits().to_le_bytes());
                b.extend_from_slice(&n.to_le_bytes());
            }
            Self::LogBins { fs, n, ppo } => {
                b.push(4);
                b.extend_from_slice(&fs.0.to_bits().to_le_bytes());
                b.extend_from_slice(&n.to_le_bytes());
                b.extend_from_slice(&ppo.to_le_bytes());
            }
        }
        b
    }

    /// Stable id.
    pub fn id(&self) -> GridId {
        let h = self.canonical_bytes().iter().fold(FNV_OFFSET, |h, &x| {
            (h ^ u64::from(x)).wrapping_mul(FNV_PRIME)
        });
        GridId(h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable() {
        // Golden values: changing the canonical encoding changes every id on the wire.
        let log = GridDef::Log {
            ppo: 48,
            k_min: -240,
            k_max: 239,
        };
        assert_eq!(log.len(), 480);
        assert_eq!(log.id(), GridId(0x79ec_3d16_ae0e_94d0), "{}", log.id());
    }

    #[test]
    fn ids_differ() {
        let a = GridDef::Linear {
            fs: Hz(48000.0),
            n: 4096,
        };
        let b = GridDef::Linear {
            fs: Hz(44100.0),
            n: 4096,
        };
        assert_ne!(a.id(), b.id());
        assert_eq!(a.len(), 2049);
    }

    #[test]
    fn log_bins_partition_every_bin() {
        for (fs, n, ppo) in [
            (48_000.0, 65_536, 96),
            (44_100.0, 32_768, 96),
            (96_000.0, 4096, 48),
            (48_000.0, 256, 96),
            (48_000.0, 65_536, 1),
        ] {
            let c = BinColumns::new(fs, n, ppo);
            let g = GridDef::LogBins { fs: Hz(fs), n, ppo };
            assert_eq!(g.len(), c.len());
            assert_eq!(c.first_bin.len(), c.len() + 1);
            assert_eq!(c.first_bin[0], 0);
            assert_eq!(c.first_bin[c.len()], n / 2 + 1);
            assert!(c.first_bin.windows(2).all(|w| w[0] < w[1]), "{fs} {n}");
            assert!(c.edges.windows(2).all(|w| w[0] < w[1]), "{fs} {n}");
            let df = fs / f64::from(n);
            for (i, w) in c.first_bin.windows(2).enumerate() {
                for k in w[0]..w[1] {
                    let f = f64::from(k) * df;
                    assert!(c.edges[i] <= f && f < c.edges[i + 1], "bin {k} col {i}");
                }
            }
            assert!(c.len() <= (n / 2 + 1) as usize);
            // Below the top column, a multi-bin column is 1/ppo octave wide.
            let i = c.len() - 2;
            if c.first_bin[i + 1] - c.first_bin[i] > 1 {
                let octaves = (c.edges[i + 1] / c.edges[i]).log2();
                assert!((octaves * f64::from(ppo) - 1.0).abs() < 1e-9, "{octaves}");
            }
        }
    }

    #[test]
    fn default_spectrum_is_display_sized() {
        // 65 536 points at 48 kHz: 32 769 bins in 897 columns. A bin is 1/96 octave wide
        // at ~101 Hz; below that every bin is its own column.
        let c = BinColumns::new(48_000.0, 65_536, 96);
        assert_eq!(c.len(), 897);
        let df = 48_000.0 / 65_536.0;
        // Bins 0 … 137 are single (1 / (2^(1/96) − 1) = 138.0); the 1/96-octave columns
        // start at the upper edge of bin 137 and hold one bin each at first.
        assert_eq!(c.first_bin[136..141], [136, 137, 138, 139, 140]);
        assert!((c.centres[100] - 100.0 * df).abs() < 1e-12);
        assert!((c.edges[138] - 137.5 * df).abs() < 1e-9);
        // Near the top a column holds ~170 Hz / 0.73 Hz ≈ 235 bins (the last one ends at
        // Nyquist, so it may hold fewer).
        let top = c.first_bin[c.len() - 1] - c.first_bin[c.len() - 2];
        assert!((150..300).contains(&top), "{top}");
        let g = GridDef::LogBins {
            fs: Hz(48_000.0),
            n: 65_536,
            ppo: 96,
        };
        // Golden: the canonical encoding is on the wire.
        assert_eq!(g.id(), GridId(0x8269_d8d7_79e5_5b85), "{}", g.id());
    }
}
