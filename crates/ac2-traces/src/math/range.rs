//! The range across positions of an average (`docs/design/spatial-walk.md` §10): per column,
//! how far the positions' levels spread, without a centre. The power average sits wherever
//! the loudest seats pull it; an envelope or a pair of percentiles makes no claim about a
//! centre, so it shows where seats agree (an EQ helps all of them) and where they scatter.
//!
//! Below [`RangeSpread::PERCENTILE_FROM`] positions a 10th or 90th percentile is no more than
//! an interpolation between the two lowest or highest levels, so the band is the envelope;
//! from there on one stray seat no longer sets its edges. Percentiles interpolate linearly
//! between order statistics at rank `p·(n − 1)` (Hyndman–Fan type 7), so 0 % and 100 % are
//! the envelope's edges.

use ac2_proto::model::{RangeBand, RangeSpread};

use super::Input;

/// The `p`-th quantile (`0..=1`) of `sorted` (ascending, not empty).
fn quantile(sorted: &[f64], p: f64) -> f64 {
    let r = p * (sorted.len() - 1) as f64;
    let (i, t) = (r.floor() as usize, r.fract());
    match sorted.get(i + 1) {
        Some(next) => sorted[i] + t * (next - sorted[i]),
        None => sorted[i],
    }
}

/// The range band of `inputs` (each operand's levels in dB, on one grid) and what it spans;
/// no band below [`RangeSpread::MIN_POSITIONS`] operands. A column is in the band only where
/// every operand has a level: one missing seat would narrow it to the others. Its `coh` is
/// the lowest γ² among the operands that have one (NaN when none has), so a display can
/// leave the column out where any position would be blanked as noise.
pub fn range_band(inputs: &[Input<'_>]) -> (RangeSpread, Option<RangeBand>) {
    let spread = RangeSpread::of(inputs.len());
    let (lo_p, hi_p) = match spread {
        RangeSpread::TooFew => return (spread, None),
        RangeSpread::Envelope { .. } => (0.0, 1.0),
        RangeSpread::Percentiles { .. } => RangeSpread::PERCENTILES,
    };
    let n = inputs.first().map_or(0, |i| i.columns.len());
    let mut band = RangeBand {
        lo: Vec::with_capacity(n),
        hi: Vec::with_capacity(n),
        coh: Vec::with_capacity(n),
    };
    let mut levels = Vec::with_capacity(inputs.len());
    for i in 0..n {
        levels.clear();
        levels.extend(
            inputs
                .iter()
                .map(|x| f64::from(x.columns.mag_db.get(i).copied().unwrap_or(f32::NAN))),
        );
        let (lo, hi) = if levels.iter().all(|v| v.is_finite()) {
            levels.sort_by(f64::total_cmp);
            (
                quantile(&levels, lo_p) as f32,
                quantile(&levels, hi_p) as f32,
            )
        } else {
            (f32::NAN, f32::NAN)
        };
        let coh = inputs
            .iter()
            .filter_map(|x| x.columns.coherence.as_ref()?.get(i).copied())
            .filter(|g| g.is_finite())
            .reduce(f32::min)
            .unwrap_or(f32::NAN);
        band.lo.push(lo);
        band.hi.push(hi);
        band.coh.push(coh);
    }
    (spread, Some(band))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::columns::Columns;

    fn cols(levels: &[f32], coh: Option<&[f32]>) -> Columns {
        Columns {
            mag_db: levels.to_vec(),
            phase_deg: None,
            coherence: coh.map(<[f32]>::to_vec),
        }
    }

    /// One column per operand level set: `levels[k]` is operand `k`'s level in every column.
    fn band_of(columns: &[Columns]) -> (RangeSpread, Option<RangeBand>) {
        let inputs: Vec<Input<'_>> = columns
            .iter()
            .map(|c| Input {
                columns: c,
                delay: 0.0,
                time_base: None,
            })
            .collect();
        range_band(&inputs)
    }

    fn single(levels: &[f32]) -> Vec<Columns> {
        levels.iter().map(|l| cols(&[*l], None)).collect()
    }

    #[test]
    fn below_three_positions_no_band() {
        let (s, b) = band_of(&single(&[-3.0, 1.0]));
        assert_eq!(s, RangeSpread::TooFew);
        assert!(b.is_none());
    }

    #[test]
    fn envelope_below_six_positions() {
        let (s, b) = band_of(&single(&[-3.0, 2.5, -8.0, 0.0, 1.0]));
        assert_eq!(s, RangeSpread::Envelope { positions: 5 });
        let b = b.expect("a band");
        assert_eq!((b.lo[0], b.hi[0]), (-8.0, 2.5));
        let (s, b) = band_of(&single(&[-3.0, 2.5, -8.0]));
        assert_eq!(s, RangeSpread::Envelope { positions: 3 });
        let b = b.expect("a band");
        assert_eq!((b.lo[0], b.hi[0]), (-8.0, 2.5));
    }

    #[test]
    fn percentiles_from_six_positions() {
        // Sorted: −8, −3, 0, 1, 2.5, 9. Rank 0.1·5 = 0.5: −8 + 0.5·5 = −5.5; rank 0.9·5 = 4.5:
        // 2.5 + 0.5·6.5 = 5.75. The envelope would have been −8 … 9.
        let (s, b) = band_of(&single(&[1.0, -3.0, 9.0, -8.0, 2.5, 0.0]));
        assert_eq!(s, RangeSpread::Percentiles { positions: 6 });
        let b = b.expect("a band");
        assert!((b.lo[0] - -5.5).abs() < 1e-6, "{}", b.lo[0]);
        assert!((b.hi[0] - 5.75).abs() < 1e-6, "{}", b.hi[0]);
        // Eight positions 0 … 7 dB: ranks 0.7 and 6.3.
        let (s, b) = band_of(&single(&[7.0, 6.0, 5.0, 4.0, 3.0, 2.0, 1.0, 0.0]));
        assert_eq!(s, RangeSpread::Percentiles { positions: 8 });
        let b = b.expect("a band");
        assert!((b.lo[0] - 0.7).abs() < 1e-6 && (b.hi[0] - 6.3).abs() < 1e-6);
    }

    #[test]
    fn a_missing_level_leaves_the_column_out() {
        let c = [
            cols(&[0.0, 1.0], None),
            cols(&[2.0, f32::NAN], None),
            cols(&[-1.0, 3.0], None),
        ];
        let b = band_of(&c).1.expect("a band");
        assert_eq!((b.lo[0], b.hi[0]), (-1.0, 2.0));
        assert!(b.lo[1].is_nan() && b.hi[1].is_nan());
    }

    #[test]
    fn coherence_is_the_lowest_known() {
        let c = [
            cols(&[0.0, 0.0, 0.0], Some(&[0.9, 0.2, f32::NAN])),
            cols(&[1.0, 1.0, 1.0], Some(&[0.8, 0.95, f32::NAN])),
            cols(&[2.0, 2.0, 2.0], None),
        ];
        let b = band_of(&c).1.expect("a band");
        assert_eq!(b.coh[0], 0.8);
        assert_eq!(b.coh[1], 0.2);
        assert!(b.coh[2].is_nan());
    }
}
