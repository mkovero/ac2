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
        }
    }

    /// True when the grid has no columns.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Canonical byte encoding hashed into the id: a tag byte (1 log, 2 IEC bands,
    /// 3 linear) followed by the fields little-endian; floats as IEEE-754 f64 bits.
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
}
