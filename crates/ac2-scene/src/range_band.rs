//! An average's range across positions (`docs/design/spatial-walk.md` §10): the daemon sends
//! per column how far the positions' levels spread ([`RangeBand`]); the transfer view shades
//! it faintly behind the average's magnitude, dashed at its edges, in the plot-hint style of
//! the resolution marker ([`crate::unresolved`]), and the legend says what it spans.
//!
//! A column where any position's γ² is below the view's coherence mask is left out: there
//! the band would be the spread of noise, not of seats. That is the threshold that blanks a
//! single curve, so the band never runs where its own positions would show a gap.

use ac2_proto::model::{RangeBand, RangeSpread};

use crate::axis::Mapping;
use crate::canvas::{Canvas, gapped};
use crate::primitives::{Band, BandPoint, Polyline, Rect};
use crate::theme::Theme;
use crate::view::CoherenceStyle;

/// What a curve's range band spans, and the band when there is one.
#[derive(Clone, Copy, Debug)]
pub struct Source<'a> {
    pub spread: RangeSpread,
    pub band: Option<&'a RangeBand>,
}

/// The legend tag: `range of 4 positions`, `10–90 % of 8 positions`,
/// `range: too few positions`.
pub fn legend_text(spread: RangeSpread) -> String {
    match spread {
        RangeSpread::TooFew => "range: too few positions".to_owned(),
        RangeSpread::Envelope { positions } => format!("range of {positions} positions"),
        RangeSpread::Percentiles { positions } => {
            let (lo, hi) = RangeSpread::PERCENTILES;
            format!(
                "{:.0}–{:.0} % of {positions} positions",
                lo * 100.0,
                hi * 100.0
            )
        }
    }
}

/// The band's edges per column of `band` in dB plus `offset_db` (the curve's display offset);
/// NaN where it is left out ([module docs](self)).
pub fn edges(band: &RangeBand, offset_db: f64, style: &CoherenceStyle) -> Vec<(f64, f64)> {
    band.lo
        .iter()
        .zip(&band.hi)
        .zip(band.coh.iter().chain(std::iter::repeat(&f32::NAN)))
        .map(|((lo, hi), g)| {
            let masked = style.blank_below.is_some_and(|th| g.is_finite() && *g < th);
            if masked || !lo.is_finite() || !hi.is_finite() {
                (f64::NAN, f64::NAN)
            } else {
                (f64::from(*lo) + offset_db, f64::from(*hi) + offset_db)
            }
        })
        .collect()
}

/// Shade and dashed edges of `edges` (on `freqs`, visible columns `cols`) in `plot`, under
/// the grid and the curves.
pub(crate) fn draw(
    c: &mut Canvas,
    plot: Rect,
    (xm, ym): (&Mapping, &Mapping),
    freqs: &[f64],
    edges: &[(f64, f64)],
    cols: std::ops::Range<usize>,
    theme: &Theme,
) {
    let cols = cols.start..cols.end.min(edges.len()).min(freqs.len());
    let xs: Vec<f32> = freqs[cols.clone()].iter().map(|f| xm.to_px(*f)).collect();
    let lo: Vec<f32> = edges[cols.clone()].iter().map(|e| ym.to_px(e.0)).collect();
    let hi: Vec<f32> = edges[cols].iter().map(|e| ym.to_px(e.1)).collect();
    let points: Vec<BandPoint> = xs
        .iter()
        .zip(lo.iter().zip(&hi))
        .map(|(&x, (&y0, &y1))| BandPoint { x, y0, y1 })
        .collect();
    if points.iter().all(|p| !p.y0.is_finite()) {
        return;
    }
    c.base.bands.push(Band {
        points,
        color: theme.unresolved_shade,
        clip: Some(plot),
    });
    for ys in [&lo, &hi] {
        let (points, _) = gapped(&xs, ys, None, |_, _| false);
        if !points.is_empty() {
            c.base.polylines.push(Polyline {
                points,
                alpha: vec![],
                stroke: theme.unresolved_edge,
                clip: Some(plot),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legend_texts() {
        assert_eq!(legend_text(RangeSpread::of(2)), "range: too few positions");
        assert_eq!(legend_text(RangeSpread::of(4)), "range of 4 positions");
        assert_eq!(legend_text(RangeSpread::of(5)), "range of 5 positions");
        assert_eq!(legend_text(RangeSpread::of(8)), "10–90 % of 8 positions");
    }

    #[test]
    fn coherence_mask_leaves_columns_out() {
        let band = RangeBand {
            lo: vec![-3.0, -2.0, -1.0, f32::NAN],
            hi: vec![3.0, 2.0, 1.0, 0.0],
            coh: vec![0.9, 0.4, f32::NAN, 0.9],
        };
        let off = CoherenceStyle::default();
        let e = edges(&band, 1.0, &off);
        assert_eq!(e[0], (-2.0, 4.0));
        assert_eq!(e[1], (-1.0, 3.0));
        assert_eq!(e[2], (0.0, 2.0));
        assert!(e[3].0.is_nan());
        let masked = CoherenceStyle {
            blank_below: Some(0.5),
            ..off
        };
        let e = edges(&band, 0.0, &masked);
        assert_eq!(e[0], (-3.0, 3.0));
        // A position below the mask: out. No coherence known: kept, as a curve is.
        assert!(e[1].0.is_nan() && e[1].1.is_nan());
        assert_eq!(e[2], (-1.0, 1.0));
    }
}
