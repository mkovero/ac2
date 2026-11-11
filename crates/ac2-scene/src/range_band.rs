//! An average's range across positions (`docs/design/spatial-walk.md` §10): the daemon sends
//! per column how far the positions' levels spread ([`RangeBand`]); the transfer view shades
//! it faintly behind the average's magnitude, dashed at its edges, in the plot-hint style of
//! the resolution marker ([`crate::unresolved`]), and the legend says what it spans.
//!
//! A column where any position's γ² is below the view's coherence mask is left out: there
//! the band would be the spread of noise, not of seats. That is the threshold that blanks a
//! single curve, so the band never runs where its own positions would show a gap.
//!
//! The wire carries that γ² in hundredths rounded down (`docs/protocol.md` §5.3): the mask's
//! steps are hundredths, and `k / 100` as `f32` orders like `k`, so `γ² < mask` on the
//! received value blanks exactly the columns it would on the daemon's (0.2996 arrives as
//! 0.29, below a mask at 0.3).

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
            c.base
                .polylines
                .push(Polyline::clipped(points, theme.unresolved_edge, plot));
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

    /// The band as it arrives: γ² in hundredths rounded down still masks exactly where the
    /// daemon's value would, at every mask step in hundredths.
    #[test]
    fn coherence_mask_is_exact_after_the_wire() {
        use ac2_proto::frame::{FrameData, decode_frame, encode_frame};
        let mut f = ac2_proto::samples::tf_frame();
        let FrameData::Tf(tf) = &mut f.data else {
            panic!("a tf frame")
        };
        let coh = [0.2996f32, 0.3, 0.30004, 0.0999, 1.0, 0.0];
        let mut band = tf.range.clone().expect("the sample has a band");
        for (i, g) in coh.iter().enumerate() {
            band.lo[10 + i] = -3.0;
            band.hi[10 + i] = 3.0;
            band.coh[10 + i] = *g;
        }
        tf.range = Some(band);
        let parts = encode_frame(&f).expect("encodes");
        let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
        let FrameData::Tf(got) = decode_frame(&refs).expect("decodes").data else {
            panic!("a tf frame")
        };
        let got = got.range.expect("a band");
        assert_eq!(got.coh[10..16], [0.29, 0.3, 0.3, 0.09, 1.0, 0.0]);
        let band = RangeBand {
            lo: got.lo[10..16].to_vec(),
            hi: got.hi[10..16].to_vec(),
            coh: got.coh[10..16].to_vec(),
        };
        let at = |th: f32| {
            let style = CoherenceStyle {
                blank_below: Some(th),
                ..CoherenceStyle::default()
            };
            edges(&band, 0.0, &style)
                .iter()
                .map(|e| e.0.is_nan())
                .collect::<Vec<_>>()
        };
        assert_eq!(at(0.3), [true, false, false, true, false, true]);
        for k in 0..=100u8 {
            let th = f32::from(k) / 100.0;
            let want: Vec<bool> = coh.iter().map(|g| *g < th).collect();
            assert_eq!(at(th), want, "mask {th}");
        }
    }
}
