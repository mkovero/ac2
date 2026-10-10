//! Where a curve's columns are finer than its analysis resolves.
//!
//! An estimate at frequency `f` summarises a band its analysis window sets: one resolution
//! cell, `1/T` wide for a window of `T` seconds (one FFT bin of an unpadded window). A log
//! grid of `N` points per octave has columns `f · (2^(1/2N) − 2^(−1/2N)) = f / κ(N)` wide
//! ([`crate::mtw::layout::validity_factor`]). Where the cell is wider than the column,
//! neighbouring columns read the same cell: the extra columns are interpolation (or, on the
//! MTW grid, columns left without a bin), not detail. That happens below `κ(N) · cell`, and
//! wherever a different window takes over (an MTW stage, a harmonic order's own window) the
//! comparison starts again, so the result is a set of ranges, not one edge.

use crate::grid::LogGrid;

/// Ranges (column edges, Hz, ascending and disjoint) of `grid` whose column is narrower
/// than the resolution cell `cell_hz(f)` at its centre. `None` from `cell_hz`: nothing is
/// estimated there, so nothing is marked.
pub fn unresolved_ranges(grid: &LogGrid, cell_hz: impl Fn(f64) -> Option<f64>) -> Vec<(f64, f64)> {
    unresolved_columns(grid, |i| cell_hz(grid.frequency(i)))
}

/// [`unresolved_ranges`] with the cell given per column index.
pub fn unresolved_columns(
    grid: &LogGrid,
    cell_hz: impl Fn(usize) -> Option<f64>,
) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::new();
    let mut open: Option<(f64, f64)> = None;
    for i in 0..grid.len() {
        let (lo, hi) = grid.edges(i);
        // A relative margin keeps a column that is as wide as the cell (an edge the
        // estimator was designed for) on the resolved side despite rounding.
        let coarse = cell_hz(i).is_some_and(|c| c > (hi - lo) * (1.0 + 1e-9));
        match (coarse, open.as_mut()) {
            (true, Some(r)) => r.1 = hi,
            (true, None) => open = Some((lo, hi)),
            (false, Some(_)) => out.extend(open.take()),
            (false, None) => {}
        }
    }
    out.extend(open);
    out
}

/// Lowest frequency at which a cell of `cell_hz` fills no more than one column of `ppo`
/// points per octave: `κ(ppo) · cell`.
pub fn resolved_from_hz(cell_hz: f64, ppo: u32) -> f64 {
    crate::mtw::layout::validity_factor(ppo) * cell_hz
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ten_octaves(ppo: u32) -> LogGrid {
        let p = ppo as i32;
        LogGrid {
            ppo,
            k_min: -5 * p,
            k_max: 5 * p - 1,
        }
    }

    #[test]
    fn a_constant_cell_marks_everything_below_kappa_cell() {
        // A 1 s window: 1 Hz cells. At 1/48 a column is one hertz wide at κ(48) ≈ 69.25 Hz.
        let g = ten_octaves(48);
        let r = unresolved_ranges(&g, |_| Some(1.0));
        assert_eq!(r.len(), 1);
        let (lo, hi) = r[0];
        assert!((lo - g.edges(0).0).abs() < 1e-9);
        let edge = resolved_from_hz(1.0, 48);
        assert!((edge - 69.2488).abs() < 1e-3);
        // The range ends on the edge of the last column narrower than the cell: within
        // one column of κ·cell.
        let col = 2f64.powf(1.0 / 48.0);
        assert!(hi <= edge * col.sqrt() && hi >= edge / col.sqrt(), "{hi}");
        // Twice the points per octave: twice the edge.
        let r96 = unresolved_ranges(&ten_octaves(96), |_| Some(1.0));
        let e96 = resolved_from_hz(1.0, 96);
        assert!((e96 / edge - 2.0).abs() < 1e-3);
        assert!(
            (r96[0].1 / e96 - 1.0).abs() < 2f64.powf(1.0 / 192.0) - 1.0,
            "{r96:?}"
        );
    }

    #[test]
    fn nothing_estimated_nothing_marked() {
        let g = ten_octaves(24);
        assert!(unresolved_ranges(&g, |_| None).is_empty());
        assert!(unresolved_ranges(&g, |_| Some(1e-6)).is_empty());
        // A cell that switches: two separate ranges.
        let r = unresolved_ranges(&g, |f| Some(if f < 1000.0 { 4.0 } else { 100.0 }));
        assert_eq!(r.len(), 2, "{r:?}");
        assert!(r[0].1 <= 1000.0 && r[1].0 >= 1000.0 / 1.1);
    }
}
