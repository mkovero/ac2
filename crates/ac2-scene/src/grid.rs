//! Column frequencies and band edges of a protocol [`GridDef`] ([`GridDef::centres`],
//! [`GridDef::edges`]), cached per grid.
//!
//! The scene works from the grid definition the daemon publishes (`grid.get`), so column
//! positions on screen are exactly the frequencies the DSP used.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use ac2_proto::{GridDef, GridId};

/// Frequencies and edges of a grid's columns, computed once per grid.
#[derive(Debug, Clone, PartialEq)]
pub struct GridColumns {
    /// [`GridDef::centres`].
    pub freqs: Vec<f64>,
    /// [`GridDef::edges`].
    pub edges: Vec<(f64, f64)>,
    /// [`bin_spacing`].
    pub bin_hz: Option<f64>,
}

/// FFT bin spacing `fs / n` of a grid of FFT bins (gathered for display or not); `None`
/// for band and log grids, whose columns are no FFT's bins.
pub fn bin_spacing(g: &GridDef) -> Option<f64> {
    match g {
        GridDef::Linear { fs, n } | GridDef::LogBins { fs, n, .. } if *n > 0 => {
            Some(fs.0 / f64::from(*n))
        }
        _ => None,
    }
}

/// Most grids [`columns`] remembers: a session has a handful (one per measurement kind and
/// FFT length, plus imported traces' grids).
const COLUMN_CACHE: usize = 64;

/// The columns of `g`, shared: a view repainted every frame asks for the same few grids
/// over and over, and a 65 536-point FFT grid has 32 769 columns.
pub fn columns(g: &GridDef) -> Arc<GridColumns> {
    static CACHE: Mutex<Option<HashMap<GridId, Arc<GridColumns>>>> = Mutex::new(None);
    let id = g.id();
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    let map = cache.get_or_insert_with(HashMap::new);
    if let Some(c) = map.get(&id) {
        return Arc::clone(c);
    }
    if map.len() >= COLUMN_CACHE {
        map.clear();
    }
    let c = Arc::new(GridColumns {
        freqs: g.centres(),
        edges: g.edges(),
        bin_hz: bin_spacing(g),
    });
    map.insert(id, Arc::clone(&c));
    c
}

/// Index of the column whose band holds `hz` (`edges` are each column's `(lo, hi)`), else
/// the nearest one on a log scale. A wide low-frequency bin is one value over its whole band,
/// so a cursor anywhere in it reads that bin, while the cursor itself stays where it was put.
pub fn column_at(freqs: &[f64], edges: &[(f64, f64)], hz: f64) -> Option<usize> {
    if hz.is_nan() || hz <= 0.0 {
        return None;
    }
    let i = edges.partition_point(|e| e.1 <= hz);
    match edges.get(i) {
        Some(&(lo, _)) if lo <= hz && i < freqs.len() => Some(i),
        _ => nearest_column(freqs, hz),
    }
}

/// Index of the column nearest to `hz` on a log scale (the way the axis shows distance).
/// `None` for an empty grid or a non-positive frequency.
pub fn nearest_column(freqs: &[f64], hz: f64) -> Option<usize> {
    if hz.is_nan() || hz <= 0.0 {
        return None;
    }
    freqs
        .iter()
        .enumerate()
        .filter(|(_, f)| **f > 0.0)
        .min_by(|(_, a), (_, b)| {
            (a.ln() - hz.ln())
                .abs()
                .total_cmp(&(b.ln() - hz.ln()).abs())
        })
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::BandFraction;
    use ac2_proto::units::Hz;

    #[test]
    fn log_grid_hits_1k() {
        let g = GridDef::Log {
            ppo: 48,
            k_min: -240,
            k_max: 239,
        };
        let f = g.centres();
        assert_eq!(f.len(), 480);
        assert_eq!(f[240], 1000.0);
        assert!((f[0] - 1000.0 / 32.0).abs() < 1e-9);
        let e = g.edges();
        assert!((e[240].1 / e[240].0 - 2f64.powf(1.0 / 48.0)).abs() < 1e-12);
        // Adjacent columns share an edge.
        assert!((e[240].1 - e[241].0).abs() < 1e-9);
    }

    #[test]
    fn iec_and_linear_edges() {
        let g = GridDef::IecBands {
            fraction: BandFraction::Octave,
            centres: vec![Hz(1000.0)],
        };
        let e = g.edges()[0];
        // Base-10 octave band edges at 1 kHz: 707.946 / 1412.54 Hz.
        assert!((e.0 - 707.945_78).abs() < 1e-3, "{e:?}");
        assert!((e.1 - 1_412.537_5).abs() < 1e-3, "{e:?}");
        let g = GridDef::Linear {
            fs: Hz(48000.0),
            n: 8,
        };
        assert_eq!(g.centres(), [0.0, 6000.0, 12000.0, 18000.0, 24000.0]);
        assert_eq!(g.edges()[0], (0.0, 3000.0));
        assert_eq!(bin_spacing(&g), Some(6000.0));
        let g = GridDef::LogBins {
            fs: Hz(48_000.0),
            n: 32_768,
            ppo: 96,
        };
        assert_eq!(bin_spacing(&g), Some(48_000.0 / 32_768.0));
        let g = GridDef::Log {
            ppo: 48,
            k_min: -240,
            k_max: 239,
        };
        assert_eq!(bin_spacing(&g), None);
    }

    #[test]
    fn log_bins_columns_follow_the_bins() {
        let g = GridDef::LogBins {
            fs: Hz(48_000.0),
            n: 65_536,
            ppo: 96,
        };
        let f = g.centres();
        let e = g.edges();
        assert_eq!(f.len(), g.len());
        assert_eq!(e.len(), g.len());
        // Single bins at the bottom, at their own frequency.
        assert!((f[10] - 10.0 * 48_000.0 / 65_536.0).abs() < 1e-12);
        // Every centre inside its column; columns tile the axis.
        for (c, (lo, hi)) in f.iter().zip(&e) {
            assert!(lo <= c && c < hi);
        }
        assert!(e.windows(2).all(|w| w[0].1 == w[1].0));
        let shared = columns(&g);
        assert_eq!(shared.freqs, f);
        assert!(Arc::ptr_eq(&shared, &columns(&g)));
    }

    #[test]
    fn nearest_is_log_distance() {
        let f = [100.0, 200.0];
        // 141 Hz is the geometric middle: 140 is nearer 100, 142 nearer 200.
        assert_eq!(nearest_column(&f, 140.0), Some(0));
        assert_eq!(nearest_column(&f, 142.0), Some(1));
        assert_eq!(nearest_column(&f, 0.0), None);
        assert_eq!(nearest_column(&[], 100.0), None);
    }
}
