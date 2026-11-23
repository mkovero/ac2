//! A transfer curve's columns as drawn: one polyline per curve and pane, broken where a
//! column has no value, and joined across the columns without a bin of their own where the
//! grid is finer than the analysis ([`crate::trace::Joined`]), each estimate there also a
//! point.

use super::*;
use crate::canvas::gapped;
use crate::primitives::{Blend, Point, Points};
use crate::trace::Joined;

/// A bin's point, as a multiple of the line's width: wide enough to read as a dot on the
/// line, small enough that 1 Hz bins a few pixels apart stay separate.
const BIN_POINT_WIDTHS: f32 = 2.6;

/// One trace's values on one mapping, as a gapped polyline; `None` when nothing is visible.
/// `joined` (column-aligned with `values`, or empty) names the columns the line passes
/// over: there it runs on to the next value, unless `wrapped` and the step between the
/// two is a phase wrap, which breaks it as between any two neighbours.
#[allow(clippy::too_many_arguments)]
pub(super) fn trace_line(
    xs: &[f32],
    values: &[f64],
    to_px: impl Fn(f64) -> f32,
    alpha: Option<&[f32]>,
    joined: &[Option<Joined>],
    wrapped: bool,
    stroke: Stroke,
    clip: Rect,
) -> Option<Polyline> {
    let passed = |i: usize| joined.get(i) == Some(&Some(Joined::Thinned));
    let kept: Vec<usize> = (0..xs.len().min(values.len()))
        .filter(|&i| !passed(i))
        .collect();
    let xs: Vec<f32> = kept.iter().map(|&i| xs[i]).collect();
    let ys: Vec<f32> = kept.iter().map(|&i| to_px(values[i])).collect();
    let alpha: Option<Vec<f32>> = alpha.map(|a| {
        kept.iter()
            .map(|&i| a.get(i).copied().unwrap_or(1.0))
            .collect()
    });
    let (points, alpha) = gapped(&xs, &ys, alpha.as_deref(), |a, b| {
        wrapped && (values[kept[b]] - values[kept[a]]).abs() > 180.0
    });
    (!points.is_empty()).then_some(Polyline {
        points,
        alpha,
        stroke,
        clip: Some(clip),
    })
}

/// The estimates of a joined range as points (`joined` and `alpha` column-aligned with
/// `values`): how few bins the line there is drawn through. `None` outside such ranges.
pub(super) fn bin_points(
    xs: &[f32],
    values: &[f64],
    to_px: impl Fn(f64) -> f32,
    alpha: Option<&[f32]>,
    joined: &[Option<Joined>],
    stroke: Stroke,
    clip: Rect,
) -> Option<Points> {
    let points: Vec<Point> = joined
        .iter()
        .zip(xs.iter().zip(values))
        .enumerate()
        .filter(|(_, (j, _))| **j == Some(Joined::Estimate))
        .filter_map(|(i, (_, (x, v)))| {
            let y = to_px(*v);
            (x.is_finite() && y.is_finite()).then(|| {
                let a = alpha.and_then(|a| a.get(i)).copied().unwrap_or(1.0);
                Point {
                    pos: [*x, y],
                    color: stroke.color.with_alpha(a),
                }
            })
        })
        .collect();
    (!points.is_empty()).then_some(Points {
        points,
        size_px: stroke.width * BIN_POINT_WIDTHS,
        blend: Blend::Alpha,
        clip: Some(clip),
    })
}
