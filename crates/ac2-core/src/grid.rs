//! Frequency grids.
//!
//! Two different octave definitions are used on purpose and must never be mixed:
//! - **Base-2** octaves (ratio 2) for the MTW display grid and smoothing.
//! - **Base-10** octaves (G = 10^(3/10), IEC 61260-1) for standard band centres.
//!
//! The two differ by about 0.24 % per octave, which adds up to visible misplacement over
//! the audio band.

/// Base-2 octave ratio.
pub const OCTAVE_BASE2: f64 = 2.0;

/// IEC 61260-1 base-10 octave ratio G = 10^(3/10).
pub fn iec_octave_ratio() -> f64 {
    10f64.powf(0.3)
}

/// Reference frequency both grids are anchored to.
pub const F_REF_HZ: f64 = 1000.0;

/// Log-spaced display grid, base-2, anchored so 1 kHz is a column.
///
/// Column `i` sits at `F_REF · 2^((k_min + i) / ppo)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LogGrid {
    /// Points per octave.
    pub ppo: u32,
    /// Index of the first column relative to 1 kHz.
    pub k_min: i32,
    /// Index of the last column relative to 1 kHz (inclusive).
    pub k_max: i32,
}

impl LogGrid {
    /// Smallest grid at `ppo` points per octave that covers `[f_lo, f_hi]`.
    ///
    /// # Panics
    /// If `ppo` is zero or the range is not positive and increasing.
    pub fn covering(ppo: u32, f_lo: f64, f_hi: f64) -> Self {
        assert!(ppo > 0, "ppo must be positive");
        assert!(f_lo > 0.0 && f_hi > f_lo, "invalid range {f_lo}..{f_hi}");
        let k = |f: f64| (f / F_REF_HZ).log2() * f64::from(ppo);
        Self {
            ppo,
            k_min: k(f_lo).floor() as i32,
            k_max: k(f_hi).ceil() as i32,
        }
    }

    /// Number of columns.
    pub fn len(&self) -> usize {
        (self.k_max - self.k_min + 1) as usize
    }

    /// True if the grid has no columns (never for a grid built by [`LogGrid::covering`]).
    pub fn is_empty(&self) -> bool {
        self.k_max < self.k_min
    }

    /// Centre frequency of column `i`.
    pub fn frequency(&self, i: usize) -> f64 {
        F_REF_HZ * OCTAVE_BASE2.powf(f64::from(self.k_min + i as i32) / f64::from(self.ppo))
    }

    /// All centre frequencies.
    pub fn frequencies(&self) -> Vec<f64> {
        (0..self.len()).map(|i| self.frequency(i)).collect()
    }

    /// Lower and upper edge of column `i` (geometric midpoints to neighbours).
    pub fn edges(&self, i: usize) -> (f64, f64) {
        let half = OCTAVE_BASE2.powf(0.5 / f64::from(self.ppo));
        let f = self.frequency(i);
        (f / half, f * half)
    }
}

/// IEC 61260-1 exact mid-band frequencies for 1/`b`-octave bands within `[f_lo, f_hi]`.
///
/// Odd `b`: f = 1000 · G^(x/b). Even `b`: f = 1000 · G^((2x+1)/(2b)).
///
/// # Panics
/// If `b` is zero or the range is invalid.
pub fn iec_band_centres(b: u32, f_lo: f64, f_hi: f64) -> Vec<f64> {
    assert!(b > 0, "fraction must be positive");
    assert!(f_lo > 0.0 && f_hi > f_lo, "invalid range {f_lo}..{f_hi}");
    let g = iec_octave_ratio();
    let bf = f64::from(b);
    let exponent = |x: i32| {
        if b % 2 == 1 {
            f64::from(x) / bf
        } else {
            f64::from(2 * x + 1) / (2.0 * bf)
        }
    };
    let x_lo = ((f_lo / F_REF_HZ).log(g) * bf).floor() as i32 - 1;
    let x_hi = ((f_hi / F_REF_HZ).log(g) * bf).ceil() as i32 + 1;
    (x_lo..=x_hi)
        .map(|x| F_REF_HZ * g.powf(exponent(x)))
        .filter(|f| *f >= f_lo * (1.0 - 1e-9) && *f <= f_hi * (1.0 + 1e-9))
        .collect()
}

/// IEC 61260-1 band edges for a band centred at `fm`, fraction 1/`b`.
pub fn iec_band_edges(fm: f64, b: u32) -> (f64, f64) {
    let half = iec_octave_ratio().powf(1.0 / (2.0 * f64::from(b)));
    (fm / half, fm * half)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_khz_is_a_column() {
        let g = LogGrid::covering(48, 20.0, 20_000.0);
        let i = (-g.k_min) as usize;
        assert!((g.frequency(i) - 1000.0).abs() < 1e-9);
        assert!(g.frequency(0) <= 20.0 && g.frequency(g.len() - 1) >= 20_000.0);
    }

    #[test]
    fn display_grid_size_is_about_480_columns() {
        let g = LogGrid::covering(48, 20.0, 20_000.0);
        assert!((478..=482).contains(&g.len()), "len {}", g.len());
    }

    #[test]
    fn edges_tile_the_axis() {
        let g = LogGrid::covering(24, 100.0, 1000.0);
        for i in 0..g.len() - 1 {
            let (_, hi) = g.edges(i);
            let (lo, _) = g.edges(i + 1);
            assert!((hi - lo).abs() < 1e-9 * hi);
        }
    }

    #[test]
    fn iec_third_octave_nominals() {
        // Exact mid-band frequencies round to the familiar nominal values.
        let c = iec_band_centres(3, 19.0, 21_000.0);
        let nominal = [
            20.0, 25.0, 31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0, 250.0, 315.0,
            400.0, 500.0, 630.0, 800.0, 1000.0, 1250.0, 1600.0, 2000.0, 2500.0, 3150.0, 4000.0,
            5000.0, 6300.0, 8000.0, 10_000.0, 12_500.0, 16_000.0, 20_000.0,
        ];
        assert_eq!(c.len(), nominal.len());
        for (f, n) in c.iter().zip(nominal) {
            assert!((f / n - 1.0).abs() < 0.03, "{f} vs nominal {n}");
        }
    }

    #[test]
    fn iec_even_fraction_straddles_1k() {
        let c = iec_band_centres(6, 900.0, 1100.0);
        assert!(c.iter().all(|f| (f - 1000.0).abs() > 1.0));
    }

    #[test]
    fn base2_and_base10_differ() {
        let ten_oct_base2 = OCTAVE_BASE2.powi(10);
        let ten_oct_iec = iec_octave_ratio().powi(10);
        assert!((ten_oct_iec / ten_oct_base2 - 1.0).abs() > 0.02);
    }
}
