//! Tracked peaks (P) in the 3D view: the measurement's peaks on the skyline's newest row,
//! each a box in screen space with its frequency and level on an axis label's plate.

use super::scene::{Projector, label_box, plate, thin};
use super::*;
use crate::format;
use crate::spectrum::peaks::Peak;

/// A tracked peak as placed.
#[derive(Clone, Debug, PartialEq)]
pub struct PeakPoint3d {
    pub freq_hz: f64,
    /// The peak's displayed level (offset included): what its label says.
    pub level: f64,
    /// Where it stands on the floor's grid: world x, height and depth (the newest row's).
    pub world: [f64; 3],
    /// Its box's centre on screen.
    pub pos: [f32; 2],
    /// Its label as drawn; `None` when it would overlap a louder one's or be cut.
    pub label: Option<String>,
}

/// Half a point's box, pixels: the 2D view's box, so it stays findable at any depth.
const BOX_HALF: f32 = 4.0;

/// The points of `peaks` (loudest first, levels as stored) of a curve offset by `offset_db`
/// and read in `unit`, on skyline `sk` (its rows drawn as `front`) within level range
/// `range`. A peak outside the frequency span, under the level range, over a cell the newest
/// row has no level in, or behind the eye gets none; one above the range top stands on its
/// spike, which keeps growing there. Its height is its level clamped to its cell's spike on
/// the newest row (a cell is a coarser grid than the peak's bin); the label keeps its own.
#[allow(clippy::too_many_arguments)]
pub(super) fn peak_points(
    c: &mut Canvas,
    pj: &Projector,
    sk: &Skyline,
    front: DrawnRows,
    peaks: &[Peak],
    offset_db: f64,
    unit: &str,
    range: Range,
    plot: Rect,
    small: f32,
) -> Vec<PeakPoint3d> {
    let (Some((lo, hi)), Some(newest)) = (sk.span_hz(), sk.row_back(0)) else {
        return Vec::new();
    };
    let cols = sk.cols();
    if cols == 0 || sk.rows() == 0 || !range.is_valid() {
        return Vec::new();
    }
    // The newest row glides back with the rows ([`Skyline::drawn`]), as the renderer
    // places it, each spike where in its cell the renderer stands it.
    let foot = |col| {
        spike_foot(
            cols,
            sk.rows(),
            (col, 0),
            f64::from(front.phase),
            front.newest_id,
        )
    };
    let mut points = Vec::new();
    for p in peaks {
        let level = p.level + offset_db;
        if !(lo..=hi).contains(&p.freq_hz) || !level.is_finite() || level < range.lo {
            continue;
        }
        let col =
            (((p.freq_hz / lo).ln() / (hi / lo).ln() * cols as f64).floor() as usize).min(cols - 1);
        let Some(top) = newest.get(col).copied().filter(|v| v.is_finite()) else {
            continue;
        };
        let shown = level.min(f64::from(top) + offset_db);
        let n = super::spike_height(shown, range.lo, range.hi);
        let [x, _, z] = foot(col);
        let world = [x, MAX_HEIGHT * n, z];
        let Some(pos) = pj.point(world).filter(|q| pj.inside(*q)) else {
            continue;
        };
        points.push(PeakPoint3d {
            freq_hz: p.freq_hz,
            level,
            world,
            pos,
            label: Some(format!(
                "{}  {} {unit}",
                format::freq_readout(p.freq_hz),
                format::level(level)
            )),
        });
    }
    // The field is dark in every theme: the dark theme's text, on the axis labels' plates.
    let dark = Theme::dark();
    let cands: Vec<(usize, Label)> = points
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let text = p.label.clone()?;
            Some((
                i,
                label(
                    text,
                    [p.pos[0], p.pos[1] - BOX_HALF - 2.0],
                    anchor(HAlign::Center, VAlign::Bottom),
                    small,
                    dark.text,
                ),
            ))
        })
        .collect();
    let mut kept = Vec::new();
    thin(cands, plot, &mut kept);
    for (i, p) in points.iter_mut().enumerate() {
        if !kept.iter().any(|(k, _)| *k == i) {
            p.label = None;
        }
        let [x, y] = p.pos;
        let (x0, y0, x1, y1) = (x - BOX_HALF, y - BOX_HALF, x + BOX_HALF, y + BOX_HALF);
        c.overlay.polylines.push(Polyline::clipped(
            vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]],
            Stroke::solid(dark.text, 1.5),
            plot,
        ));
    }
    for (_, mut l) in kept {
        c.overlay.polygons.push(FillPolygon {
            points: plate(label_box(&l)),
            color: FIELD.with_alpha(PLATE_ALPHA),
            clip: Some(plot),
        });
        l.clip = Some(plot);
        c.overlay.labels.push(l);
    }
    points
}
