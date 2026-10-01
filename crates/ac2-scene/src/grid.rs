//! Column frequencies and band edges of a protocol [`GridDef`].
//!
//! The scene works from the grid definition the daemon publishes (`grid.get`), so column
//! positions on screen are exactly the frequencies the DSP used.

use ac2_proto::GridDef;

/// IEC 61260-1 base-10 octave ratio G = 10^(3/10); IEC band edges are `fm · G^(±1/(2b))`.
fn iec_g() -> f64 {
    10f64.powf(0.3)
}

/// Centre frequency of every column, in column order.
pub fn column_frequencies(g: &GridDef) -> Vec<f64> {
    match g {
        GridDef::Log { ppo, k_min, k_max } => (*k_min..=*k_max)
            .map(|k| 1000.0 * 2f64.powf(f64::from(k) / f64::from(*ppo)))
            .collect(),
        GridDef::IecBands { centres, .. } => centres.iter().map(|c| c.0).collect(),
        GridDef::Linear { fs, n } => (0..=*n / 2)
            .map(|k| f64::from(k) * fs.0 / f64::from(*n))
            .collect(),
    }
}

/// Lower and upper edge of every column: the band a bar covers.
///
/// Log grids: geometric midpoints to the neighbours (`f · 2^(±1/(2·ppo))`). IEC bands: the
/// standard band edges. Linear (FFT) grids: half a bin either side, the lowest edge clamped
/// at 0 Hz.
pub fn column_edges(g: &GridDef) -> Vec<(f64, f64)> {
    let f = column_frequencies(g);
    match g {
        GridDef::Log { ppo, .. } => {
            let h = 2f64.powf(0.5 / f64::from(*ppo));
            f.iter().map(|&c| (c / h, c * h)).collect()
        }
        GridDef::IecBands { fraction, .. } => {
            let h = iec_g().powf(0.5 / f64::from(fraction.b()));
            f.iter().map(|&c| (c / h, c * h)).collect()
        }
        GridDef::Linear { fs, n } => {
            let half = fs.0 / f64::from(*n) / 2.0;
            f.iter().map(|&c| ((c - half).max(0.0), c + half)).collect()
        }
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
        let f = column_frequencies(&g);
        assert_eq!(f.len(), 480);
        assert_eq!(f[240], 1000.0);
        assert!((f[0] - 1000.0 / 32.0).abs() < 1e-9);
        let e = column_edges(&g);
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
        let e = column_edges(&g)[0];
        // Base-10 octave band edges at 1 kHz: 707.946 / 1412.54 Hz.
        assert!((e.0 - 707.945_78).abs() < 1e-3, "{e:?}");
        assert!((e.1 - 1_412.537_5).abs() < 1e-3, "{e:?}");
        let g = GridDef::Linear {
            fs: Hz(48000.0),
            n: 8,
        };
        assert_eq!(
            column_frequencies(&g),
            [0.0, 6000.0, 12000.0, 18000.0, 24000.0]
        );
        assert_eq!(column_edges(&g)[0], (0.0, 3000.0));
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
